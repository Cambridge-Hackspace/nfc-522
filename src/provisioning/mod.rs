//! Provisioning mode: SoftAP + captive DNS + HTTP config form.
//!
//! Blocks until the user submits a config, then reboots into normal operation.

mod dns;
mod http;
mod serial;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::Result;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::hal::reset;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use crate::device_id::setup_ssid;
use crate::wifi;

/// SoftAP gateway IP that the captive DNS advertises and the portal lives on.
const AP_IP: [u8; 4] = [192, 168, 71, 1];

/// Run provisioning forever; reboots the device once a config is saved.
pub fn run(
    modem: Modem<'static>,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    id: &str,
    led: AnyIOPin<'static>,
) -> Result<()> {
    let ssid = setup_ssid(id);
    // Keep the AP handle alive for the duration of provisioning.
    let _ap = wifi::start_ap(modem, sysloop, nvs.clone(), &ssid)?;

    std::thread::Builder::new()
        .stack_size(4096)
        .spawn(move || dns::serve(AP_IP))?;

    let saved = Arc::new(AtomicBool::new(false));
    // Keep the server alive; dropping it would unregister the handlers.
    let _server = http::start(nvs.clone(), saved.clone())?;

    // Alternative to the AP + HTTP portal: configure directly over the same
    // USB serial connection used for flashing (see provisioning page).
    serial::run(nvs, saved.clone(), id.to_string(), led)?;

    log::info!("provisioning ready: join '{ssid}', config opens automatically");

    loop {
        if saved.load(Ordering::SeqCst) {
            log::info!("config saved; rebooting into normal operation");
            // Give the HTTP response time to flush before resetting.
            FreeRtos::delay_ms(1500);
            reset::restart();
        }
        FreeRtos::delay_ms(200);
    }
}
