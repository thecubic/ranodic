#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![feature(type_alias_impl_trait)]

extern crate alloc;
use embassy_net::{Runner, StackResources};
use esp_hal::clock::CpuClock;
use esp_hal::gpio::Pin;
use esp_hal::rmt::Rmt;
use esp_hal::time::Rate;
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::Async;

#[cfg(feature = "defmt")]
use defmt::{debug, error, info};

use esp_hal_smartled::{buffer_size_async, SmartLedsAdapterAsync};
use esp_println as _;

use embassy_time::{Duration, Timer};

use esp_backtrace as _;
use esp_wifi::wifi::{
    ClientConfiguration, Configuration, WifiController, WifiDevice, WifiEvent, WifiState,
};
use static_cell::make_static;

const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");

esp_bootloader_esp_idf::esp_app_desc!();

use esp_hub75::{
    framebuffer::{compute_frame_count, compute_rows, plain::DmaFrameBuffer},
    *,
};
const ROWS: usize = 32;
const COLS: usize = 64;
const BITS: u8 = 4;
const NROWS: usize = compute_rows(ROWS);
const FRAME_COUNT: usize = compute_frame_count(BITS);
type FBType = DmaFrameBuffer<ROWS, COLS, NROWS, BITS, FRAME_COUNT>;

use embedded_graphics::geometry::Point;
use embedded_graphics::mono_font::ascii::FONT_5X7;
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::prelude::RgbColor;
use embedded_graphics::text::Alignment;
use embedded_graphics::text::Text;
use embedded_graphics::Drawable;

use smart_leds::{
    brightness, gamma,
    hsv::{hsv2rgb, Hsv},
    SmartLedsWriteAsync, RGB8,
};

#[esp_hal_embassy::main]
async fn main(spawner: embassy_executor::Spawner) {
    // esp_println::logger::init_logger_from_env();
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 64 * 1024);
    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: 64 * 1024);

    let timer0 = SystemTimer::new(peripherals.SYSTIMER);
    esp_hal_embassy::init(timer0.alarm0);

    info!("Embassy initialized!");

    {
        let (_, tx_descriptors) = esp_hal::dma_descriptors!(0, FBType::dma_buffer_size_bytes());
        let pins = Hub75Pins16 {
            red1: peripherals.GPIO19.degrade(),
            grn1: peripherals.GPIO21.degrade(),
            blu1: peripherals.GPIO20.degrade(),
            // the panel itself is backwards, these are if it wasn't
            // grn1: peripherals.GPIO20.degrade(),
            // blu1: peripherals.GPIO21.degrade(),
            red2: peripherals.GPIO22.degrade(),
            // the panel itself is backwards, these are if it wasn't
            // grn2: peripherals.GPIO23.degrade(),
            // blu2: peripherals.GPIO18.degrade(),
            grn2: peripherals.GPIO18.degrade(),
            blu2: peripherals.GPIO23.degrade(),
            addr0: peripherals.GPIO10.degrade(),
            addr1: peripherals.GPIO2.degrade(),
            addr2: peripherals.GPIO1.degrade(),
            addr3: peripherals.GPIO0.degrade(),
            addr4: peripherals.GPIO11.degrade(),
            blank: peripherals.GPIO3.degrade(),
            clock: peripherals.GPIO7.degrade(),
            latch: peripherals.GPIO6.degrade(),
        };

        let hub75 = Hub75::new_async(
            peripherals.PARL_IO,
            pins,
            peripherals.DMA_CH0,
            tx_descriptors,
            Rate::from_mhz(20),
        )
        .expect("couldn't create Hub75 driver");
        info!("created hub75 driver");
        info!("starting hub75 hello world");
        spawner.spawn(hub75_hello_world(hub75)).ok();
    }

    {
        // init smart led
        // technically an IR remote control *IS* a PWM LED
        #[cfg(feature = "esp32h2")]
        let frequency = Rate::from_mhz(32);
        #[cfg(not(feature = "esp32h2"))]
        let frequency = Rate::from_mhz(80);

        let rmt: Rmt<'_, esp_hal::Async> = Rmt::new(peripherals.RMT, frequency)
            .expect("Failed to initialize RMT")
            .into_async();

        let rmt_channel = rmt.channel0;
        let rmt_buffer = [0_u32; buffer_size_async(1)];

        #[cfg(feature = "esp32")]
        let ledpin = peripherals.GPIO33;
        #[cfg(feature = "esp32c3")]
        let ledpin = peripherals.GPIO2;
        #[cfg(any(feature = "esp32c6", feature = "esp32h2"))]
        let ledpin = peripherals.GPIO8;
        #[cfg(feature = "esp32s2")]
        let ledpin = peripherals.GPIO18;
        #[cfg(feature = "esp32s3")]
        let ledpin = peripherals.GPIO48;

        let led: SmartLedsAdapterAsync<_, 25> =
            SmartLedsAdapterAsync::new(rmt_channel, ledpin, rmt_buffer);

        info!("starting LED rainbow");
        spawner.spawn(led_rainbow(led)).ok();
    }

    {
        let mut rng = esp_hal::rng::Rng::new(peripherals.RNG);
        let timer1 = TimerGroup::new(peripherals.TIMG0);
        let wifi_init =
            make_static!(esp_wifi::init(timer1.timer0, rng)
                .expect("Failed to initialize WIFI/BLE controller"));
        let (wifi_controller, interfaces) = esp_wifi::wifi::new(wifi_init, peripherals.WIFI)
            .expect("Failed to initialize WIFI controller");
        let wdevice = interfaces.sta;
        let dhcpcfg = embassy_net::Config::dhcpv4(Default::default());
        let seed = (rng.random() as u64) << 32 | rng.random() as u64;
        let (stack, netrunner) = embassy_net::new(
            wdevice,
            dhcpcfg,
            make_static!(StackResources::<3>::new()),
            seed,
        );
        let wcn_watchdog_task = conn_watchdog(wifi_controller);
        spawner.spawn(wcn_watchdog_task).ok();
        let netrun_task = net_task(netrunner);
        spawner.spawn(netrun_task).ok();

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

    spawner.spawn(heap_stats_printer()).ok();
    info!("steady state; awaiting heat death of the universe");
    loop {
        Timer::after(Duration::from_secs(5)).await;
    }
}

#[embassy_executor::task]
async fn conn_watchdog(mut controller: WifiController<'static>) {
    debug!("start connection task");
    debug!("Device capabilities: {:?}", controller.capabilities());
    loop {
        match esp_wifi::wifi::wifi_state() {
            WifiState::StaConnected => {
                controller.wait_for_event(WifiEvent::StaDisconnected).await;
                Timer::after(Duration::from_millis(5000)).await
            }
            _ => {}
        }
        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = Configuration::Client(ClientConfiguration {
                ssid: SSID.try_into().unwrap(),
                password: PASSWORD.try_into().unwrap(),
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
async fn net_task(mut runner: Runner<'static, WifiDevice<'static>>) {
    runner.run().await
}

#[embassy_executor::task]
async fn heap_stats_printer() {
    loop {
        esp_println::println!("{}", esp_alloc::HEAP.stats());
        Timer::after_secs(5).await;
    }
}

#[embassy_executor::task]
async fn led_rainbow(
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
async fn hub75_hello_world(mut hub75: Hub75<'static, Async>) {
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
