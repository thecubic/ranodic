#[cfg(feature = "defmt")]
use defmt::{debug, error, info};

use embassy_net::{Runner, Stack};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{Duration, Timer};

use esp_radio::wifi::{ClientConfig, WifiController, WifiDevice, WifiEvent, WifiStaState};
use nanofish::HttpClient;

pub const SSID: &str = env!("WIFI_SSID");
pub const PASSWORD: &str = env!("WIFI_PASSWORD");

pub const DEFAULT_SSID: &str = "fbi";
pub const DEFAULT_PASSWORD: &str = "flowers by irene";

#[cfg(not(feature = "esp32"))]
pub type WorkingClient<'a> = HttpClient<'a, 2048, 2048, 16640, 2048, 2048>;

#[cfg(feature = "esp32")]
pub type WorkingClient<'a> = HttpClient<'a, 2048, 2048, 4096, 2048, 2048>;

pub static NET_REQUEST_QUEUE: Mutex<CriticalSectionRawMutex, ()> = Mutex::new(());

#[embassy_executor::task]
pub async fn conn_watchdog(mut controller: WifiController<'static>) {
    debug!("start connection task");
    // this always fucking dies
    // debug!("Device capabilities: {:?}", controller.capabilities());
    loop {
        if esp_radio::wifi::sta_state() == WifiStaState::Connected {
            controller.wait_for_event(WifiEvent::StaDisconnected).await;
            Timer::after(Duration::from_millis(5000)).await;
        }
        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = esp_radio::wifi::ModeConfig::Client(
                ClientConfig::default()
                    .with_ssid(SSID.into())
                    .with_password(PASSWORD.into()),
            );
            controller.set_config(&client_config).unwrap();
            debug!("Starting wifi");
            controller.start_async().await.unwrap();
            info!("Wifi started");
        }
        debug!("About to connect...");
        match controller.connect_async().await {
            Ok(_) => info!("Wifi connected"),
            Err(e) => {
                error!("Failed to connect to wifi: {:?}", e);
                crate::graceful_sever_divine_light().await;
                Timer::after(Duration::from_millis(5000)).await
            }
        }
    }
}

#[embassy_executor::task]
pub async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await
}

pub async fn net_up(stack: Stack<'static>) {
    'link_up: loop {
        if stack.is_link_up() {
            break 'link_up;
        }
        Timer::after_secs(2).await;
        info!("awaiting link up");
    }
    info!("link up");

    'addr_up: loop {
        if let Some(config) = stack.config_v4() {
            info!(
                "got cfg: {:?} to {:?} with DNS {:?}",
                config.address, config.gateway, config.dns_servers
            );
            break 'addr_up;
        }
        Timer::after_secs(2).await;
        info!("awaiting addr up");
    }
    info!("addr up");
}

pub const NUM_SOCKS: usize = 8;
