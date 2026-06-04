//! MQTT client setup. Uses the async client so publishing and the event pump
//! cooperate on the single `block_on` executor. A retained Last-Will message
//! marks the device offline if it drops; the app publishes `online` on connect.

use anyhow::Result;
use esp_idf_svc::mqtt::client::{
    EspAsyncMqttClient, EspAsyncMqttConnection, LwtConfiguration, MqttClientConfiguration, QoS,
};

use crate::config::Config;

/// Build and start the async MQTT client for the given config and device id.
///
/// Returns the client (for publishing) and its connection (which must be
/// pumped via [`EspAsyncMqttConnection::next`] for the client to make progress).
pub fn connect(cfg: &Config, id: &str) -> Result<(EspAsyncMqttClient, EspAsyncMqttConnection)> {
    let scheme = if cfg.mqtt_use_tls { "mqtts" } else { "mqtt" };
    let url = format!("{scheme}://{}:{}", cfg.mqtt_host, cfg.mqtt_port);
    let status_topic = cfg.status_topic(id);

    let mqtt_cfg = MqttClientConfiguration {
        client_id: Some(id),
        lwt: Some(LwtConfiguration {
            topic: &status_topic,
            payload: b"offline",
            qos: QoS::AtLeastOnce,
            retain: true,
        }),
        username: cfg.mqtt_username.as_deref(),
        password: cfg.mqtt_password.as_deref(),
        // Use the bundled CA store for TLS connections (mqtts).
        crt_bundle_attach: if cfg.mqtt_use_tls {
            Some(esp_idf_svc::sys::esp_crt_bundle_attach)
        } else {
            None
        },
        ..Default::default()
    };

    log::info!("connecting to MQTT broker {url}");
    let (client, conn) = EspAsyncMqttClient::new(&url, &mqtt_cfg)?;
    Ok((client, conn))
}
