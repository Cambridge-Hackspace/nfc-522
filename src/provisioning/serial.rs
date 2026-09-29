//! Serial (UART/USB-CDC) configuration protocol, used by the browser-based
//! provisioning page as an alternative to joining the AP + HTTP portal.
//!
//! Line-based, newline-delimited, every protocol line prefixed `NFC522:` so it
//! can never collide with ESP-IDF's own log format (`I (12345) tag: msg`,
//! which never starts with `N`). The host filters incoming lines for this
//! prefix and ignores everything else as log noise.
//!
//! | Direction      | Line                     | Meaning                        |
//! |----------------|---------------------------|--------------------------------|
//! | device -> host | `NFC522:READY <id>`       | sent once when the listener starts |
//! | host -> device | `NFC522:PING`             | liveness probe                 |
//! | device -> host | `NFC522:PONG <id>`        | reply to PING                  |
//! | host -> device | `NFC522:GET`              | request current settings       |
//! | device -> host | `NFC522:CFG <json>`       | current settings + fw_version, passwords omitted (see [`redact`]) |
//! | host -> device | `NFC522:SET <json>`       | `<json>` deserializes as [`Config`]; blank password fields keep the stored password (see [`merge_passwords`]) |
//! | device -> host | `NFC522:OK`               | config saved                   |
//! | device -> host | `NFC522:ERR <message>`    | parse or save failure          |
//! | host -> device | `NFC522:LED <on\|off\|toggle>` | drive the board LED (GPIO8, active-low) |
//! | device -> host | `NFC522:LED <on\|off>`    | new LED state, confirming the command |

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use esp_idf_svc::hal::gpio::{AnyIOPin, Output, PinDriver};
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use crate::config::{self, Config};

/// Firmware version reported to the host via `GET`, so the provisioning page
/// can show what's already on a device before overwriting its settings.
const FW_VERSION: &str = env!("CARGO_PKG_VERSION");

type LedPin = PinDriver<'static, Output>;

/// Spawn the serial config listener. `saved` is the same flag the HTTP portal
/// uses; `provisioning::run`'s poll loop reboots once it flips true,
/// regardless of which source set it. `led` is the board LED (GPIO8) — owned
/// entirely by this thread, since nothing else touches it in provisioning
/// mode.
pub fn run(
    nvs: EspDefaultNvsPartition,
    saved: Arc<AtomicBool>,
    id: String,
    led: AnyIOPin<'static>,
) -> Result<()> {
    std::thread::Builder::new()
        .stack_size(4096)
        .spawn(move || {
            println!("NFC522:READY {id}");

            let mut led_pin: Option<LedPin> = match PinDriver::output(led) {
                Ok(pin) => Some(pin),
                Err(e) => {
                    log::warn!("LED pin setup failed ({e}); NFC522:LED will report ERR");
                    None
                }
            };
            let mut led_on = false;

            // Read one byte at a time rather than `read_line` (which buffers
            // internally in a way that hid this): the console fd here doesn't
            // block the way a normal blocking read would, so a tight retry
            // loop pegs this core and starves the IDLE task, tripping the
            // task watchdog. Sleeping whenever no byte was read keeps this
            // thread cooperative regardless of the fd's actual blocking
            // behavior. Confirmed on real ESP32-C3 hardware: without the
            // sleep, the board watchdog-resets a few seconds after boot.
            let mut stdin = std::io::stdin();
            let mut byte = [0u8; 1];
            let mut line = String::new();

            loop {
                match stdin.read(&mut byte) {
                    Ok(1) => match byte[0] {
                        b'\n' => {
                            handle_line(&line, &nvs, &saved, &id, &mut led_pin, &mut led_on);
                            line.clear();
                        }
                        b'\r' => {}
                        b => line.push(b as char),
                    },
                    _ => std::thread::sleep(Duration::from_millis(20)),
                }
            }
        })?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_line(
    line: &str,
    nvs: &EspDefaultNvsPartition,
    saved: &Arc<AtomicBool>,
    id: &str,
    led_pin: &mut Option<LedPin>,
    led_on: &mut bool,
) {
    let Some(rest) = line.trim_end().strip_prefix("NFC522:") else {
        return;
    };

    if let Some(json) = rest.strip_prefix("SET ") {
        match serde_json::from_str::<Config>(json).map_err(anyhow::Error::from) {
            Ok(mut cfg) => {
                merge_passwords(&mut cfg, &config::load(nvs));
                match config::save(nvs, &cfg) {
                    Ok(()) => {
                        println!("NFC522:OK");
                        saved.store(true, Ordering::SeqCst);
                    }
                    Err(e) => println!("NFC522:ERR {e}"),
                }
            }
            Err(e) => println!("NFC522:ERR {e}"),
        }
    } else if rest.trim() == "GET" {
        match serde_json::to_string(&redact(&config::load(nvs))) {
            Ok(json) => println!("NFC522:CFG {json}"),
            Err(e) => println!("NFC522:ERR {e}"),
        }
    } else if rest.trim() == "PING" {
        println!("NFC522:PONG {id}");
    } else if let Some(cmd) = rest.strip_prefix("LED ") {
        handle_led(cmd.trim(), led_pin, led_on);
    }
}

/// Drive the board LED. Active-low (mirrors `app.rs`'s normal-operation LED
/// handling): `set_low()` turns it on, `set_high()` turns it off.
fn handle_led(cmd: &str, led_pin: &mut Option<LedPin>, led_on: &mut bool) {
    let Some(pin) = led_pin.as_mut() else {
        println!("NFC522:ERR LED not available");
        return;
    };

    let new_state = match cmd {
        "on" => true,
        "off" => false,
        "toggle" => !*led_on,
        _ => {
            println!("NFC522:ERR unknown LED command");
            return;
        }
    };

    let result = if new_state {
        pin.set_low()
    } else {
        pin.set_high()
    };
    match result {
        Ok(()) => {
            *led_on = new_state;
            println!("NFC522:LED {}", if new_state { "on" } else { "off" });
        }
        Err(e) => println!("NFC522:ERR {e}"),
    }
}

/// A blank password field in an incoming `SET` means "leave it as-is" rather
/// than "clear it" — the host never has the real password to send back
/// (see [`redact`]), so a naive full overwrite would wipe it every time the
/// page is used to touch an already-provisioned device without retyping
/// secrets it was never shown.
fn merge_passwords(incoming: &mut Config, existing: &Config) {
    if incoming.wifi_pass.is_empty() {
        incoming.wifi_pass = existing.wifi_pass.clone();
    }
    if incoming.mqtt_password.as_deref().unwrap_or("").is_empty() {
        incoming.mqtt_password = existing.mqtt_password.clone();
    }
}

/// Config with secrets stripped, safe to send to the host in response to
/// `GET` — used by the provisioning page to show "this device is already
/// configured" along with its non-secret settings and firmware version.
fn redact(cfg: &Config) -> serde_json::Value {
    serde_json::json!({
        "provisioned": cfg.is_provisioned(),
        "fw_version": FW_VERSION,
        "wifi_ssid": cfg.wifi_ssid,
        "mqtt_host": cfg.mqtt_host,
        "mqtt_port": cfg.mqtt_port,
        "mqtt_username": cfg.mqtt_username,
        "mqtt_use_tls": cfg.mqtt_use_tls,
        "topic_root": cfg.topic_root,
    })
}
