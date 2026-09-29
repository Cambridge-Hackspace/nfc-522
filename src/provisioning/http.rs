//! Provisioning HTTP server: serves the config form, accepts the saved config,
//! and redirects OS captive-portal probes to the form so it auto-opens.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use esp_idf_svc::http::server::{Configuration, EspHttpServer};
use esp_idf_svc::http::Method;
use esp_idf_svc::io::Write as _;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use crate::config::{self, Config};

// Pre-built, pre-compressed assets emitted by build.rs into OUT_DIR. They are
// served with `Content-Encoding: gzip` (the woff2 fonts are already compressed).
const PORTAL_HTML_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/portal.html.gz"));
const SAVED_HTML_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/saved.html.gz"));
const APP_CSS_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app.css.gz"));
const FONT_400: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/font-400.woff2"));
const FONT_700: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/font-700.woff2"));

const GATEWAY: &str = "http://192.168.71.1/";

// Response header sets.
const H_HTML_GZ: &[(&str, &str)] = &[
    ("Content-Type", "text/html; charset=utf-8"),
    ("Content-Encoding", "gzip"),
];
const H_CSS_GZ: &[(&str, &str)] = &[
    ("Content-Type", "text/css"),
    ("Content-Encoding", "gzip"),
    ("Cache-Control", "max-age=86400"),
];
const H_WOFF2: &[(&str, &str)] = &[
    ("Content-Type", "font/woff2"),
    ("Cache-Control", "max-age=604800"),
];

/// OS connectivity-check URLs; redirecting these to the portal pops the
/// "sign in to network" browser on Android/iOS/Windows.
const PROBES: &[&str] = &[
    "/generate_204",
    "/gen_204",
    "/hotspot-detect.html",
    "/library/test/success.html",
    "/ncsi.txt",
    "/connecttest.txt",
    "/canonical.html",
    "/success.txt",
];

/// Start the provisioning HTTP server. `saved` is flipped to `true` once the
/// user submits a valid config (the caller then reboots). The returned server
/// must be kept alive for handlers to run.
pub fn start(
    nvs: EspDefaultNvsPartition,
    saved: Arc<AtomicBool>,
) -> anyhow::Result<EspHttpServer<'static>> {
    let mut server = EspHttpServer::new(&Configuration::default())?;

    // Static, pre-compressed assets.
    server.fn_handler("/", Method::Get, |req| {
        req.into_response(200, Some("OK"), H_HTML_GZ)?
            .write_all(PORTAL_HTML_GZ)?;
        Ok::<(), anyhow::Error>(())
    })?;
    server.fn_handler("/app.css", Method::Get, |req| {
        req.into_response(200, Some("OK"), H_CSS_GZ)?
            .write_all(APP_CSS_GZ)?;
        Ok::<(), anyhow::Error>(())
    })?;
    server.fn_handler("/font-400.woff2", Method::Get, |req| {
        req.into_response(200, Some("OK"), H_WOFF2)?
            .write_all(FONT_400)?;
        Ok::<(), anyhow::Error>(())
    })?;
    server.fn_handler("/font-700.woff2", Method::Get, |req| {
        req.into_response(200, Some("OK"), H_WOFF2)?
            .write_all(FONT_700)?;
        Ok::<(), anyhow::Error>(())
    })?;

    server.fn_handler("/save", Method::Post, move |mut req| {
        let mut body = Vec::new();
        let mut buf = [0u8; 512];
        loop {
            let n = req.read(&mut buf)?;
            if n == 0 || body.len() > 8192 {
                break;
            }
            body.extend_from_slice(&buf[..n]);
        }

        let cfg = parse_config(&String::from_utf8_lossy(&body));
        config::save(&nvs, &cfg)?;
        log::info!(
            "provisioning saved: wifi='{}' broker='{}:{}' root='{}'",
            cfg.wifi_ssid,
            cfg.mqtt_host,
            cfg.mqtt_port,
            cfg.topic_root
        );

        req.into_response(200, Some("OK"), H_HTML_GZ)?
            .write_all(SAVED_HTML_GZ)?;
        saved.store(true, Ordering::SeqCst);
        Ok::<(), anyhow::Error>(())
    })?;

    for probe in PROBES {
        server.fn_handler(probe, Method::Get, |req| {
            req.into_response(302, Some("Found"), &[("Location", GATEWAY)])?;
            Ok::<(), anyhow::Error>(())
        })?;
    }

    Ok(server)
}

/// Parse a `application/x-www-form-urlencoded` body into a [`Config`].
fn parse_config(body: &str) -> Config {
    let mut cfg = Config::default();
    let mut tls_seen = false;

    for pair in body.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let value = url_decode(value);
        match url_decode(key).as_str() {
            "wifi_ssid" => cfg.wifi_ssid = value,
            "wifi_pass" => cfg.wifi_pass = value,
            "mqtt_host" => cfg.mqtt_host = value,
            "mqtt_port" => {
                if let Ok(p) = value.trim().parse::<u16>() {
                    cfg.mqtt_port = p;
                }
            }
            "mqtt_username" => cfg.mqtt_username = non_empty(value),
            "mqtt_password" => cfg.mqtt_password = non_empty(value),
            "mqtt_use_tls" => {
                tls_seen = true;
                cfg.mqtt_use_tls = matches!(value.as_str(), "on" | "true" | "1");
            }
            "topic_root" if !value.trim().is_empty() => {
                cfg.topic_root = value.trim().to_string();
            }
            _ => {}
        }
    }

    // Unchecked checkboxes are simply omitted from the form body.
    if !tls_seen {
        cfg.mqtt_use_tls = false;
    }
    cfg
}

fn non_empty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Decode a urlencoded component (`+` -> space, `%XX` -> byte).
fn url_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
