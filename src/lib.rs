#![no_std]

extern crate alloc;
use alloc::{
    format,
    string::{String, ToString},
};
use core::{
    net::{IpAddr, SocketAddr},
    sync::atomic::{AtomicBool, Ordering},
};
#[cfg(feature = "defmt")]
use defmt::{debug, error, info, trace, write, Format, Formatter};
use embassy_net::{
    dns::DnsSocket,
    tcp::client::{TcpClient, TcpClientState},
    udp::{PacketMetadata, UdpSocket},
    Runner, Stack,
};
use embassy_time::{Duration, Timer};
use embedded_graphics::{
    geometry::Point,
    image::ImageDrawable,
    mono_font::{ascii::FONT_5X7, MonoTextStyle, MonoTextStyleBuilder},
    pixelcolor::Rgb888,
    prelude::RgbColor,
    text::{Alignment, Text},
    Drawable,
};
use esp_hal::{rng::Rng, rtc_cntl::Rtc, time::Rate};
use esp_hub75::{
    framebuffer::{compute_frame_count, compute_rows, plain::DmaFrameBuffer},
    Color, Hub75, Hub75Pins16,
};
use esp_radio::wifi::{ClientConfig, WifiController, WifiDevice, WifiEvent, WifiStaState};
use esp_storage::FlashStorage;
use jiff::Zoned;
use reqwless::{
    client::{HttpClient, TlsConfig, TlsVerify},
    request::{Request, RequestBuilder},
};
use serde_json::Value;
use smoltcp::wire::DnsQueryType;
use sntpc::{get_time, NtpContext, NtpTimestampGenerator};
use static_cell::StaticCell;

pub const SSID: &str = env!("SSID");
pub const PASSWORD: &str = env!("PASSWORD");

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

pub struct DisplayPeripherals<'d> {
    pub parl_io: esp_hal::peripherals::PARL_IO<'d>,
    pub dma_channel: esp_hal::peripherals::DMA_CH0<'d>,
    pub red1: esp_hal::gpio::AnyPin<'d>,
    pub grn1: esp_hal::gpio::AnyPin<'d>,
    pub blu1: esp_hal::gpio::AnyPin<'d>,
    pub red2: esp_hal::gpio::AnyPin<'d>,
    pub grn2: esp_hal::gpio::AnyPin<'d>,
    pub blu2: esp_hal::gpio::AnyPin<'d>,
    pub addr0: esp_hal::gpio::AnyPin<'d>,
    pub addr1: esp_hal::gpio::AnyPin<'d>,
    pub addr2: esp_hal::gpio::AnyPin<'d>,
    pub addr3: esp_hal::gpio::AnyPin<'d>,
    pub addr4: esp_hal::gpio::AnyPin<'d>,
    pub blank: esp_hal::gpio::AnyPin<'d>,
    pub clock: esp_hal::gpio::AnyPin<'d>,
    pub latch: esp_hal::gpio::AnyPin<'d>,
}

const ROWS: usize = 32;
const COLS: usize = 64;
const BITS: u8 = 4; // 3; // TODO: hm
const NROWS: usize = compute_rows(ROWS);
const FRAME_COUNT: usize = compute_frame_count(BITS);
pub type FBType = DmaFrameBuffer<ROWS, COLS, NROWS, BITS, FRAME_COUNT>;
pub type Hub75Type<'d> = Hub75<'d, esp_hal::Async>;
pub type FrameBufferExchange = Signal<CriticalSectionRawMutex, &'static mut FBType>;

const CLOCKPOINT: Point = Point::new(32, 7);
const BGPOINT: Point = Point::new(32, 22);

// static SHOWME: LazyLock<Gif> =
//     LazyLock::new(|| Gif::<Rgb888>::from_slice(include_bytes!("../../assets/showme.gif")).unwrap());

static LOGO: LazyLock<Gif> = LazyLock::new(|| {
    Gif::<Rgb888>::from_slice(include_bytes!("../assets/ranodiclogo.gif")).unwrap()
});

// the painter sends the last painted frame down this signal
static FB_XMIT: FrameBufferExchange = FrameBufferExchange::new();
// the xmitter sends the obsolete fb back
static FB_PAINT: FrameBufferExchange = FrameBufferExchange::new();

pub static FB0: StaticCell<FBType> = StaticCell::new();
pub static FB1: StaticCell<FBType> = StaticCell::new();

#[embassy_executor::task]
pub async fn display_painter(fb_inc: &'static mut FBType) {
    info!("display painter started");
    let mut bgrecvr = BGDATA.receiver().expect("couldn't get BGDATA recvr");
    let mut fb = fb_inc;
    fb.erase();
    loop {
        if !TIME_SYNCED.load(Ordering::Relaxed) && !bgrecvr.contains_value() {
            'animation: for frame in LOGO.get().frames() {
                frame.draw(fb).unwrap();
                FB_XMIT.signal(fb);
                fb = FB_PAINT.wait().await;
                fb.erase();
                if TIME_SYNCED.load(Ordering::Relaxed) || bgrecvr.contains_value() {
                    break 'animation;
                }
                Timer::after_millis(frame.delay_centis as u64 * 10).await;
            }
        } else {
            let now = zgettimeofday().await;
            if TIME_SYNCED.load(Ordering::Relaxed) {
                Text::with_alignment(
                    now.strftime("%a %b %d %y\n%H:%M:%S").to_string().as_str(),
                    CLOCKPOINT,
                    FONT,
                    Alignment::Center,
                )
                .draw(fb)
                .expect("failed to draw text");
            }
            if bgrecvr.contains_value() {
                let bgreading = bgrecvr.get().await;
                let diffsecs = (&now - &bgreading.timestamp)
                    .total(jiff::Unit::Second)
                    .unwrap() as u64;
                let character_style = if diffsecs > STALE_SECS {
                    trace!("datapoint is stale ({})", diffsecs);
                    STALE_FONT
                } else if bgreading.bg <= 60 {
                    trace!("datapoint is superlow");
                    SUPERLOW_FONT
                } else if bgreading.bg <= 75 {
                    trace!("datapoint is low");
                    LOW_FONT
                } else if bgreading.bg >= 150 {
                    trace!("datapoint is high");
                    HIGH_FONT
                } else if bgreading.bg >= 250 {
                    trace!("datapoint is superhigh");
                    SUPERHIGH_FONT
                } else {
                    trace!("datapoint is okay");
                    OKAY_FONT
                };
                Text::with_alignment(
                    bgreading.bg.to_string().as_str(),
                    BGPOINT,
                    character_style,
                    Alignment::Center,
                )
                .draw(fb)
                .expect("failed to draw text");
            }
            FB_XMIT.signal(fb);
            fb = FB_PAINT.wait().await;
            fb.erase();
        }
        Timer::after_millis(50).await;
    }
}

#[embassy_executor::task]
pub async fn hub75_task(peripherals: DisplayPeripherals<'static>, fb_inc: &'static mut FBType) {
    info!("hub75_task in da house");

    // local binding
    let mut fb = fb_inc;

    let channel = peripherals.dma_channel;
    let (_, tx_descriptors) = esp_hal::dma_descriptors!(0, FBType::dma_buffer_size_bytes());

    let pins = Hub75Pins16 {
        red1: peripherals.red1,
        grn1: peripherals.grn1,
        blu1: peripherals.blu1,
        red2: peripherals.red2,
        grn2: peripherals.grn2,
        blu2: peripherals.blu2,
        addr0: peripherals.addr0,
        addr1: peripherals.addr1,
        addr2: peripherals.addr2,
        addr3: peripherals.addr3,
        addr4: peripherals.addr4,
        blank: peripherals.blank,
        clock: peripherals.clock,
        latch: peripherals.latch,
    };
    let mut hub75 = Hub75Type::new_async(
        peripherals.parl_io,
        pins,
        channel,
        tx_descriptors,
        Rate::from_mhz(20),
    )
    .expect("failed to create hub75");
    loop {
        {
            // first, toss them bits
            let mut hub75xfer = {
                hub75
                    .render(fb)
                    .map_err(|(e, _hub75)| e)
                    .expect("failed to start render")
            };
            hub75xfer
                .wait_for_done()
                .await
                .expect("hub75 transfer failed");

            let (xferres, new_hub75) = hub75xfer.wait();
            xferres.expect("transfer failed");
            hub75 = new_hub75;
        }
        {
            if FB_XMIT.signaled() {
                let new_fb = FB_XMIT.wait().await;
                FB_PAINT.signal(fb);
                fb = new_fb;
            }
        }
        Timer::after_micros(10000).await;
    }
}

pub enum DrawEvent {
    Clock,
}

const FONT: MonoTextStyle<'_, Rgb888> = MonoTextStyleBuilder::new()
    .font(&FONT_5X7)
    .text_color(Color::WHITE)
    .background_color(Color::BLACK)
    .build();

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

pub static RTC: StaticCell<Rtc> = StaticCell::new();
pub static RTCREF: OnceLock<&Rtc> = OnceLock::new();

async fn zgettimeofday() -> Zoned {
    gettimeofday().await.to_zoned(TIMEZONE)
}

async fn gettimeofday() -> jiff::Timestamp {
    jiff::Timestamp::from_microsecond(RTCREF.get().await.current_time_us() as i64)
        .unwrap_or(jiff::Timestamp::MIN)
}

const TIMEZONE: jiff::tz::TimeZone = jiff::tz::get!("PST8PDT");
const NTP_SERVER: &str = env!("NTP_SERVER");
const USEC_IN_SEC: u64 = 1_000_000;
const NTP_INTERVAL: u64 = 600;

pub static TIMEINIT: Watch<CriticalSectionRawMutex, (), 2> = Watch::new();
pub static TIME_SYNCED: AtomicBool = AtomicBool::new(false);

use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, lazy_lock::LazyLock, once_lock::OnceLock,
    signal::Signal,
};
use embassy_sync::{mutex::Mutex, watch::Watch};
use tinygif::Gif;

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
pub async fn ntp_sync(stack: embassy_net::Stack<'static>) {
    info!("ntp sync started");
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
    let ntpctx = NtpContext::new(Timestamp {
        rtc: RTCREF.get().await,
        current_time_us: 0,
    });
    let addr: IpAddr = ntp_addrs[0].into();
    loop {
        debug!("ntp_sync: get_time");
        match get_time(SocketAddr::from((addr, 123)), &socket, ntpctx).await {
            Ok(ntptime) => {
                RTCREF.get().await.set_current_time_us(
                    ntptime.sec() as u64 * USEC_IN_SEC
                        + sntpc::fraction_to_microseconds(ntptime.sec_fraction()) as u64,
                );
                TIMEINIT.sender().send(());
                TIME_SYNCED.store(true, Ordering::Relaxed);
            }
            Err(e) => {
                error!("ntp_sync: Error getting time: {:?}", e);
            }
        }
        Timer::after_secs(NTP_INTERVAL).await;
    }
}

// pub static FLASH: LazyLock<Mutex<CriticalSectionRawMutex, FlashStorage>> =
//     LazyLock::new(|| Mutex::new(FlashStorage::new()));

const NIGHTSCOUT_TOKEN: &str = env!("NIGHTSCOUT_TOKEN");
const NIGHTSCOUT_URL: &str = env!("NIGHTSCOUT_URL");

const RX_BUFFER_SIZE: usize = 16640;
static mut TLS_READ_BUFFER: [u8; RX_BUFFER_SIZE] = [0; RX_BUFFER_SIZE];

pub static RNG: OnceLock<Mutex<CriticalSectionRawMutex, Rng>> = OnceLock::new();

const STALE_SECS: u64 = 600;

const OKAY_FONT: MonoTextStyle<'_, Rgb888> = MonoTextStyleBuilder::new()
    .font(&FONT_5X7)
    .text_color(Color::GREEN)
    .background_color(Color::BLACK)
    .build();

const HIGH_FONT: MonoTextStyle<'_, Rgb888> = MonoTextStyleBuilder::new()
    .font(&FONT_5X7)
    .text_color(Color::YELLOW)
    .background_color(Color::BLACK)
    .build();

const SUPERHIGH_FONT: MonoTextStyle<'_, Rgb888> = MonoTextStyleBuilder::new()
    .font(&FONT_5X7)
    .text_color(Color::BLACK)
    .background_color(Color::YELLOW)
    .build();

const LOW_FONT: MonoTextStyle<'_, Rgb888> = MonoTextStyleBuilder::new()
    .font(&FONT_5X7)
    .text_color(Color::RED)
    .background_color(Color::BLACK)
    .build();

const SUPERLOW_FONT: MonoTextStyle<'_, Rgb888> = MonoTextStyleBuilder::new()
    .font(&FONT_5X7)
    .text_color(Color::WHITE)
    .background_color(Color::RED)
    .build();

const STALE_FONT: MonoTextStyle<'_, Rgb888> = MonoTextStyleBuilder::new()
    .font(&FONT_5X7)
    .text_color(Color::WHITE)
    .background_color(Color::BLUE)
    .build();

#[derive(Clone, Debug)]
pub struct BgReading {
    bg: u64,
    units: String,
    timestamp: jiff::Zoned,
}

impl Format for BgReading {
    fn format(&self, fmt: Formatter) {
        write!(fmt, "BgReading({} {} @ ", self.bg, self.units.as_str());
        write!(fmt, "{})", self.timestamp.to_string().as_str());
    }
}

pub static BGDATA: Watch<CriticalSectionRawMutex, BgReading, 2> = Watch::new();
const BG_SUCCESS_INTERVAL: u64 = 150;
const BG_FAILTURE_INTERVAL: u64 = 10;

#[embassy_executor::task]
pub async fn nightscout_query(stack: embassy_net::Stack<'static>) {
    let rng = Rng::new();
    let sender = BGDATA.sender();
    let client_state = TcpClientState::<4, 1024, 1024>::new();
    let tcp_client = TcpClient::new(stack, &client_state);
    let dns_client = DnsSocket::new(stack);
    let tls_config = TlsConfig::new(
        (rng.random() as u64) << 32 | rng.random() as u64,
        unsafe { &mut *core::ptr::addr_of_mut!(TLS_READ_BUFFER) },
        unsafe { &mut *core::ptr::addr_of_mut!(TLS_READ_BUFFER) },
        TlsVerify::None,
    );

    let mut http_client = HttpClient::new_with_tls(&tcp_client, &dns_client, tls_config);
    let path = format!(
        "/api/v1/entries/sgv.json?count=2&token={}",
        NIGHTSCOUT_TOKEN
    );

    let mut temp_buffer: [u8; 20000] = [0; 20000];

    loop {
        let pathreq = Request::get(&path)
            .content_type(reqwless::headers::ContentType::ApplicationJson)
            .build();
        let mut c = http_client
            .resource(NIGHTSCOUT_URL)
            .await
            .expect("couldn't resource");

        let resp = c.send(pathreq, &mut temp_buffer).await.expect("http error");

        if !resp.status.is_successful() {
            error!("failed HTTP {:?}", resp.status.0);
            Timer::after_secs(BG_FAILTURE_INTERVAL).await;
            continue;
        }
        if resp.content_length.is_none() {
            error!("null content length");
            Timer::after_secs(BG_FAILTURE_INTERVAL).await;
            continue;
        }
        let bsz = resp.content_length.expect("content length");
        let mut contentbuf = alloc::vec![0u8; bsz].into_boxed_slice();
        resp.body()
            .reader()
            .read_to_end(&mut contentbuf)
            .await
            .expect("bodyread");
        let entries: Value = serde_json::from_slice(&contentbuf).expect("valued");
        if let (Some(bgda), Some(bgu), Some(bgdt), Some(bgzo)) = (
            entries[0]["sgv"].as_u64(),
            entries[0]["units"].as_str(),
            entries[0]["date"].as_i64(),
            entries[0]["utcOffset"].as_i64(),
        ) {
            let reading = BgReading {
                bg: bgda,
                units: bgu.into(),
                timestamp: jiff::Timestamp::from_millisecond(bgdt)
                    .expect("couldn't timestamp")
                    .to_zoned(jiff::tz::TimeZone::fixed(
                        jiff::tz::Offset::from_seconds((bgzo * 60).try_into().unwrap())
                            .expect("couldn't offset zone"),
                    )),
            };
            info!("sent reading: {:?}", reading);
            sender.send(reading);
            Timer::after_secs(BG_SUCCESS_INTERVAL).await;
        } else {
            error!("one of the columns wasn't defined");
            Timer::after_secs(BG_FAILTURE_INTERVAL).await;
        };
    }
}

pub static FLASH: StaticCell<FlashStorage> = StaticCell::new();
