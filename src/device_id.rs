//! Device identity derived from the factory MAC address.
//!
//! The id is the lowercase hex of the last 3 bytes of the WiFi-STA MAC (6 chars,
//! e.g. `0a1b2c`). It is used both for the setup SoftAP SSID and as the middle
//! segment of every MQTT topic, so each unit is self-identifying.

use esp_idf_svc::sys::{esp_mac_type_t_ESP_MAC_WIFI_STA, esp_read_mac};

/// Returns the 6-character lowercase-hex device id derived from the MAC.
pub fn device_id() -> String {
    let mut mac = [0u8; 6];
    // SAFETY: `esp_read_mac` writes exactly 6 bytes into the provided buffer.
    unsafe {
        esp_read_mac(mac.as_mut_ptr(), esp_mac_type_t_ESP_MAC_WIFI_STA);
    }
    format!("{:02x}{:02x}{:02x}", mac[3], mac[4], mac[5])
}

/// SSID advertised while in provisioning mode, e.g. `scan-setup-0a1b2c`.
pub fn setup_ssid(id: &str) -> String {
    format!("scan-setup-{id}")
}
