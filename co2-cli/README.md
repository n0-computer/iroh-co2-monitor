# co2-cli

A terminal GUI for the [`co2-esp32`](../co2-esp32) firmware: the same display as the
[browser GUI](../co2-wasm) — CO2 as a big number colored like the device's LED, with
temperature and humidity underneath — rendered with [ratatui](https://ratatui.rs),
plus a **connection panel** the browser can't offer: whether the QUIC connection is
running over a **direct** path or a **relay**, and its **RTT**.

Unlike the browser this is a full native iroh endpoint (UDP + relay + discovery), so
it hole-punches. On start you'll typically see a relay path first, then a direct
path appear and take over — the panel exists to make that visible.

```
╭───────────────────────────── CO₂ monitor ─────────────────────────────╮
│                                                                       │
│               ██      ██████████    ██████      ██████                │
│             ████      ██          ██      ██  ██      ██              │
│               ██      ████████    ██      ██  ██      ██              │
│               ██              ██  ██      ██  ██      ██              │
│               ██              ██  ██      ██  ██      ██              │
│               ██      ██      ██  ██      ██  ██      ██              │
│             ██████      ██████      ██████      ██████   ppm          │
│                                                                       │
│                          24.5 °C      41.7 %                          │
│                                                                       │
│                          last reading 1s ago                          │
│                                                                       │
│╭ connection ─────────────────────────────────────────────────────────╮│
││  state   connected                                                  ││
││  path    direct  10.68.32.165:60938                                 ││
││  rtt     7.8 ms                                                     ││
││                                                                     ││
││  paths   ● direct    7.8 ms  10.68.32.165:60938                     ││
││          ○ relay    81.3 ms  https://use1-1.relay.n0.iroh.link      ││
││                                                                     ││
││  remote  c464cf5958b8512cb0820640f0bf9e74665360d6e50cd4afc834b3…    ││
││  me      770010e8cb2e53adef1eec3b18355d0d8db9ddc4f3a8ec1e5f8e8a…    ││
│╰─────────────────────────────────────────────────────────────────────╯│
│                                q quit                                 │
╰───────────────────────────────────────────────────────────────────────╯
```

## Run

```bash
cd co2-cli
cargo run --release -- <ENDPOINT_ID>
```

`<ENDPOINT_ID>` is the `Endpoint ID` the firmware prints on its serial console; iroh
resolves it through discovery, exactly like the short ticket. A full endpoint ticket
(short or long) is accepted in its place. Press `q` (or `Esc` / `Ctrl-C`) to quit.

Options:

| flag | default | |
|------|---------|-|
| `-i, --interval <SECS>` | `5` | seconds between `GetLatest` polls (the SCD30 updates every ~2 s) |

The TUI owns stdout, so logging is off unless you ask for it — and then it goes to
stderr, which you'll want redirected:

```bash
RUST_LOG=iroh=debug cargo run --release -- <ENDPOINT_ID> 2> co2-cli.log
```

## What it shows

- **CO2** — the hero number, colored blue (pristine ~420 ppm) → green → yellow → red
  with the same stops as the LED and the web GUI, fading to gray over 30 s as the
  reading ages so a stalled link greys out.
- **°C / %** — temperature and humidity, muted.
- **status** — the connection state until the first reading; afterwards the age of
  the last reading (the greyed number already signals stale).
- **connection** — `state` (connecting / connected / reconnecting), the **selected
  path** (`direct` in green, `relay` in yellow) with its remote address and `rtt`,
  then **every open path** (● selected, ○ standby) with its own RTT — iroh keeps the
  relay path warm even after a direct one wins. The path list is re-sampled every
  500 ms. Finally both endpoint IDs.

## How it connects

The browser GUI uses irpc's `IrohLazyRemoteConnection`, which hides the QUIC
connection and reconnects internally. That's exactly what this can't use: reading
the path list needs the `Connection` itself. So [`net.rs`](src/net.rs) dials with
`Endpoint::connect`, wraps the connection in `IrohRemoteConnection` for the irpc
client, samples `Connection::paths()` for the panel, and reconnects by hand when a
poll fails, times out (10 s), or the connection closes.

Same crate graph as the browser GUI (released iroh / irpc, `co2-proto` by path) —
this crate is deliberately not in a workspace with the firmware, for the reasons in
the [top-level README](../README.md).
