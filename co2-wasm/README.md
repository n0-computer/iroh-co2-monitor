# co2-wasm

The browser GUI for the [`co2-esp32`](../co2-esp32) firmware: an iroh endpoint
compiled to WebAssembly that dials the device from its ticket and shows the live
CO2 / temperature / humidity — the `GetLatest` RPC over `SENSOR_ALPN` (from
[`co2-proto`](../co2-proto)), polled every 5s. Read-only: a CO2 monitor has nothing
to actuate.

Browsers can't open UDP/QUIC sockets, so this endpoint is **relay-only** (the `N0`
preset): it reaches the device through an n0 relay and resolves its address via
pkarr, so the short ticket is enough.

The big CO2 number is colored the same way as the device's on-board LED — blue
(pristine ~420 ppm) → green → yellow → red — and fades toward gray as the reading
ages, so a live link stays vivid while a stalled one greys out.

## Build

```bash
npm run build      # cargo → wasm-bindgen → self-contained dist/co2-monitor/
npm run serve      # http://localhost:8080/co2-monitor/
```

`npm run build` compiles the crate to `wasm32-unknown-unknown`, runs `wasm-bindgen`
into `public/wasm/`, then `scripts/bundle.mjs` inlines `variants/co2-monitor.html`
+ `public/{style.css,main.js}` into `dist/co2-monitor/index.html` — a drop-in
static directory you can serve or host anywhere. Paste an endpoint ticket and
connect (or pass `?ticket=…` in the URL to auto-connect).

Requires the `wasm32-unknown-unknown` target and a `wasm-bindgen` CLI matching the
pinned `wasm-bindgen` crate version (see [`Cargo.toml`](Cargo.toml)).
