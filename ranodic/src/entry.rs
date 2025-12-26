extern crate alloc;

#[cfg(feature = "defmt")]
use defmt::{error, info, println};
use embassy_executor::SendSpawner;
use embassy_net::{DhcpConfig, StackResources};
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock, delay::Delay, interrupt::software::SoftwareInterruptControl, rng::Rng,
    timer::timg::TimerGroup,
};
use esp_println as _;
use esp_radio::Controller;
use esp_rtos::embassy::InterruptExecutor;
use static_cell::StaticCell;

const NUM_SOCKS: usize = 8;
static STACK_RESOURCES: StaticCell<StackResources<NUM_SOCKS>> = StaticCell::new();
static WIFI_CONTROLLER: StaticCell<Controller> = StaticCell::new();

const HIPRI_CORE: u8 = 2;
static HIPRI_EXECUTOR: StaticCell<InterruptExecutor<HIPRI_CORE>> = StaticCell::new();
static HIPRI_SPAWNER: StaticCell<SendSpawner> = StaticCell::new();

static DELAY: Delay = Delay::new();
#[unsafe(no_mangle)]
pub extern "Rust" fn custom_halt() -> ! {
    error!("halted, resetting in");
    println!("5");
    DELAY.delay_millis(1000);
    println!("4");
    DELAY.delay_millis(1000);
    println!("3");
    DELAY.delay_millis(1000);
    println!("2");
    DELAY.delay_millis(1000);
    println!("1");
    DELAY.delay_millis(1000);
    esp_hal::system::software_reset();
}

#[esp_rtos::main]
pub async fn main(spawner: embassy_executor::Spawner) {
    info!("RRRRRRRRRRRRRRR");
    info!("AAAAAAAAAAAAAAA");
    info!("NNNNNNNNNNNNNNN");
    info!("OOOOOOOOOOOOOOO");
    info!("DDDDDDDDDDDDDDD");
    info!("IIIIIIIIIIIIIII");
    info!("CCCCCCCCCCCCCCC");
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    crate::show_reset_reason();

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64000);
    #[cfg(not(feature = "esp32"))]
    esp_alloc::heap_allocator!(size: 48 * 1024);
    #[cfg(feature = "esp32")]
    esp_alloc::heap_allocator!(size: 16 * 1024);

    let rtc = crate::RTC
        .init_with(|| core::cell::UnsafeCell::new(esp_hal::rtc_cntl::Rtc::new(peripherals.LPWR)));
    rtc.get_mut()
        .set_interrupt_handler(crate::RTC_INTERRUPT_HANDLER);
    crate::RTCREF.init(unsafe { rtc.as_mut_unchecked() }).ok();
    let rwdt = unsafe { &mut rtc.as_mut_unchecked().rwdt };

    info!("starting RTC watchdog");
    spawner.must_spawn(crate::watchdog_controller(rwdt));

    #[cfg(feature = "rtcchip")]
    let _ = crate::rtc::ic_to_sys().await;

    let hp_executor = {
        let software_interrupt = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
        let timg0 = TimerGroup::new(peripherals.TIMG0);
        esp_rtos::start(
            timg0.timer0,
            #[cfg(target_arch = "riscv32")]
            software_interrupt.software_interrupt0,
        );
        HIPRI_EXECUTOR.init_with(|| {
            InterruptExecutor::<HIPRI_CORE>::new(software_interrupt.software_interrupt2)
        })
    };

    let hp_spawner =
        HIPRI_SPAWNER.init_with(|| hp_executor.start(esp_hal::interrupt::Priority::Priority3));

    hp_spawner.must_spawn(crate::ntp::tick_writer());

    // 4 brightness bits slays the stack here
    let fb0 = crate::drawing::FB0.init_with(|| crate::hub75::FBType::new());
    let fb1 = crate::drawing::FB1.init_with(|| crate::hub75::FBType::new());
    hp_spawner.must_spawn(crate::hub75::hub75_task(
        // tried to pick least-annoying pinout for both the DevKit-C and DevKit-M
        Default::default(),
        fb1,
    ));
    spawner.must_spawn(crate::drawing::display_painter(fb0));

    if crate::net::SSID.is_empty() || crate::net::SSID == crate::net::DEFAULT_SSID {
        error!("no WIFI configured");
    } else {
        info!(
            "SSID: {} PASSWORD: {}",
            crate::net::SSID,
            crate::net::PASSWORD
        );
        // hardware stack init for wifi [link]
        let wdevice = {
            let (controller, interfaces) = esp_radio::wifi::new(
                WIFI_CONTROLLER.init(esp_radio::init().unwrap()),
                peripherals.WIFI,
                Default::default(),
            )
            .unwrap();
            let wcn_watchdog_task = crate::net::conn_watchdog(controller);
            spawner.must_spawn(wcn_watchdog_task);
            interfaces.sta
        };

        // software stack init for wifi [stack]
        let stack = {
            let rng = Rng::new();
            let mut dhcpcfg: DhcpConfig = Default::default();
            // MAX_HOSTNAME_LEN == 32 but they ain't export that
            crate::MAC_ADDRESS.init(wdevice.mac_address()).unwrap();
            let mac_address = crate::MAC_ADDRESS.get().await;
            dhcpcfg.hostname = Some(
                heapless::String::<32>::try_from(
                    alloc::format!(
                        "ranodic-{:02x}{:02x}{:02x}",
                        mac_address[3],
                        mac_address[4],
                        mac_address[5]
                    )
                    .as_str(),
                )
                .or_else(|_| heapless::String::<32>::try_from("ranodic-unknown"))
                .expect("couldn't make heapless strings"),
            );

            let netcfg = embassy_net::Config::dhcpv4(dhcpcfg);

            let seed = (rng.random() as u64) << 32 | rng.random() as u64;
            let (stack, netrunner) = embassy_net::new(
                wdevice,
                netcfg,
                STACK_RESOURCES.init(StackResources::<NUM_SOCKS>::new()),
                seed,
            );
            let netrun_task = crate::net::net_task(netrunner);
            spawner.must_spawn(netrun_task);
            stack
        };
        // net tasks have their own net up guards
        spawner.must_spawn(crate::ntp::ntp_sync(stack));
        spawner.must_spawn(crate::nightscout::nightscout_query(stack));
        spawner.must_spawn(crate::weather::weather_query(stack));
    }
    spawner.must_spawn(crate::rtc::desync_failsafe());

    #[cfg(feature = "heapstats")]
    spawner.spawn(crate::heap_stats_printer()).ok();

    #[cfg(feature = "harakiri")]
    spawner.must_spawn(crate::harakiri());

    info!("steady state; awaiting heat death of the universe");
    loop {
        Timer::after(Duration::from_secs(5)).await;
    }
}
