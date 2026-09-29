//! Normal operation: connect WiFi + MQTT, then publish scanned UIDs.
//!
//! Two cooperating futures run under the single `block_on` executor:
//! a connection pump (required for the MQTT client to make progress) and the
//! scan loop (polls the readers and publishes). They share a connected flag so
//! the scan loop can (re)publish the retained `online` status on each connect.

use std::cell::Cell;

use anyhow::Result;
use embassy_futures::select::select;
use embassy_time::{Duration, Instant, Timer};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::gpio::PinDriver;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::hal::spi::SpiAnyPins;
use esp_idf_svc::mqtt::client::{EventPayload, QoS};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::timer::EspTaskTimerService;

use crate::config::Config;
use crate::{httpd, mqtt, nfc, wifi};

/// Delay between scan sweeps across all readers.
const POLL_INTERVAL: Duration = Duration::from_millis(75);

/// How often to (re)publish the station IP to `<root>/<id>/ip` while connected.
const IP_PUBLISH_INTERVAL: Duration = Duration::from_secs(60);

/// Pins for the shared SPI bus and the two reader chip-selects.
pub struct NfcPins {
    pub sclk: AnyIOPin<'static>,
    pub mosi: AnyIOPin<'static>,
    pub miso: AnyIOPin<'static>,
    pub cs0: AnyIOPin<'static>,
    pub cs1: AnyIOPin<'static>,
}

/// Connect and run until reset. Returns `Err` only on bring-up failure (the
/// caller decides whether to fall back to provisioning or reboot).
#[allow(clippy::too_many_arguments)]
pub async fn run<SPI: SpiAnyPins + 'static>(
    modem: Modem<'static>,
    sysloop: EspSystemEventLoop,
    timer: EspTaskTimerService,
    nvs: EspDefaultNvsPartition,
    cfg: Config,
    id: String,
    spi: SPI,
    pins: NfcPins,
    led_builtin: AnyIOPin<'static>,
) -> Result<()> {
    // Turn on the builtin led
    let mut led_pin = PinDriver::output(led_builtin)?;
    led_pin.set_low()?;

    // 1. WiFi (kept alive for the whole session).
    let _wifi = wifi::connect_sta(
        modem,
        sysloop,
        timer,
        nvs.clone(),
        &cfg.wifi_ssid,
        &cfg.wifi_pass,
    )
    .await?;

    // WiFi is up: clear the consecutive-failure counter so a future bad reboot
    // starts counting from zero. (Errors past this point reboot but don't count
    // as WiFi failures, so MQTT/NFC issues won't trip the provisioning fallback.)
    crate::config::clear_boot_fail_count(&nvs);

    // Local read-only status server: last `MAX_SCANS` reads over the station IP,
    // plus the (read-only) broker host and topic root.
    let scan_log = httpd::new_log();
    let _httpd = httpd::start(
        scan_log.clone(),
        httpd::Info {
            id: id.clone(),
            mqtt_host: cfg.mqtt_host.clone(),
            topic_root: cfg.topic_root.clone(),
        },
    )?;
    if let Ok(ip) = _wifi.wifi().sta_netif().get_ip_info() {
        log::info!("status page: http://{}/ (json: /api/scans)", ip.ip);
    }

    // 2. MQTT client + its event connection.
    let (mut client, mut conn) = mqtt::connect(&cfg, &id)?;

    // 3. NFC bus + up to two readers.
    let driver = nfc::build_driver(spi, pins.sclk, pins.mosi, pins.miso)?;
    let mut readers = Vec::new();
    if let Some(r) = nfc::probe(&driver, pins.cs0, 0) {
        readers.push(r);
    }
    if let Some(r) = nfc::probe(&driver, pins.cs1, 1) {
        readers.push(r);
    }
    if readers.is_empty() {
        log::warn!("no MFRC522 readers detected; check wiring/CS pins");
    } else {
        log::info!("{} reader(s) active", readers.len());
    }

    let status_topic = cfg.status_topic(&id);
    let ip_topic = cfg.ip_topic(&id);
    let connected = Cell::new(false);

    // Pump connection events; the client only progresses while this runs.
    let pump = async {
        loop {
            match conn.next().await {
                Ok(event) => match event.payload() {
                    EventPayload::Connected(_) => {
                        log::info!("MQTT connected");
                        connected.set(true);
                    }
                    EventPayload::Disconnected => {
                        log::warn!("MQTT disconnected");
                        connected.set(false);
                    }
                    _ => {}
                },
                Err(e) => log::warn!("MQTT event error: {e}"),
            }
        }
    };

    // Poll readers and publish scans; (re)publish online + IP on each connect.
    let scan = async {
        let mut was_connected = false;
        let mut last_ip_publish: Option<Instant> = None;
        loop {
            let now_connected = connected.get();
            let just_connected = now_connected && !was_connected;
            if just_connected {
                if let Err(e) = client
                    .publish(&status_topic, QoS::AtLeastOnce, true, b"online")
                    .await
                {
                    log::warn!("publish online failed: {e}");
                } else {
                    log::info!("published online -> {status_topic}");
                }
            }
            was_connected = now_connected;

            // Publish the station IP on (re)connect, then refresh it periodically
            // (retained, so a late subscriber still learns where to reach us).
            if now_connected {
                let due = last_ip_publish.is_none_or(|t| {
                    Instant::now().saturating_duration_since(t) >= IP_PUBLISH_INTERVAL
                });
                if just_connected || due {
                    match _wifi.wifi().sta_netif().get_ip_info() {
                        Ok(info) => {
                            let ip = info.ip.to_string();
                            match client
                                .publish(&ip_topic, QoS::AtLeastOnce, true, ip.as_bytes())
                                .await
                            {
                                Ok(_) => {
                                    log::info!("published ip {ip} -> {ip_topic}");
                                    last_ip_publish = Some(Instant::now());
                                }
                                Err(e) => log::warn!("publish ip failed: {e}"),
                            }
                        }
                        Err(e) => log::warn!("read ip failed: {e}"),
                    }
                }
            } else {
                // Force a fresh publish the moment we reconnect.
                last_ip_publish = None;
            }

            for reader in readers.iter_mut() {
                if let Some(event) = reader.poll() {
                    let _ = led_pin.set_low();
                    let topic = cfg.scan_topic(&id, event.channel);
                    match serde_json::to_vec(&event) {
                        Ok(payload) => {
                            match client
                                .publish(&topic, QoS::AtMostOnce, false, &payload)
                                .await
                            {
                                Ok(_) => {
                                    log::info!("ch{} scan {} -> {topic}", event.channel, event.uid)
                                }
                                Err(e) => log::warn!("publish scan failed: {e}"),
                            }
                        }
                        Err(e) => log::warn!("encode scan failed: {e}"),
                    }
                    // Record after publishing (publish only borrows `event`).
                    httpd::record(&scan_log, event);
                }
            }

            Timer::after(POLL_INTERVAL).await;
            let _ = led_pin.set_high();
        }
    };

    // Neither future returns; run them until the device resets.
    select(pump, scan).await;
    Ok(())
}
