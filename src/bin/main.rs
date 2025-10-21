#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![feature(type_alias_impl_trait)]
extern crate alloc;
#[cfg(feature = "defmt")]
use defmt::info;
use embassy_executor::SendSpawner;
use embassy_net::StackResources;
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_bootloader_esp_idf::partitions::PartitionEntry;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::Pin;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use esp_println as _;
use esp_radio::Controller;
use esp_rtos::embassy::InterruptExecutor;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

const NUM_SOCKS: usize = 4;
static STACK_RESOURCES: StaticCell<StackResources<NUM_SOCKS>> = StaticCell::new();
static WIFI_CONTROLLER: StaticCell<Controller> = StaticCell::new();

const HIPRI_CORE: u8 = 2;
static HIPRI_EXECUTOR: StaticCell<InterruptExecutor<HIPRI_CORE>> = StaticCell::new();
static HIPRI_SPAWNER: StaticCell<SendSpawner> = StaticCell::new();

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(size: 64 * 1024);
    esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: 64 * 1024);

    let hp_executor = {
        #[cfg(target_arch = "riscv32")]
        use esp_hal::interrupt::software::SoftwareInterruptControl;
        let software_interrupt = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
        let timg0 = TimerGroup::new(peripherals.TIMG0);
        esp_rtos::start(
            timg0.timer0,
            #[cfg(target_arch = "riscv32")]
            software_interrupt.software_interrupt0,
        );

        ranodic::RTCREF
            .init(ranodic::RTC.init(esp_hal::rtc_cntl::Rtc::new(peripherals.LPWR)))
            .ok();

        HIPRI_EXECUTOR.init(InterruptExecutor::<HIPRI_CORE>::new(
            software_interrupt.software_interrupt2,
        ))
    };

    let hp_spawner = HIPRI_SPAWNER.init(hp_executor.start(esp_hal::interrupt::Priority::Priority3));

    {
        let fb0 = ranodic::FB0.init(ranodic::FBType::new());
        let fb1 = ranodic::FB1.init(ranodic::FBType::new());
        hp_spawner.must_spawn(ranodic::hub75_task(
            // tried to pick least-annoying pinout for both the DevKit-C and DevKit-M
            ranodic::DisplayPeripherals {
                parl_io: peripherals.PARL_IO,
                dma_channel: peripherals.DMA_CH0,
                red1: peripherals.GPIO19.degrade(),
                grn1: peripherals.GPIO21.degrade(),
                blu1: peripherals.GPIO20.degrade(),
                red2: peripherals.GPIO22.degrade(),
                grn2: peripherals.GPIO18.degrade(),
                blu2: peripherals.GPIO23.degrade(),
                addr0: peripherals.GPIO9.degrade(),
                addr1: peripherals.GPIO2.degrade(),
                addr2: peripherals.GPIO1.degrade(),
                addr3: peripherals.GPIO0.degrade(),
                addr4: peripherals.GPIO15.degrade(),
                blank: peripherals.GPIO3.degrade(),
                clock: peripherals.GPIO7.degrade(),
                latch: peripherals.GPIO6.degrade(),
            },
            fb1,
        ));
        spawner.must_spawn(ranodic::display_painter(fb0));
    }

    // hardware stack init for wifi [link]
    let wdevice = {
        let (controller, interfaces) = esp_radio::wifi::new(
            WIFI_CONTROLLER.init(esp_radio::init().unwrap()),
            peripherals.WIFI,
            Default::default(),
        )
        .unwrap();
        let wcn_watchdog_task = ranodic::conn_watchdog(controller);
        spawner.must_spawn(wcn_watchdog_task);
        interfaces.sta
    };

    // software stack init for wifi [stack]
    let stack = {
        let rng = Rng::new();
        let dhcpcfg = embassy_net::Config::dhcpv4(Default::default());
        let seed = (rng.random() as u64) << 32 | rng.random() as u64;
        let (stack, netrunner) = embassy_net::new(
            wdevice,
            dhcpcfg,
            STACK_RESOURCES.init(StackResources::<NUM_SOCKS>::new()),
            seed,
        );
        let netrun_task = ranodic::net_task(netrunner);
        spawner.must_spawn(netrun_task);
        stack
    };

    // remaining things are netstuff so await function
    ranodic::net_up(stack).await;

    {
        // NTP
        let ntp_task = ranodic::ntp_sync(stack);
        spawner.must_spawn(ntp_task);
    }

    {
        let nightscout_task = ranodic::nightscout_query(stack);
        spawner.must_spawn(nightscout_task);
    }

    {
        // OTA [TODO]
        use alloc::vec::Vec;
        use esp_storage::FlashStorage;
        let mut buffer = [0u8; esp_bootloader_esp_idf::partitions::PARTITION_TABLE_MAX_LEN];
        let flash = ranodic::FLASH.init(FlashStorage::new(peripherals.FLASH));
        let partition_table =
            esp_bootloader_esp_idf::partitions::read_partition_table(flash, &mut buffer).unwrap();

        let mut partitions: Vec<PartitionEntry> = Vec::new();
        let entries = partition_table.len();
        for i in 0..entries {
            let partition = partition_table.get_partition(i).unwrap();
            info!("{:?}", partition);
            partitions.push(partition);
        }
        info!(
            "Currently booted partition {:?}",
            partition_table.booted_partition()
        );
    }

    // spawner.spawn(ranodic::heap_stats_printer()).ok();

    info!("steady state; awaiting heat death of the universe");
    loop {
        Timer::after(Duration::from_secs(5)).await;
    }
}
