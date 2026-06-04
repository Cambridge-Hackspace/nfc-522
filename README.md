# neiam-nfc-522

Firmware for an **ESP32-C3** NFC scanner that drives **up to two MFRC522 readers** and
publishes scanned card UIDs over MQTT.

The device ships unconfigured: on first boot it raises a Wi-Fi setup access point with a
captive portal for entering Wi-Fi and MQTT settings. Once configured, it connects as a
station, links to the broker, and publishes a JSON message for every card tapped on
either reader.

## Features

- **Captive-portal provisioning** — join the setup AP and the config page opens
  automatically (Android / iOS / Windows connectivity probes are redirected to it).
- **Up to two channels** — both MFRC522 readers share one SPI bus. Readers are
  auto-detected at boot, so the same firmware runs on 1- or 2-reader hardware; a missing
  reader is simply skipped.
- **Per-device identity** — a 6-character id derived from the MAC (`<id>`, e.g. `0a1b2c`)
  is used for both the setup SSID and the MQTT topics, so multiple units coexist.
- **Debounced reads** — a resting card is reported once (REQA + HLTA), with a short
  same-UID cooldown to absorb tap bounce.
- **Retained availability** — an MQTT Last-Will marks the device `offline`; it publishes
  `online` on every (re)connect.
- **Optional MQTT auth + TLS** (`mqtts`, using the bundled CA store).
- **Local status page** — once connected, the device serves a small read-only HTTP
  page (and JSON API) on its station IP showing the last 7 scanned UIDs.

## Hardware

### Board
ESP32-C3 (target `riscv32imc-esp-espidf`). Adjust the chip in `.cargo/config.toml` /
`sdkconfig.defaults` if you use a different ESP32 variant. Note that on the C3,
GPIO11–GPIO17 are reserved for the internal SPI flash and must **not** be used for the
reader bus.

### Wiring

Both readers share the SPI bus, power, and ground; only the chip-select (SS) line is
per-reader. Default pins (change in `src/app.rs` `NfcPins` and `src/main.rs`):

| MFRC522 pin | ESP32-C3 GPIO | Notes                              |
|-------------|---------------|------------------------------------|
| SCK         | GPIO 10       | shared by both readers             |
| MOSI (SDA/MOSI) | GPIO 5    | shared                             |
| MISO        | GPIO 7        | shared                             |
| SS / SDA (CS) — reader 0 | GPIO 6  | channel 0                      |
| SS / SDA (CS) — reader 1 | GPIO 4  | channel 1 (optional; auto-skipped) |
| RST         | GPIO 9        | driven high at boot to enable the reader |
| IRQ         | GPIO 8        | unused (firmware polls)            |
| 3.3V        | 3V3           | **do not use 5V**                  |
| GND         | GND           | shared                             |

The BOOT-hold re-provision input is GPIO 0 (see below). Note GPIO 9 is the C3's BOOT
strapping pin but is used here for the reader's RST line.

> Single-reader build: just wire one module to the channel-0 CS; channel 1 stays idle.

## MQTT interface

`<root>` defaults to `neiam` and is set in the portal; topics are namespaced under
`<root>/nfc/`. `<id>` is the device id. `<channel>` is `0` or `1`.

| Topic                              | Direction | Payload                                                      | QoS | Retain |
|------------------------------------|-----------|--------------------------------------------------------------|-----|--------|
| `<root>/nfc/<id>/scans/<channel>`  | publish   | scan envelope (below)                                        | 0   | no     |
| `<root>/nfc/<id>/status`           | publish   | `online` / `offline` (LWT)                                   | 1   | yes    |
| `<root>/nfc/<id>/ip`               | publish   | station IP (e.g. `192.168.1.42`), on connect + every 60s     | 1   | yes    |

### Scan payload

```json
{
  "uid": "04A1B2C3",
  "channel": 0,
  "ts_ms": 123456,
  "type": "Mifare1k"
}
```

- `uid` — uppercase hex of the card UID (4, 7, or 10 bytes).
- `channel` — which reader (`0` or `1`).
- `ts_ms` — milliseconds since boot.
- `type` — PICC type as reported by the reader (`Mifare1k`, `MifareUL`, `Iso14443_4`, …).

## HTTP status page

Once WiFi + MQTT are up, the device runs a small read-only HTTP server on its station
IP (logged at boot, e.g. `status page: http://192.168.1.42/`). It exposes the last
**7** scans — enough to confirm readers are working without watching MQTT.

| Path         | Returns                                                            |
|--------------|-------------------------------------------------------------------|
| `/`          | status page, auto-refreshing every second                         |
| `/app.css`, `/font-*.woff2` | shared portal styling + fonts (gzipped)            |
| `/api/scans` | JSON `{ "id": "<id>", "scans": [ … ] }`, most-recent first         |

The page reuses the same Tailwind/DaisyUI shell as the provisioning portal (navbar,
themes, B612 Mono) and reads everything from `/api/scans`, which also reports the device
id and the (read-only) broker host and topic root:

```json
{ "id": "0a1b2c", "mqtt_host": "192.168.1.10", "topic_root": "neiam", "scans": [ … ] }
```

`scans[]` entries use the same shape as the MQTT [scan payload](#scan-payload). The list
is in-memory only (cleared on reboot) and the server accepts no input — it's purely for
observability. (The provisioning portal at `192.168.71.1` is separate and only runs in
setup mode.)

## Configuration reference

Entered through the captive portal and stored in NVS as a JSON blob:

| Field            | Default | Description                                      |
|------------------|---------|--------------------------------------------------|
| `wifi_ssid`      | —       | Wi-Fi network to join (required)                 |
| `wifi_pass`      | —       | Wi-Fi password (blank = open network)            |
| `mqtt_host`      | —       | Broker hostname or IP (required)                 |
| `mqtt_port`      | `1883`  | Broker port (use `8883` for TLS)                 |
| `mqtt_username`  | —       | Optional broker username                         |
| `mqtt_password`  | —       | Optional broker password                         |
| `mqtt_use_tls`   | off     | Connect with `mqtts` using the bundled CA store  |
| `topic_root`     | `neiam` | Topic prefix                                     |

## Building & flashing

Requires the Espressif Rust toolchain (via [`espup`](https://github.com/esp-rs/espup)),
[`espflash`](https://github.com/esp-rs/espflash), and **Node.js + npm** (the build script
generates the captive-portal CSS — see [Web assets](#web-assets--theming)).

```bash
. ~/export-esp.sh        # put the `esp` toolchain on PATH (once per shell)
cargo build --release    # build firmware (build.rs also builds the portal assets)
cargo run                # flash + monitor (runner = `espflash flash --monitor`)
```

The first build runs `npm ci` in `web/` automatically; later builds reuse `node_modules`
and only re-run Tailwind when files under `web/` change.

> This project pins `esp-idf-{sys,hal,svc}` to esp-rs **git master** (see
> `[patch.crates-io]` in `Cargo.toml`), so its HAL API can differ from the published
> docs.rs versions.

## Web assets / theming

The captive portal is styled with **Tailwind + DaisyUI**, reusing the theming and base
layout of the sibling project `../dms` (navbar + centered card, B612 Mono, a light/dark
theme toggle). Sources live in `web/`:

```
web/
  package.json / package-lock.json   tailwindcss, daisyui, @tailwindcss/forms, @fontsource/b612-mono
  tailwind.config.js                 themes: her (light) + afterdark (dark)
  input.css                          @font-face + @tailwind layers
  src/portal.html, src/saved.html    provisioning pages (served + Tailwind content)
  src/status.html                    normal-mode status page (last-7 scans)
```

`build.rs` runs the pipeline on every build and writes the results to `OUT_DIR`, which the
firmware embeds via `include_bytes!`. The pipeline is built to **preserve flash**:

1. **Purge** — Tailwind emits only the classes the two pages use.
2. **Minify** — `tailwindcss --minify`.
3. **Gzip** — CSS and HTML are gzipped and served with `Content-Encoding: gzip`.
4. **No FontAwesome** (icons are inline SVG); only **two** themes; **latin-subset** font.

Resulting embedded footprint (≈ **48 KB**, font-dominated):

| Asset            | Size     |
|------------------|----------|
| `app.css.gz`     | ~7.8 KB  |
| `portal.html.gz` | ~2.0 KB  |
| `saved.html.gz`  | ~0.9 KB  |
| B612 Mono woff2  | 2 × 19 KB |

Dropping the 700-weight font (`build.rs` + `input.css`) saves ~19 KB. To edit the look,
change files under `web/` and rebuild. The theme choice persists in the browser
(`localStorage` key `nfc:theme`); everything is served from the device, so the portal
renders fully offline.

## First-time setup

1. Power on an unconfigured device. It starts a Wi-Fi AP named **`scan-setup-<id>`**
   (open, gateway `192.168.71.1`).
2. Join that network from a phone or laptop — the configuration page should open
   automatically. If it doesn't, browse to `http://192.168.71.1/`.
3. Fill in Wi-Fi and MQTT details and submit. The device saves the config and reboots.
4. It connects to your network and broker; confirm with:
   ```bash
   mosquitto_sub -t 'neiam/#' -v      # status -> online, plus scans
   ```
5. Tap cards on each reader and watch messages on `neiam/nfc/<id>/scans/0` and `.../1`.

## Re-provisioning

Hold the **BOOT** button (GPIO 0) for ~3 seconds during power-on. The device starts the
setup AP again so you can enter new settings; submitting the form overwrites the stored
configuration. (Holding BOOT without submitting leaves the existing config untouched on
the next normal boot.)

### Automatic fallback on bad Wi-Fi

If Wi-Fi bring-up fails (e.g. a wrong password or a network that has gone away), the
device reboots and retries. A counter in NVS tracks **consecutive** failures; after
`MAX_BOOT_FAILS` (default **3**) in a row it stops retrying and raises the setup AP on its
own — so a typo'd password recovers without the BOOT button. The counter is reset the
moment Wi-Fi associates and gets an IP, so only *Wi-Fi* failures count toward the
fallback; once connected, MQTT/NFC errors reboot but don't trigger re-provisioning.

## Project layout

```
src/
  main.rs            entry point, mode selection, BOOT-hold reprovision
  device_id.rs       MAC -> device id, setup SSID
  config.rs          Config struct + NVS load/save
  wifi.rs            async STA connect + SoftAP for provisioning
  mqtt.rs            async MQTT client (LWT, optional auth/TLS)
  nfc.rs             shared SPI bus, MFRC522 probe + UID scan/debounce
  httpd.rs           normal-mode status server (last-7-scans HTML + /api/scans)
  app.rs             normal operation: connection pump + scan/publish loop
  provisioning/
    mod.rs           AP + DNS + HTTP orchestration, reboot on save
    http.rs          serves gzipped portal/CSS/font assets, /save handler, captive redirects
    dns.rs           captive DNS responder (answers all queries with the AP IP)
web/                 Tailwind+DaisyUI sources for the portal (built by build.rs)
build.rs             ESP-IDF env + web asset pipeline (Tailwind -> minify -> gzip)
```

## Troubleshooting

- **No `scan-setup-*` AP** — the device already has a stored config; hold BOOT at power-on
  to re-provision.
- **Portal doesn't auto-open** — browse to `http://192.168.71.1/` manually; some clients
  suppress the captive check.
- **`no MFRC522 readers detected`** in the log — check SPI wiring, CS pins, 3.3V power, and
  that RST is held high.
- **No scans published but status is `online`** — verify the reader is detected at boot and
  the card is a supported ISO 14443-A type.

## Continuous integration

`.github/workflows/rust_ci.yml` builds `--release` and runs `cargo fmt --check` and
`cargo clippy -D warnings` on the esp toolchain.
