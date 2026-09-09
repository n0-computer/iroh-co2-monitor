const $ticket = document.querySelector("#ticket");
const $connect = document.querySelector("#connect");
const $co2 = document.querySelector("#co2");
const $temp = document.querySelector("#temp");
const $hum = document.querySelector("#hum");
const $status = document.querySelector("#status");
const $conn = document.querySelector("#conn"); // panel grouping the ticket field
const $connToggle = document.querySelector("#conn-toggle"); // gear that shows/hides it

// Namespace persisted state by the page's path segment, so multiple GUI variants
// embedded on one page (each in its own same-origin iframe) don't share an endpoint
// identity / ticket via localStorage.
const NS = location.pathname.replace(/\/(index\.html)?$/, "").split("/").pop() || "co2-monitor";
const SECRET_KEY = `${NS}:secret`;
const TICKET_KEY = `${NS}:ticket`;

// CO2 → color, mirroring the device's LED: blue (pristine ~420 ppm) → green (fresh)
// → yellow (moderate) → red (stuffy), interpolated in RGB. Screen-bright versions of
// the LED stops. A reading fades toward gray as it goes stale (see paint()).
const CO2_STOPS = [
  [420, [59, 130, 246]], // blue
  [650, [34, 197, 94]], // green
  [1000, [234, 179, 8]], // yellow
  [1500, [239, 68, 68]], // red
];
const STALE = [138, 143, 152]; // #8a8f98
const FADE_MS = 30_000;

function co2Color(ppm) {
  if (ppm <= CO2_STOPS[0][0]) return CO2_STOPS[0][1];
  for (let i = 0; i < CO2_STOPS.length - 1; i++) {
    const [p0, c0] = CO2_STOPS[i];
    const [p1, c1] = CO2_STOPS[i + 1];
    if (ppm <= p1) {
      const t = (ppm - p0) / (p1 - p0);
      return c0.map((a, k) => Math.round(a + (c1[k] - a) * t));
    }
  }
  return CO2_STOPS[CO2_STOPS.length - 1][1];
}

let node = null;
let NodeClass = null; // the wasm `Node` class, captured at boot so we can re-spawn
let current = null; // active Subscription handle
let connectedTicket = null; // the ticket `current` is polling
let lastReading = null; // timestamp of the last successful read
let lastCo2 = null; // ppm of the last reading, for the color

// The ticket is low-stakes, so accept it from the query (?ticket=) or the fragment.
function paramFromUrl(name) {
  const frag = new URLSearchParams(location.hash.replace(/^#/, ""));
  return new URLSearchParams(location.search).get(name) ?? frag.get(name);
}

// Prefill the ticket: a ticket on the URL wins (and auto-connects below); otherwise
// fall back to the last ticket we stored.
const urlTicket = paramFromUrl("ticket");
$ticket.value = (urlTicket ?? localStorage.getItem(TICKET_KEY) ?? "").trim();

// The connection panel is setup clutter once you have a ticket, so it's hidden behind
// the gear. Start collapsed if a ticket is already prefilled; open otherwise.
if ($conn) $conn.hidden = $ticket.value.trim() !== "";
function refreshConn() {
  if ($connToggle && $conn) $connToggle.classList.toggle("active", !$conn.hidden);
}
refreshConn();

// The CO2 number is colored by its value, fading toward gray as the reading ages, so
// staleness reads at a glance (and it matches the device's LED). Temp/humidity stay a
// muted gray — CO2 is the hero.
function paint() {
  if (lastReading != null && lastCo2 != null) {
    const t = Math.min((Date.now() - lastReading) / FADE_MS, 1);
    const base = co2Color(lastCo2);
    const c = base.map((f, i) => Math.round(f + (STALE[i] - f) * t));
    $co2.style.color = `rgb(${c[0]}, ${c[1]}, ${c[2]})`;
  }
  requestAnimationFrame(paint);
}
requestAnimationFrame(paint);

function onReading(co2, temp, hum) {
  $co2.textContent = Math.round(co2);
  $temp.textContent = temp.toFixed(1);
  $hum.textContent = hum.toFixed(1);
  lastCo2 = co2;
  lastReading = Date.now();
  $status.textContent = `last reading ${new Date(lastReading).toLocaleTimeString()}`;
}

function onStatus(text) {
  // Once we've had a reading, keep showing when it was rather than clobbering it with
  // raw rpc/connection errors — the greyed-out number already signals stale.
  if (lastReading) {
    $status.textContent = `last reading ${new Date(lastReading).toLocaleTimeString()}`;
  } else {
    $status.textContent = text;
  }
}

// Offer "connect" only once the node is up, the field is non-empty, and it differs
// from what we're already polling.
function refreshConnectButton() {
  const t = $ticket.value.trim();
  $connect.disabled = !node || t === "" || t === connectedTicket;
}

function connect() {
  const ticket = $ticket.value.trim();
  if (!node || !ticket || ticket === connectedTicket) return;
  localStorage.setItem(TICKET_KEY, ticket);
  // Switch devices: stop the previous poll loop (and close its connection) first.
  if (current) {
    current.free();
    current = null;
  }
  // Reset the display for the new device.
  lastReading = null;
  lastCo2 = null;
  $co2.textContent = "—";
  $temp.textContent = "—";
  $hum.textContent = "—";
  onStatus("connecting…");
  current = node.subscribe(ticket, onReading, onStatus);
  connectedTicket = ticket;
  refreshConnectButton();
  // Fold the setup field away now that we're connected.
  if ($conn) $conn.hidden = true;
  refreshConn();
}

$connect.addEventListener("click", connect);
$ticket.addEventListener("input", refreshConnectButton);
$ticket.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && !$connect.disabled) connect();
});

// Gear toggles the connection setup panel.
if ($connToggle) {
  $connToggle.addEventListener("click", () => {
    if (!$conn) return;
    $conn.hidden = !$conn.hidden;
    refreshConn();
  });
}

// Fast reconnect after the tab/phone was frozen. While suspended, the endpoint's relay
// and device connections go stale, and iroh would otherwise wait out its reconnect
// backoff — slow. A manual reload is instant because it starts from scratch, so on
// return to the foreground we do the same thing in place: tear the node down and
// re-spawn + resubscribe (keeping the page intact, unlike a reload).
let hiddenAt = null;
async function reconnectFresh() {
  const ticket = connectedTicket;
  if (!ticket || !NodeClass) return;
  onStatus("reconnecting…");
  if (current) {
    current.free();
    current = null;
  }
  if (node) {
    try {
      node.free();
    } catch (_) {}
    node = null;
  }
  connectedTicket = null; // so connect() proceeds with the same (still-filled) ticket
  try {
    node = await NodeClass.spawn(localStorage.getItem(SECRET_KEY));
    localStorage.setItem(SECRET_KEY, node.secret_hex());
    connect();
  } catch (err) {
    $status.textContent = `reconnect failed: ${err}`;
    console.error(err);
  }
}

document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "hidden") {
    hiddenAt = Date.now();
    return;
  }
  const staleFor = hiddenAt == null ? 0 : Date.now() - hiddenAt;
  hiddenAt = null;
  // Only rebuild if we were connected and away long enough to have gone stale — a
  // quick tab flip doesn't need it.
  if (connectedTicket && staleFor > 5000) reconnectFresh();
});

// Boot the endpoint.
try {
  // Resolve the wasm relative to THIS page's directory. The page may be served with or
  // without a trailing slash — normalize to a directory path, then import from there.
  let dir = location.pathname;
  if (!dir.endsWith("/")) {
    dir = dir.endsWith(".html") ? dir.slice(0, dir.lastIndexOf("/") + 1) : `${dir}/`;
  }
  const { default: init, Node } = await import(`${dir}wasm/co2_wasm.js`);
  await init();
  NodeClass = Node;
  node = await Node.spawn(localStorage.getItem(SECRET_KEY));
  localStorage.setItem(SECRET_KEY, node.secret_hex());
  $status.textContent = "ready — paste a ticket and connect";
  refreshConnectButton();
  if (urlTicket && $ticket.value) connect();
} catch (err) {
  $status.textContent = `failed to start: ${err}`;
  console.error(err);
}
