//! Persistent device configuration, stored as a single JSON blob in NVS.

use anyhow::Result;
use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs};
use serde::{Deserialize, Serialize};

/// NVS namespace and key under which the JSON config blob lives.
const NVS_NAMESPACE: &str = "cfg";
const NVS_KEY: &str = "config";
/// Key (same namespace) for the consecutive-WiFi-failure counter.
const NVS_BOOTFAIL_KEY: &str = "bootfail";
/// Upper bound for the serialized config; comfortably larger than any real config.
const BLOB_CAP: usize = 1024;

/// All user-provisioned settings. Missing fields fall back to [`Default`], so
/// adding new fields stays backward-compatible with older stored blobs.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    #[serde(default)]
    pub wifi_ssid: String,
    #[serde(default)]
    pub wifi_pass: String,
    #[serde(default)]
    pub mqtt_host: String,
    #[serde(default = "default_mqtt_port")]
    pub mqtt_port: u16,
    #[serde(default)]
    pub mqtt_username: Option<String>,
    #[serde(default)]
    pub mqtt_password: Option<String>,
    #[serde(default)]
    pub mqtt_use_tls: bool,
    #[serde(default = "default_topic_root")]
    pub topic_root: String,
}

fn default_mqtt_port() -> u16 {
    1883
}

fn default_topic_root() -> String {
    "neiam".to_string()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            wifi_ssid: String::new(),
            wifi_pass: String::new(),
            mqtt_host: String::new(),
            mqtt_port: default_mqtt_port(),
            mqtt_username: None,
            mqtt_password: None,
            mqtt_use_tls: false,
            topic_root: default_topic_root(),
        }
    }
}

impl Config {
    /// A config is usable for normal operation once it has both WiFi and a broker.
    pub fn is_provisioned(&self) -> bool {
        !self.wifi_ssid.is_empty() && !self.mqtt_host.is_empty()
    }

    /// Topic for scans on a given channel: `<root>/nfc/<id>/scans/<channel>`.
    pub fn scan_topic(&self, id: &str, channel: u8) -> String {
        format!("{}/nfc/{}/scans/{}", self.topic_root, id, channel)
    }

    /// Retained availability topic: `<root>/nfc/<id>/status`.
    pub fn status_topic(&self, id: &str) -> String {
        format!("{}/nfc/{}/status", self.topic_root, id)
    }

    /// Retained station-IP topic: `<root>/nfc/<id>/ip`.
    pub fn ip_topic(&self, id: &str) -> String {
        format!("{}/nfc/{}/ip", self.topic_root, id)
    }
}

/// Load the config from NVS, returning [`Config::default`] if absent or unreadable.
pub fn load(nvs: &EspDefaultNvsPartition) -> Config {
    let store = match EspNvs::new(nvs.clone(), NVS_NAMESPACE, true) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("nvs open failed ({e}); using default config");
            return Config::default();
        }
    };

    let mut buf = [0u8; BLOB_CAP];
    match store.get_blob(NVS_KEY, &mut buf) {
        Ok(Some(bytes)) => serde_json::from_slice(bytes).unwrap_or_else(|e| {
            log::warn!("config parse failed ({e}); using default config");
            Config::default()
        }),
        Ok(None) => Config::default(),
        Err(e) => {
            log::warn!("config read failed ({e}); using default config");
            Config::default()
        }
    }
}

/// Persist the config to NVS as a JSON blob.
pub fn save(nvs: &EspDefaultNvsPartition, config: &Config) -> Result<()> {
    let store = EspNvs::new(nvs.clone(), NVS_NAMESPACE, true)?;
    let json = serde_json::to_vec(config)?;
    store.set_blob(NVS_KEY, &json)?;
    Ok(())
}

/// Consecutive failed WiFi bring-ups recorded since the last success (0 if unset
/// or unreadable). Used by `main` to fall back to provisioning after repeated
/// failures (e.g. a wrong WiFi password).
pub fn boot_fail_count(nvs: &EspDefaultNvsPartition) -> u8 {
    match EspNvs::new(nvs.clone(), NVS_NAMESPACE, true) {
        Ok(store) => store.get_u8(NVS_BOOTFAIL_KEY).unwrap_or(None).unwrap_or(0),
        Err(e) => {
            log::warn!("nvs open failed ({e}); assuming 0 boot failures");
            0
        }
    }
}

/// Persist the consecutive-failure counter. Best-effort: a failure to write is
/// logged but not fatal (worst case the fallback is delayed by a boot).
pub fn set_boot_fail_count(nvs: &EspDefaultNvsPartition, n: u8) {
    match EspNvs::new(nvs.clone(), NVS_NAMESPACE, true) {
        Ok(store) => {
            if let Err(e) = store.set_u8(NVS_BOOTFAIL_KEY, n) {
                log::warn!("failed to persist boot-fail counter ({e})");
            }
        }
        Err(e) => log::warn!("nvs open failed ({e}); boot-fail counter not saved"),
    }
}

/// Reset the consecutive-failure counter (call once WiFi is up).
pub fn clear_boot_fail_count(nvs: &EspDefaultNvsPartition) {
    set_boot_fail_count(nvs, 0);
}
