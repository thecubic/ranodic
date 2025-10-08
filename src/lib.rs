#![no_std]

use core::net::{IpAddr, SocketAddr};

use embassy_net::{
    udp::{PacketMetadata, UdpSocket},
    Runner, Stack,
};
use embassy_time::{Duration, Instant, Timer};
use esp_hal::{rtc_cntl::Rtc, Async};
use esp_hal_smartled::SmartLedsAdapterAsync;
use esp_hub75::{
    framebuffer::{compute_frame_count, compute_rows, plain::DmaFrameBuffer},
    Color, Hub75,
};
use esp_wifi::wifi::{
    ClientConfiguration, Configuration, WifiController, WifiDevice, WifiEvent, WifiState,
};

#[cfg(feature = "defmt")]
use defmt::{debug, error, info};

use smart_leds::{
    brightness, gamma,
    hsv::{hsv2rgb, Hsv},
    SmartLedsWriteAsync, RGB8,
};

extern crate alloc;
use alloc::string::ToString;
use embedded_graphics::{
    geometry::Point,
    mono_font::{ascii::FONT_5X7, MonoTextStyleBuilder},
    pixelcolor::{Rgb565, Rgb888},
    prelude::RgbColor,
    text::{Alignment, Text},
    Drawable,
};
use smoltcp::wire::DnsQueryType;
use sntpc::{get_time, NtpContext, NtpTimestampGenerator};

const ROWS: usize = 32;
const COLS: usize = 64;
const BITS: u8 = 4;
const NROWS: usize = compute_rows(ROWS);
const FRAME_COUNT: usize = compute_frame_count(BITS);
pub type FBType = DmaFrameBuffer<ROWS, COLS, NROWS, BITS, FRAME_COUNT>;

const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");

#[embassy_executor::task]
pub async fn conn_watchdog(mut controller: WifiController<'static>) {
    debug!("start connection task");
    debug!("Device capabilities: {:?}", controller.capabilities());
    loop {
        if esp_wifi::wifi::wifi_state() == WifiState::StaConnected {
            controller.wait_for_event(WifiEvent::StaDisconnected).await;
            Timer::after(Duration::from_millis(5000)).await;
        }
        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = Configuration::Client(ClientConfiguration {
                ssid: SSID.into(),
                password: PASSWORD.into(),
                ..Default::default()
            });
            controller.set_configuration(&client_config).unwrap();
            debug!("Starting wifi");
            controller.start_async().await.unwrap();
            info!("Wifi started");
        }
        debug!("About to connect...");

        match controller.connect_async().await {
            Ok(_) => info!("Wifi connected"),
            Err(e) => {
                error!("Failed to connect to wifi: {:?}", e);
                Timer::after(Duration::from_millis(5000)).await
            }
        }
    }
}

#[embassy_executor::task]
pub async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await
}

#[embassy_executor::task]
pub async fn heap_stats_printer() {
    loop {
        esp_println::println!("{}", esp_alloc::HEAP.stats());
        Timer::after_secs(5).await;
    }
}

#[embassy_executor::task]
pub async fn led_rainbow(
    mut led: SmartLedsAdapterAsync<esp_hal::rmt::ConstChannelAccess<esp_hal::rmt::Tx, 0>, 25>,
) {
    let mut color = Hsv {
        hue: 0,
        sat: 255,
        val: 255,
    };
    let mut data: RGB8;
    let level = 255;

    loop {
        info!("LED color sweep");
        for hue in 0..=255 {
            color.hue = hue;
            data = hsv2rgb(color);
            led.write(brightness(gamma([data].into_iter()), level))
                .await
                .unwrap();
            Timer::after(Duration::from_millis(10)).await;
        }
        Timer::after_secs(1).await;
    }
}

#[embassy_executor::task]
pub async fn hub75_hello_world(mut hub75: Hub75<'static, Async>) {
    let mut fb = FBType::new();
    let text_style = MonoTextStyleBuilder::new()
        .font(&FONT_5X7)
        .text_color(Color::WHITE)
        .background_color(Color::BLACK)
        .build();
    let point = Point::new(32, 31);
    Text::with_alignment("Hello, World!", point, text_style, Alignment::Center)
        .draw(&mut fb)
        .expect("failed to draw text");

    const STEP: u8 = (256 / COLS) as u8;
    for x in 0..COLS {
        let brightness = (x as u8) * STEP;
        for y in 0..8 {
            fb.set_pixel(Point::new(x as i32, y), Color::new(brightness, 0, 0));
        }
        for y in 8..16 {
            fb.set_pixel(Point::new(x as i32, y), Color::new(0, brightness, 0));
        }
        for y in 16..24 {
            fb.set_pixel(Point::new(x as i32, y), Color::new(0, 0, brightness));
        }
    }

    loop {
        let mut hub75xfer = hub75
            .render(&fb)
            .map_err(|(e, _hub75)| e)
            .expect("failed to start render");
        hub75xfer
            .wait_for_done()
            .await
            .expect("hub75 transfer failed");
        let (xferres, new_hub75) = hub75xfer.wait();
        xferres.expect("transfer failed");
        hub75 = new_hub75;
    }
}

#[embassy_executor::task]
pub async fn hub75_ferris(mut hub75: Hub75<'static, Async>) {
    use embedded_graphics::image::ImageDrawable;
    let mut fb = FBType::new();

    let image =
        tinygif::Gif::<Rgb888>::from_slice(include_bytes!("../assets/Ferris-64x32.gif")).unwrap();
    loop {
        for frame in image.frames() {
            frame.draw(&mut fb).unwrap();

            let mut frame_drawn: Option<Instant> = None;
            let mut frame_complete: Option<Instant> = None;

            'paintspin: loop {
                let mut hub75xfer = hub75
                    .render(&fb)
                    .map_err(|(e, _hub75)| e)
                    .expect("failed to start render");
                hub75xfer
                    .wait_for_done()
                    .await
                    .expect("hub75 transfer failed");
                let (xferres, new_hub75) = hub75xfer.wait();
                xferres.expect("transfer failed");
                hub75 = new_hub75;
                if frame_drawn.is_none() {
                    frame_drawn = Some(Instant::now());
                }
                match frame_complete {
                    None => {
                        if let Some(frame_start) = frame_drawn {
                            frame_complete = Some(
                                frame_start
                                    + Duration::from_millis((frame.delay_centis as u64) * 10),
                            );
                        }
                    }
                    Some(frame_done) => {
                        if Instant::now() > frame_done {
                            break 'paintspin;
                        }
                    }
                };
            }
            // fb.erase();
        }
    }
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

// const TZ: &str = env!("TZ");

// and this is how I learned the "fun" of nested macros
// https://github.com/rust-lang/rust/issues/90765

// fuckin' software, amirite
// const TIMEZONE: jiff::tz::TimeZone = jiff::tz::get!("PST8PDT,M3.2.0,M11.1.0");
const TIMEZONE: jiff::tz::TimeZone = jiff::tz::get!("PST8PDT");
const NTP_SERVER: &str = "pool.ntp.org";
const USEC_IN_SEC: u64 = 1_000_000;

#[derive(Clone, Copy)]
struct Timestamp<'a> {
    rtc: &'a Rtc<'a>,
    current_time_us: u64,
}

impl NtpTimestampGenerator for Timestamp<'_> {
    fn init(&mut self) {
        self.current_time_us = self.rtc.current_time_us();
    }

    fn timestamp_sec(&self) -> u64 {
        self.current_time_us / 1_000_000
    }

    fn timestamp_subsec_micros(&self) -> u32 {
        (self.current_time_us % 1_000_000) as u32
    }
}

#[embassy_executor::task]
pub async fn ntp_sync(stack: embassy_net::Stack<'static>, rtc: Rtc<'static>) {
    let ntp_addrs = stack.dns_query(NTP_SERVER, DnsQueryType::A).await.unwrap();
    if ntp_addrs.is_empty() {
        error!("empty server result for {}", NTP_SERVER);
        return;
    }
    let mut rx_meta = [PacketMetadata::EMPTY; 16];
    let mut rx_buffer = [0; 4096];
    let mut tx_meta = [PacketMetadata::EMPTY; 16];
    let mut tx_buffer = [0; 4096];
    let mut socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );
    socket.bind(123).unwrap();
    let now = jiff::Timestamp::from_microsecond(rtc.current_time_us() as i64).unwrap();
    info!("ntp_sync: RTC: {}", now.to_string().as_str());
    loop {
        let addr: IpAddr = ntp_addrs[0].into();
        info!("ntp_sync: get_time");
        let result = get_time(
            SocketAddr::from((addr, 123)),
            &socket,
            NtpContext::new(Timestamp {
                rtc: &rtc,
                current_time_us: 0,
            }),
        )
        .await;
        match result {
            Ok(time) => {
                rtc.set_current_time_us(
                    (time.sec() as u64 * USEC_IN_SEC)
                        + ((time.sec_fraction() as u64 * USEC_IN_SEC) >> 32),
                );
                info!("ntp_sync: NTP Result: {:?}", time);
                info!(
                    "ntp_sync: net time: {}",
                    jiff::Timestamp::from_second(time.sec() as i64)
                        .unwrap()
                        .checked_add(
                            jiff::Span::new()
                                .nanoseconds((time.seconds_fraction as i64 * 1_000_000_000) >> 32),
                        )
                        .unwrap()
                        .to_zoned(TIMEZONE)
                        .to_string()
                        .as_str()
                );
                info!(
                    "ntp_sync: RTC time: {}",
                    jiff::Timestamp::from_microsecond(rtc.current_time_us() as i64)
                        .unwrap()
                        .to_zoned(TIMEZONE)
                        .to_string()
                        .as_str()
                );
            }
            Err(e) => {
                error!("ntp_sync: Error getting time: {:?}", e);
            }
        }
        Timer::after(Duration::from_secs(10)).await;
    }
}
