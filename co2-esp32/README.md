# co2-esp32

The firmware for [iroh-co2-monitor](../README.md): an iroh endpoint on an
**ESP32-C61** (RISC-V, PSRAM, full relay + pkarr discovery) with a **Sensirion
SCD30 CO2 sensor** over I2C and the devkit's on-board **WS2812 RGB LED** as a CO2
traffic light. A sensor thread reads CO2 / temperature / humidity every ~2 s, logs
them, colors the LED, and publishes them into shared state; the iroh endpoint serves
that reading over the `iroh-co2/sensor/0` irpc protocol ([`co2-proto`](../co2-proto)'s
`GetLatest`), which the [`co2-wasm`](../co2-wasm) browser GUI polls. (It also still
answers the `echo/0` protocol from bring-up.)

No driver crates: both the SCD30 (16-bit commands + Sensirion CRC-8, over I2C) and
the WS2812 (one-wire waveform clocked out of SPI MOSI) are handled directly in
[`src/co2.rs`](src/co2.rs) / [`src/led.rs`](src/led.rs) — **zero new dependencies**.

## Wiring

The SCD30 is a 100 kHz (standard-mode) I2C device at address `0x61`. VDD accepts
3.3–5.5 V, but keep the I2C logic at 3.3 V for the ESP32.

| SCD30 pin | ESP32-C61 | notes |
|-----------|-----------|-------|
| SDA       | **GPIO4** | |
| SCL       | **GPIO5** | |
| VDD       | 3V3       | SCD30 draws ~19 mA avg / ~75 mA peak — feed it from a solid 3V3 |
| GND       | GND       | |
| SEL       | GND / floating | selects I2C mode (VDD would select Modbus) |

**Why GPIO4 / GPIO5:** on the C61 they're plain GPIOs — not strapping pins
(8/9/15), not the SPI flash/PSRAM bus, not USB (12/13), and not the console UART.
The I2C peripheral is routed to them through the GPIO matrix, so no fixed-function
conflict. Swap the pair in [`src/main.rs`](src/main.rs) (the `sda`/`scl` bindings)
if your layout differs.

Most SCD30 breakouts include their own SDA/SCL pull-ups; the ESP32's internal
pull-ups are also enabled in [`src/co2.rs`](src/co2.rs) as a weak backup. If you
have a bare sensor with no pull-ups, add ~10 kΩ to 3V3 on each line.

## How it's wired in software

`Peripherals::take()` moves to `main()`, which splits the peripherals: the modem
goes to WiFi, and `I2C0` + `GPIO4/5` (sensor) + `SPI2` + `GPIO8` (LED) go to a
dedicated **CO2 thread** ([`src/co2.rs`](src/co2.rs)). That thread owns the I2C
bus, checks the SCD30
firmware version, starts continuous measurement, then polls data-ready and reads a
sample every ~2 s. It's a plain blocking thread — kept off the tokio/main task so
its I2C reads and sleeps never stall the iroh endpoint. Its 8 KB stack may live in
PSRAM (`CONFIG_SPIRAM_ALLOW_STACK_EXTERNAL_MEMORY=y`); that's safe here because the
sensor loop does no flash/NVS writes (the cache-disable hazard that rules out PSRAM
stacks for flash-touching code doesn't apply).

The driver is ~90 lines: 16-bit command pointers, a `read_words` helper that
verifies the CRC-8 on each returned word, and float reassembly (each of CO2 / T /
RH is a big-endian IEEE-754 f32 split across two CRC'd words). Serial output:
```
I (…) co2_esp32: [co2] SCD30 found, firmware 3.66
I (…) co2_esp32: [co2] continuous measurement started (~2 s interval)
I (…) co2_esp32: [co2] CO2 = 812 ppm   T = 23.4 °C   RH = 41.8 %
```

## Fitting relay + sensor on the C61

The C61 has only ~320 KB internal SRAM (~188 KB heap), vs 512 KB on an ESP32/S3, so
relay + discovery + the CO2 sensor barely fit. Two things make it work:

- **The tokio runtime runs on a PSRAM-backed thread.** pthread stacks default to
  internal SRAM (even with `SPIRAM_ALLOW_STACK_EXTERNAL_MEMORY`), so `run_iroh()`
  sets `esp_pthread`'s `stack_alloc_caps = MALLOC_CAP_SPIRAM` before spawning a
  192 KB stack. That moves the deep relay/QUIC/TLS async tree (~84 KB high-water)
  out of internal SRAM, and lets the main-task stack shrink to 32 KB — freeing
  ~65 KB of internal SRAM for the relay's FreeRTOS objects (mutexes/DMA/TCBs, none
  PSRAM-backable). Caveat: that thread must never write flash/NVS (cache-off makes a
  PSRAM stack unreachable) — NVS + WiFi init run on the main task first.
- **Tight internal-SRAM buffers** (small WiFi/lwIP pools, `TRY_ALLOCATE_WIFI_LWIP`)
  keep more internal SRAM free, and the iroh heap lives in PSRAM.

Toolchain (all newer than the rest of the esp Rust ecosystem, because the C61 is new
silicon): **ESP-IDF v5.5.4**, **esp-idf-svc 0.52** (first release that knows the
C61), **espflash 4.4** (`cargo install espflash --locked`).

## RGB LED as a CO2 traffic light

The devkit's on-board **WS2812 RGB LED** (GPIO8, per the `RGB@IO8` silk) shows the
air quality at a glance as a **smooth color fade** (not hard steps): **blue** at
outdoor-ambient ~420 ppm (cleanest) → **green** (fresh) → **yellow** (moderate) →
**red** past ~1500 ppm (stuffy), interpolated continuously in between (dim purple
while warming up or if the sensor isn't wired). Breathe on the sensor and watch it
slide up toward red, then drift back down.

Getting there was less trivial than it should be: the **C61 has no RMT peripheral**
(a cost-reduced C6 — its SOC caps list SPI and I2S but not RMT), so the usual
WS2812 driver path is gone, and the ws2812 driver crates can't be used regardless
(they pin esp-idf-sys 0.36, which predates the C61). So [`src/led.rs`](src/led.rs)
clocks the one-wire waveform out of **SPI2 MOSI (GPIO8)** at 2.4 MHz, encoding each
WS2812 bit as three SPI bits (`110` = 1, `100` = 0) → 9 bytes per pixel. No SCLK or
CS pin is used, so only GPIO8 is consumed.

## Build / run

Flash over the C61's **native USB port** (bottom port on this devkit; the top one
is the CP210x UART bridge — for that one add `ESPFLASH_BAUD=921600`):

```bash
WIFI_CONFIG='SSID:PASSWORD' cargo run --release
```

The device connects to WiFi, brings up the iroh endpoint (prints short + long
tickets and the `Sensor ALPN`), and begins logging CO2 readings + coloring the LED.
Paste the **short ticket** into the [`co2-wasm`](../co2-wasm) browser GUI to see the
readings live from anywhere over the relay.
