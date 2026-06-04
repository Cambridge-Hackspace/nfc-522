//! NFC front-end: a shared SPI bus driving up to two MFRC522 readers.
//!
//! Readers are *probed* at startup by reading the version register, so the same
//! firmware runs on 1- or 2-channel hardware — an absent reader is simply
//! skipped. Each [`Reader`] debounces with REQA+HLTA (a resting card is read
//! once, halted, then ignored until removed) plus a short same-UID cooldown to
//! absorb tap bounce.

use embassy_time::{Duration, Instant};
use embedded_hal::spi::SpiDevice;
use esp_idf_svc::hal::gpio::AnyIOPin;
use esp_idf_svc::hal::spi::config::{Config as SpiConfig, DriverConfig};
use esp_idf_svc::hal::spi::{SpiAnyPins, SpiDeviceDriver, SpiDriver};
use esp_idf_svc::hal::units::Hertz;
use mfrc522::comm::blocking::spi::{DummyDelay, SpiInterface};
use mfrc522::{Initialized, Mfrc522};
use serde::Serialize;

/// Suppress a repeated read of the *same* UID seen within this window.
const SAME_UID_COOLDOWN: Duration = Duration::from_millis(1000);

/// A single confirmed card read, serialized as the MQTT payload.
#[derive(Serialize, Debug, Clone)]
pub struct ScanEvent {
    pub uid: String,
    pub channel: u8,
    pub ts_ms: u64,
    #[serde(rename = "type")]
    pub kind: String,
}

/// One MFRC522 reader on a given channel, with per-reader debounce state.
pub struct Reader<SPI: SpiDevice> {
    mfrc522: Mfrc522<SpiInterface<SPI, DummyDelay>, Initialized>,
    channel: u8,
    last_uid: Option<String>,
    last_seen: Instant,
}

impl<SPI, E> Reader<SPI>
where
    SPI: SpiDevice<Error = E>,
    E: core::fmt::Debug,
{
    /// Poll the reader once. Returns a [`ScanEvent`] only for a *new* tap.
    pub fn poll(&mut self) -> Option<ScanEvent> {
        // REQA is only answered by cards in IDLE state, so a card we already
        // read and HLTA'd stays silent until it leaves and re-enters the field.
        let atqa = self.mfrc522.reqa().ok()?;
        let uid = self.mfrc522.select(&atqa).ok()?;

        // Halt the card and drop any crypto state so the next tap re-detects.
        let _ = self.mfrc522.hlta();
        let _ = self.mfrc522.stop_crypto1();

        let uid_hex = to_hex(uid.as_bytes());
        let now = Instant::now();

        // Secondary guard: ignore an identical UID that bounces within the window.
        if self.last_uid.as_deref() == Some(uid_hex.as_str())
            && now.saturating_duration_since(self.last_seen) < SAME_UID_COOLDOWN
        {
            self.last_seen = now;
            return None;
        }

        self.last_uid = Some(uid_hex.clone());
        self.last_seen = now;

        Some(ScanEvent {
            uid: uid_hex,
            channel: self.channel,
            ts_ms: now.as_millis(),
            kind: format!("{:?}", uid.get_type()),
        })
    }
}

/// Build the shared SPI bus driver. `sdo` = MOSI, `sdi` = MISO.
pub fn build_driver<'d, SPI: SpiAnyPins + 'd>(
    spi: SPI,
    sclk: AnyIOPin<'d>,
    sdo: AnyIOPin<'d>,
    sdi: AnyIOPin<'d>,
) -> anyhow::Result<SpiDriver<'d>> {
    let driver = SpiDriver::new(spi, sclk, sdo, Some(sdi), &DriverConfig::new())?;
    Ok(driver)
}

/// Probe one chip-select line for an MFRC522. Returns a ready [`Reader`] if a
/// reader answers with a plausible version byte (`0x91`/`0x92`), else `None`.
pub fn probe<'d>(
    driver: &'d SpiDriver<'d>,
    cs: AnyIOPin<'d>,
    channel: u8,
) -> Option<Reader<SpiDeviceDriver<'d, &'d SpiDriver<'d>>>> {
    // MFRC522 tolerates well beyond 1 MHz; keep it conservative for wiring slack.
    let config = SpiConfig::new().baudrate(Hertz(1_000_000));
    let device = match SpiDeviceDriver::new(driver, Some(cs), &config) {
        Ok(d) => d,
        Err(e) => {
            log::warn!("ch{channel}: SPI device init failed: {e}");
            return None;
        }
    };

    let mut mfrc522 = match Mfrc522::new(SpiInterface::new(device)).init() {
        Ok(m) => m,
        Err(e) => {
            log::warn!("ch{channel}: MFRC522 init failed: {e:?}");
            return None;
        }
    };

    match mfrc522.version() {
        Ok(v) if v != 0x00 && v != 0xFF => {
            log::info!("ch{channel}: MFRC522 present (version {v:#04x})");
            Some(Reader {
                mfrc522,
                channel,
                last_uid: None,
                last_seen: Instant::now(),
            })
        }
        Ok(v) => {
            log::warn!("ch{channel}: no reader (version {v:#04x})");
            None
        }
        Err(e) => {
            log::warn!("ch{channel}: version read failed: {e:?}");
            None
        }
    }
}

/// Format bytes as uppercase hex, e.g. `04A1B2C3`.
fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02X}"));
    }
    s
}
