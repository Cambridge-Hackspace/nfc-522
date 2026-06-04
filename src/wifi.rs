//! WiFi bring-up: async STA connect for normal operation, blocking SoftAP for
//! provisioning. Each helper takes the modem peripheral (consumed once) and
//! returns the live wifi handle, which the caller must keep alive.

use anyhow::Result;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::timer::EspTaskTimerService;
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AsyncWifi, AuthMethod, BlockingWifi, ClientConfiguration,
    Configuration, EspWifi,
};

/// Connect to an access point as a station and wait for an IP address.
pub async fn connect_sta(
    modem: Modem<'static>,
    sysloop: EspSystemEventLoop,
    timer: EspTaskTimerService,
    nvs: EspDefaultNvsPartition,
    ssid: &str,
    pass: &str,
) -> Result<AsyncWifi<EspWifi<'static>>> {
    let mut wifi = AsyncWifi::wrap(
        EspWifi::new(modem, sysloop.clone(), Some(nvs))?,
        sysloop,
        timer,
    )?;

    let auth_method = if pass.is_empty() {
        AuthMethod::None
    } else {
        AuthMethod::WPA2Personal
    };

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: ssid
            .try_into()
            .map_err(|_| anyhow::anyhow!("ssid too long"))?,
        password: pass
            .try_into()
            .map_err(|_| anyhow::anyhow!("password too long"))?,
        auth_method,
        ..Default::default()
    }))?;

    wifi.start().await?;
    wifi.connect().await?;
    wifi.wait_netif_up().await?;

    let ip = wifi.wifi().sta_netif().get_ip_info()?;
    log::info!("WiFi connected to '{ssid}', ip = {}", ip.ip);
    Ok(wifi)
}

/// Start an open SoftAP for provisioning. The default ESP-IDF AP gateway is
/// `192.168.71.1`, which the captive portal advertises.
pub fn start_ap(
    modem: Modem<'static>,
    sysloop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    ssid: &str,
) -> Result<BlockingWifi<EspWifi<'static>>> {
    let mut wifi = BlockingWifi::wrap(EspWifi::new(modem, sysloop.clone(), Some(nvs))?, sysloop)?;

    wifi.set_configuration(&Configuration::AccessPoint(AccessPointConfiguration {
        ssid: ssid
            .try_into()
            .map_err(|_| anyhow::anyhow!("ssid too long"))?,
        auth_method: AuthMethod::None,
        channel: 1,
        max_connections: 4,
        ..Default::default()
    }))?;

    wifi.start()?;
    log::info!("SoftAP '{ssid}' started at http://192.168.71.1/");
    Ok(wifi)
}
