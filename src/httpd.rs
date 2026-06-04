//! Normal-operation HTTP status server.
//!
//! Once WiFi is up the device serves a small status page (and a JSON API) over
//! its station IP showing the most recent card reads. It reuses the same
//! Tailwind/DaisyUI shell, CSS, and fonts as the provisioning portal. This is
//! observability only — it is read-only and accepts no config.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use esp_idf_svc::http::server::{Configuration, EspHttpServer};
use esp_idf_svc::http::Method;
use esp_idf_svc::io::Write as _;
use serde::Serialize;

use crate::nfc::ScanEvent;

/// How many recent scans to retain and expose.
pub const MAX_SCANS: usize = 7;

// Pre-built, pre-compressed assets emitted by build.rs into OUT_DIR, shared with
// the provisioning portal (served with `Content-Encoding: gzip`).
const STATUS_HTML_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/status.html.gz"));
const APP_CSS_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/app.css.gz"));
const FONT_400: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/font-400.woff2"));
const FONT_700: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/font-700.woff2"));

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
const H_JSON: &[(&str, &str)] = &[("Content-Type", "application/json")];

/// Shared, bounded log of recent scans. Written by the scan loop, read by the
/// HTTP handlers (which run on the server's own task), hence `Arc<Mutex<_>>`.
pub type ScanLog = Arc<Mutex<VecDeque<ScanEvent>>>;

/// Create an empty scan log.
pub fn new_log() -> ScanLog {
    Arc::new(Mutex::new(VecDeque::with_capacity(MAX_SCANS)))
}

/// Record a scan, evicting the oldest once [`MAX_SCANS`] is reached.
pub fn record(log: &ScanLog, event: ScanEvent) {
    let mut q = log.lock().unwrap_or_else(|e| e.into_inner());
    if q.len() == MAX_SCANS {
        q.pop_front();
    }
    q.push_back(event);
}

/// Read-only device facts shown on the status page.
pub struct Info {
    pub id: String,
    pub mqtt_host: String,
    pub topic_root: String,
}

#[derive(Serialize)]
struct ScansBody<'a> {
    id: &'a str,
    mqtt_host: &'a str,
    topic_root: &'a str,
    /// Most-recent first.
    scans: Vec<ScanEvent>,
}

/// Start the status server. The returned handle must be kept alive for the
/// handlers to keep running.
pub fn start(log: ScanLog, info: Info) -> anyhow::Result<EspHttpServer<'static>> {
    let mut server = EspHttpServer::new(&Configuration::default())?;

    // Status page + shared assets (same shell as the provisioning portal).
    server.fn_handler("/", Method::Get, |req| {
        req.into_response(200, Some("OK"), H_HTML_GZ)?
            .write_all(STATUS_HTML_GZ)?;
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

    server.fn_handler("/api/scans", Method::Get, move |req| {
        let scans: Vec<ScanEvent> = {
            let q = log.lock().unwrap_or_else(|e| e.into_inner());
            q.iter().rev().cloned().collect() // most-recent first
        };
        let body = serde_json::to_vec(&ScansBody {
            id: &info.id,
            mqtt_host: &info.mqtt_host,
            topic_root: &info.topic_root,
            scans,
        })?;
        req.into_response(200, Some("OK"), H_JSON)?
            .write_all(&body)?;
        Ok::<(), anyhow::Error>(())
    })?;

    Ok(server)
}
