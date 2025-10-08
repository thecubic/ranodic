#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![feature(type_alias_impl_trait)]

extern crate alloc;
use embassy_net::StackResources;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::Pin;
use esp_hal::rmt::Rmt;
use esp_hal::time::Rate;
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::timer::timg::TimerGroup;

#[cfg(feature = "defmt")]
use defmt::info;

use esp_hal_smartled::{buffer_size_async, SmartLedsAdapterAsync};
use esp_println as _;

use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::rtc_cntl::Rtc;
use static_cell::make_static;

esp_bootloader_esp_idf::esp_app_desc!();

use esp_hub75::{Hub75, Hub75Pins16};

#[esp_hal_embassy::main]
async fn main(spawner: embassy_executor::Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 64 * 1024);
    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: 64 * 1024);

    {
        let timer0 = SystemTimer::new(peripherals.SYSTIMER);
        esp_hal_embassy::init(timer0.alarm0);
        info!("Embassy initialized!");
    }

    {
        let (_, tx_descriptors) =
            esp_hal::dma_descriptors!(0, ranodic::FBType::dma_buffer_size_bytes());
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
        // info!("starting hub75 hello world");
        // spawner.spawn(ranodic::hub75_hello_world(hub75)).ok();
        info!("starting hub75 ferris");
        spawner.spawn(ranodic::hub75_ferris(hub75)).ok();
    }

    // {
    //     // init smart led
    //     // technically an IR remote control *IS* a PWM LED
    //     #[cfg(feature = "esp32h2")]
    //     let frequency = Rate::from_mhz(32);
    //     #[cfg(not(feature = "esp32h2"))]
    //     let frequency = Rate::from_mhz(80);

    //     let rmt: Rmt<'_, esp_hal::Async> = Rmt::new(peripherals.RMT, frequency)
    //         .expect("Failed to initialize RMT")
    //         .into_async();

    //     let rmt_channel = rmt.channel0;
    //     let rmt_buffer = [0_u32; buffer_size_async(1)];

    //     #[cfg(feature = "esp32")]
    //     let ledpin = peripherals.GPIO33;
    //     #[cfg(feature = "esp32c3")]
    //     let ledpin = peripherals.GPIO2;
    //     #[cfg(any(feature = "esp32c6", feature = "esp32h2"))]
    //     let ledpin = peripherals.GPIO8;
    //     #[cfg(feature = "esp32s2")]
    //     let ledpin = peripherals.GPIO18;
    //     #[cfg(feature = "esp32s3")]
    //     let ledpin = peripherals.GPIO48;

    //     let led: SmartLedsAdapterAsync<_, 25> =
    //         SmartLedsAdapterAsync::new(rmt_channel, ledpin, rmt_buffer);

    //     info!("starting LED rainbow");
    //     spawner.spawn(ranodic::led_rainbow(led)).ok();
    // }

    let stack = {
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
        let wcn_watchdog_task = ranodic::conn_watchdog(wifi_controller);
        spawner.spawn(wcn_watchdog_task).ok();
        let netrun_task = ranodic::net_task(netrunner);
        spawner.spawn(netrun_task).ok();

        ranodic::net_up(stack).await;
        stack
    };

    {
        // NTP
        let rtc = Rtc::new(peripherals.LPWR);
        let ntp_task = ranodic::ntp_sync(stack, rtc);
        spawner.spawn(ntp_task).ok();
    }

    // spawner.spawn(ranodic::heap_stats_printer()).ok();
    info!("steady state; awaiting heat death of the universe");
    loop {
        Timer::after(Duration::from_secs(5)).await;
    }
}
