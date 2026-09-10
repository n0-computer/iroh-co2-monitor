# iroh-co2-monitor

An [iroh](https://iroh.computer) endpoint running on an **ESP32-C61** (RISC-V, with
PSRAM) that reads a **Sensirion SCD30** CO2 sensor, shows the air quality on the
board's RGB LED, and serves the readings over standard QUIC. Dial it — over the
internet (via an n0 relay) or locally — from a **browser GUI** (Rust compiled to
WebAssembly) or a **terminal GUI** (ratatui).

Breathe on the sensor and watch the number climb, on the board's LED and in the
browser at once.

(It also still answers the `echo/0` protocol from the initial bring-up.)

## Four crates (deliberately not a workspace)

| crate | what |
|-------|------|
| [`co2-proto/`](co2-proto) | shared irpc protocol: `Reading { co2, temperature, humidity }` + `GetLatest` |
| [`co2-esp32/`](co2-esp32) | ESP32-C61 firmware — reads the SCD30, drives the LED, serves the RPC |
| [`co2-wasm/`](co2-wasm) | browser GUI (Rust → WebAssembly): live readings, relay-only |
| [`co2-cli/`](co2-cli) | terminal GUI (ratatui): the same display, plus direct/relay path + RTT |

The firmware can't share a workspace with the GUIs: it needs a different toolchain
(the `esp` channel, target `riscv32imac-esp-espidf`) and a patched, ring-free
iroh/irpc graph, while the GUIs use released iroh/irpc. The `co2-proto` crate is
board-agnostic (no `[patch]`) and a path dependency of all three.

## The board

This targets an **ESP32-C61** specifically — it's a cost-reduced C6 (no RMT
peripheral, only ~320 KB internal SRAM), so the firmware carries C61-specific work
that does **not** apply to a roomier ESP32/S3:

- The WS2812 LED is driven over **SPI** (the C61 has no RMT).
- The relay + sensor only fit by running the tokio runtime on a **PSRAM-backed
  thread** (via `esp_pthread` stack caps), freeing internal SRAM for the relay's
  FreeRTOS objects.

See [`co2-esp32/README.md`](co2-esp32/README.md) for the full memory story and the
newer-tooling requirements (ESP-IDF v5.5.x, esp-idf-svc 0.52, espflash 4.4).

## Wiring

SCD30 over I2C: **SDA → GPIO4**, **SCL → GPIO5**, VDD → 3V3, GND → GND, SEL → GND.
The CO2 traffic-light LED is the devkit's on-board WS2812 on **GPIO8** — no wiring.

## Quick start

1. **Flash the firmware** (needs the esp Rust toolchain — see [`co2-esp32/`](co2-esp32)):
   ```bash
   cd co2-esp32
   WIFI_CONFIG='SSID:PASSWORD' cargo run --release
   ```
   It connects to WiFi and prints an **endpoint ticket** on the serial console (the
   short ticket works globally via discovery), and begins logging CO2 / temperature
   / humidity while coloring the on-board LED.

2. **Open the browser GUI** — the readings, live, as a WebAssembly page:
   ```bash
   cd co2-wasm
   npm run build && npm run serve
   ```
   Open <http://localhost:8080/co2-monitor/>, paste the ticket, and connect. The big
   CO2 number is colored the same way as the board's LED (blue → green → yellow →
   red), fading to gray as a reading ages. Browsers are relay-only; the short ticket
   is enough.

3. **Or the terminal GUI** — same display, plus whether you're on a direct path or a
   relay, and the RTT:
   ```bash
   cd co2-cli
   cargo run --release -- <ENDPOINT_ID>
   ```
   Pass the `Endpoint ID` from the serial console (a ticket works too). Being a full
   native endpoint it hole-punches, so watch the path flip from relay to direct.

## License

MIT OR Apache-2.0.
