//! Up-to-2-channel MFRC522 NFC scanner for ESP32-C3.
//!
//! Boots into a captive-portal provisioning AP when unconfigured (or when the
//! BOOT button is held at startup); otherwise connects WiFi + MQTT and publishes
//! scanned card UIDs to `<root>/nfc/<id>/scans/<channel>`.

mod app;
mod config;
mod device_id;
mod httpd;
mod mqtt;
mod nfc;
mod provisioning;
mod wifi;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{PinDriver, Pull};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::hal::reset;
use esp_idf_svc::hal::task::block_on;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::timer::EspTaskTimerService;

/// How long BOOT must be held low at startup to force re-provisioning.
const REPROVISION_HOLD_MS: u32 = 3000;

/// After this many consecutive failed WiFi bring-ups, fall back to the
/// provisioning portal instead of rebooting again (recovers from bad credentials
/// without needing the BOOT button).
const MAX_BOOT_FAILS: u8 = 3;

fn main() -> anyhow::Result<()> {
    // Link runtime patches and bind `log` to the ESP logging facility.
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let sysloop = EspSystemEventLoop::take()?;
    let timer = EspTaskTimerService::new()?;
    let nvs = EspDefaultNvsPartition::take()?;
    let pins = peripherals.pins;

    let id = device_id::device_id();
    log::info!("neiam-nfc-522 booting, device id = {id}");

    let cfg = config::load(&nvs);
    let force_provision = boot_held(PinDriver::input(pins.gpio0, Pull::Up)?);

    // Enter provisioning if there's no usable config, the BOOT button is held, or
    // we've failed to bring up WiFi too many times in a row (bad-credential escape
    // hatch). `app::run` clears the failure counter once WiFi is actually up.
    let fail_count = config::boot_fail_count(&nvs);
    let fallback = cfg.is_provisioned() && fail_count >= MAX_BOOT_FAILS;

    if !cfg.is_provisioned() || force_provision || fallback {
        if force_provision {
            log::info!("BOOT held: entering provisioning");
        } else if fallback {
            log::warn!("{fail_count} consecutive WiFi failures: falling back to provisioning");
        } else {
            log::info!("no stored config: entering provisioning");
        }
        // Reset so the device boots normally again after new credentials are saved.
        config::clear_boot_fail_count(&nvs);
        provisioning::run(peripherals.modem, sysloop, nvs, &id)?;
        return Ok(()); // provisioning::run reboots; this is unreachable.
    }

    // Count this attempt before trying; `app::run` clears it the moment WiFi is up.
    // If bring-up hangs, crashes, or returns an error and we reboot, the increment
    // persists and eventually trips the fallback above.
    let attempt = fail_count.saturating_add(1);
    config::set_boot_fail_count(&nvs, attempt);
    log::info!("config present (WiFi attempt {attempt}/{MAX_BOOT_FAILS}): connecting WiFi + MQTT");

    // MFRC522 hardware reset (NRST, active-low) is wired to GPIO9; drive it high
    // to release the reader from power-down. Held high for the program lifetime
    // (the firmware otherwise relies on the MFRC522 soft-reset).
    let mut rst = PinDriver::output(pins.gpio9)?;
    rst.set_high()?;

    let nfc_pins = app::NfcPins {
        sclk: pins.gpio10.into(),
        mosi: pins.gpio5.into(),
        miso: pins.gpio7.into(),
        cs0: pins.gpio6.into(),
        // Second reader's chip-select (optional). The bus auto-probes and skips
        // it if no reader is attached; GPIO8 is left for the unused IRQ line.
        cs1: pins.gpio4.into(),
    };

    let result = block_on(app::run(
        peripherals.modem,
        sysloop,
        timer,
        nvs,
        cfg,
        id,
        peripherals.spi2,
        nfc_pins,
    ));

    if let Err(e) = result {
        log::error!("operation failed: {e:#}; rebooting in 5s");
        FreeRtos::delay_ms(5000);
        reset::restart();
    }
    Ok(())
}

/// Return `true` if the BOOT button is held low for [`REPROVISION_HOLD_MS`].
fn boot_held(boot: PinDriver<'_, esp_idf_svc::hal::gpio::Input>) -> bool {
    // Sample every 100 ms; abort early the moment it reads high (released).
    for _ in 0..(REPROVISION_HOLD_MS / 100) {
        if boot.is_high() {
            return false;
        }
        FreeRtos::delay_ms(100);
    }
    true
}
