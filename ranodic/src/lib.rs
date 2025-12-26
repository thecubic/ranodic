#![no_std]
#![feature(unsafe_cell_access)]

pub mod config;
pub mod drawing;
pub mod entry;
pub mod forecast;
pub mod hub75;
pub mod net;
pub mod nightscout;
pub mod ntp;
#[cfg(feature = "rtcchip")]
pub mod rtc;
// pub mod storage;
pub mod weather;

extern crate alloc;
use core::cell::UnsafeCell;

use core::sync::atomic::{AtomicBool, AtomicI8, Ordering};

#[cfg(feature = "defmt")]
use defmt::{debug, error, info, warn};
use embassy_executor::{SendSpawner, Spawner};

use embassy_sync::once_lock::OnceLock;
use embassy_time::Timer;

use esp_hal::interrupt::{InterruptHandler, Priority};
#[cfg(feature = "esp32c6")]
use esp_hal::peripherals::LP_WDT;
#[cfg(any(feature = "esp32", feature = "esp32s3"))]
use esp_hal::peripherals::LPWR;
use esp_hal::rom::software_reset;
use esp_hal::rtc_cntl::{Rtc, Rwdt, RwdtStage, RwdtStageAction, SocResetReason, wakeup_cause};
use esp_hal::system::SleepSource;
use esp_hal::system::reset_reason;
use esp_hal::time::Duration;

use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

#[embassy_executor::task]
pub async fn heap_stats_printer() {
    loop {
        esp_println::println!("{}", esp_alloc::HEAP.stats());
        Timer::after_secs(5).await;
    }
}

pub static SPAWNER: StaticCell<Spawner> = StaticCell::new();
pub static HP_SPAWNER: StaticCell<SendSpawner> = StaticCell::new();

pub const HARAKIRI_TIME: u64 = 3600 / 2;

#[embassy_executor::task]
pub async fn harakiri() {
    info!("scheduling harakiri in {} seconds", HARAKIRI_TIME);
    Timer::after_secs(HARAKIRI_TIME).await;
    graceful_shutdown();
    Timer::after_secs(1).await;
    info!("guess i'll die");
    software_reset();
}

static DIVINE_LIGHT: AtomicI8 = AtomicI8::new(5);
#[unsafe(no_mangle)]
pub extern "C" fn sever_divine_light() {
    let light = DIVINE_LIGHT.fetch_sub(1, Ordering::Relaxed);
    info!("severed divine light: {}", light);
    if light == 4 {
        warn!("four divine lights left");
    } else if light == 3 {
        warn!("three divine lights left");
    } else if light == 2 {
        warn!("two divine lights left");
    } else if light == 1 {
        warn!("one divine light left");
    } else {
        error!("no divine light left, resetting...");
        software_reset();
    }
}

pub async fn graceful_sever_divine_light() {
    let light = DIVINE_LIGHT.fetch_sub(1, Ordering::Relaxed);
    info!("severed divine light: {}", light);
    if light == 4 {
        warn!("four divine lights left");
    } else if light == 3 {
        warn!("three divine lights left");
    } else if light == 2 {
        warn!("two divine lights left");
    } else if light == 1 {
        warn!("one divine light left");
    } else {
        error!("no divine light left, resetting...");
        graceful_shutdown();
        Timer::after_secs(1).await;
        software_reset();
    }
}

pub static MAC_ADDRESS: OnceLock<[u8; 6]> = OnceLock::new();

fn clear_rtc_interrupt() {
    #[cfg(feature = "esp32c6")]
    let wpr = LP_WDT::regs().wdtwprotect();

    #[cfg(any(feature = "esp32s3", feature = "esp32"))]
    let wpr = LPWR::regs().wdtwprotect();

    // disable write protection
    wpr.write(|w| unsafe { w.bits(0x50D8_3AA1) });

    #[cfg(feature = "esp32c6")]
    LP_WDT::regs()
        .int_clr()
        .write(|w| w.wdt().clear_bit_by_one());

    #[cfg(any(feature = "esp32s3", feature = "esp32"))]
    LPWR::regs().int_clr().write(|w| w.wdt().clear_bit_by_one());

    wpr.write(|w| unsafe { w.bits(0u32) });
}

#[unsafe(no_mangle)]
extern "C" fn RtcInterruptHandler() {
    let cpu = esp_hal::system::Cpu::current();

    // stuff here

    info!("RTC interrupt on {:?}", cpu);

    // i do not know how to get the WDT object here
    // so copypasta cowabunga it is

    #[cfg(feature = "esp32c6")]
    let wpr = LP_WDT::regs().wdtwprotect();

    #[cfg(any(feature = "esp32s3", feature = "esp32"))]
    let wpr = LPWR::regs().wdtwprotect();

    // disable write protection
    wpr.write(|w| unsafe { w.bits(0x50D8_3AA1) });

    #[cfg(feature = "esp32c6")]
    LP_WDT::regs()
        .int_clr()
        .write(|w| w.wdt().clear_bit_by_one());

    #[cfg(any(feature = "esp32s3", feature = "esp32"))]
    LPWR::regs().int_clr().write(|w| w.wdt().clear_bit_by_one());

    wpr.write(|w| unsafe { w.bits(0u32) });

    info!("RTC interrupt cleared");

    // sever_divine_light();
    // with_protected_write(do_clear_interrupt);
}

pub const RTC_INTERRUPT_HANDLER: InterruptHandler =
    InterruptHandler::new(RtcInterruptHandler, Priority::max());

pub static RTC: StaticCell<UnsafeCell<Rtc>> = StaticCell::new();
pub static RTCREF: OnceLock<&'static mut Rtc> = OnceLock::new();
// pub static mut RWDTREF: OnceLock<&'static mut Rwdt> = OnceLock::new();

#[embassy_executor::task]
pub async fn watchdog_controller(rwdt: &'static mut Rwdt) {
    rwdt.set_timeout(RwdtStage::Stage0, Duration::from_secs(1));
    rwdt.set_stage_action(RwdtStage::Stage0, RwdtStageAction::Interrupt);
    rwdt.set_timeout(RwdtStage::Stage1, Duration::from_secs(2));
    rwdt.set_stage_action(RwdtStage::Stage1, RwdtStageAction::ResetSystem);
    clear_rtc_interrupt();
    rwdt.enable();
    rwdt.listen();
    let mut feeds = 0u64;
    loop {
        rwdt.feed();
        feeds += 1;
        if feeds % 100 == 0 {
            debug!("watchdog_controller: what the dog doin?");
        }
        Timer::after_millis(300).await;
    }
}

pub static GRACEFUL_SHUTDOWN: AtomicBool = AtomicBool::new(false);
pub fn graceful_shutdown() {
    GRACEFUL_SHUTDOWN.store(false, Ordering::Relaxed);
}

// pub static FLASH: LazyLock<Mutex<CriticalSectionRawMutex, FlashStorage>> =
//     LazyLock::new(|| Mutex::new(FlashStorage::new()));

// pub static RNG: OnceLock<Mutex<CriticalSectionRawMutex, Rng>> = OnceLock::new();

pub fn show_wakeup_cause() {
    match wakeup_cause() {
        SleepSource::Undefined => info!("wakeup_cause: Undefined"),
        SleepSource::All => info!("wakeup_cause: All"),
        SleepSource::Ext0 => info!("wakeup_cause: Ext0"),
        SleepSource::Ext1 => info!("wakeup_cause: Ext1"),
        SleepSource::Timer => info!("wakeup_cause: Timer"),
        SleepSource::TouchPad => info!("wakeup_cause: TouchPad"),
        SleepSource::Ulp => info!("wakeup_cause: Ulp"),
        SleepSource::Gpio => info!("wakeup_cause: Gpio"),
        SleepSource::Uart => info!("wakeup_cause: Uart"),
        SleepSource::Wifi => info!("wakeup_cause: Wifi"),
        SleepSource::Cocpu => info!("wakeup_cause: Cocpu"),
        SleepSource::CocpuTrapTrig => info!("wakeup_cause: CocpuTrapTrig"),
        SleepSource::BT => info!("wakeup_cause: BT"),
    }
}

pub fn show_reset_reason() {
    match reset_reason() {
        Some(SocResetReason::ChipPowerOn) => {
            info!("reset_reason: ChipPowerOn");
        }
        // Software resets the digital core by RTC_CNTL_SW_SYS_RST
        Some(SocResetReason::CoreSw) => {
            info!("reset_reason: CoreSw");
        }
        // Deep sleep reset the digital core
        Some(SocResetReason::CoreDeepSleep) => {
            info!("reset_reason: CoreDeepSleep");
        }
        // // SDIO Core reset
        #[cfg(feature = "esp32c6")]
        Some(SocResetReason::CoreSDIO) => {
            info!("reset_reason: CoreSDIO");
        }
        // Main watch dog 0 resets digital core 0
        #[cfg(feature = "esp32c6")]
        Some(SocResetReason::Cpu0Mwdt0) => {
            info!("reset_reason: Cpu0Mwdt0");
        }
        #[cfg(feature = "esp32c6")]
        Some(SocResetReason::Cpu0Sw) => {
            info!("reset_reason: Cpu0Sw");
        }
        #[cfg(feature = "esp32c6")]
        Some(SocResetReason::Cpu0RtcWdt) => {
            info!("reset_reason: Cpu0RtcWdt");
        }
        #[cfg(feature = "esp32c6")]
        Some(SocResetReason::Cpu0Mwdt1) => {
            info!("reset_reason: Cpu0Mwdt1");
        }
        // Main watch dog 0 resets digital core
        Some(SocResetReason::CoreMwdt0) => {
            info!("reset_reason: CoreMwdt0");
        }
        // Main watch dog 1 resets digital core
        Some(SocResetReason::CoreMwdt1) => {
            info!("reset_reason: CoreMwdt1");
        }
        // RTC watch dog resets digital core
        Some(SocResetReason::CoreRtcWdt) => {
            info!("reset_reason: CoreRtcWdt");
        }
        // Main watch dog 0 resets CPU 0
        #[cfg(any(feature = "esp32", feature = "esp32s3"))]
        Some(SocResetReason::CpuMwdt0) => {
            info!("reset_reason: CpuMwdt0");
        }
        // Software resets CPU 0 by RTC_CNTL_SW_PROCPU_RST
        #[cfg(feature = "esp32s3")]
        Some(SocResetReason::CpuSw) => {
            info!("reset_reason: CpuSw");
        }
        // RTC watch dog resets CPU 0
        #[cfg(feature = "esp32s3")]
        Some(SocResetReason::CpuRtcWdt) => {
            info!("reset_reason: CpuRtcWdt");
        }
        // VDD voltage is not stable and resets the digital core
        Some(SocResetReason::SysBrownOut) => {
            info!("reset_reason: SysBrownOut");
        }
        // RTC watch dog resets digital core and rtc module
        Some(SocResetReason::SysRtcWdt) => {
            info!("reset_reason: SysRtcWdt");
        }
        // Main watch dog 1 resets CPU 0
        #[cfg(feature = "esp32s3")]
        Some(SocResetReason::CpuMwdt1) => {
            info!("reset_reason: Cpu0Mwdt1");
        }
        // Super watch dog resets the digital core and rtc module
        #[cfg(any(feature = "esp32s3", feature = "esp32c6"))]
        Some(SocResetReason::SysSuperWdt) => {
            info!("reset_reason: SysSuperWdt");
        }
        // eFuse CRC error resets the digital core
        #[cfg(any(feature = "esp32s3", feature = "esp32c6"))]
        Some(SocResetReason::CoreEfuseCrc) => {
            info!("reset_reason: CoreEfuseCrc");
        }
        // USB UART resets the digital core
        #[cfg(any(feature = "esp32s3", feature = "esp32c6"))]
        Some(SocResetReason::CoreUsbUart) => {
            info!("reset_reason: CoreUsbUart");
        }
        // USB JTAG resets the digital core
        #[cfg(any(feature = "esp32s3", feature = "esp32c6"))]
        Some(SocResetReason::CoreUsbJtag) => {
            info!("reset_reason: CoreUsbJtag");
        }
        // // JTAG resets CPU
        #[cfg(feature = "esp32c6")]
        Some(SocResetReason::Cpu0JtagCpu) => {
            info!("reset_reason: Cpu0JtagCpu");
        }
        #[cfg(feature = "esp32s3")]
        Some(SocResetReason::SysClkGlitch) => {
            info!("reset_reason: SysClkGlitch");
        }
        #[cfg(feature = "esp32s3")]
        Some(SocResetReason::CorePwrGlitch) => {
            info!("reset_reason: CorePwrGlitch");
        }

        #[cfg(feature = "esp32")]
        Some(SocResetReason::CoreSdio) => {
            info!("reset_reason: CoreSdio");
        }
        #[cfg(feature = "esp32")]
        Some(SocResetReason::Cpu0Sw) => {
            info!("reset_reason: Cpu0Sw");
        }
        #[cfg(feature = "esp32")]
        Some(SocResetReason::Cpu0RtcWdt) => {
            info!("reset_reason: Cpu0RtcWdt");
        }
        #[cfg(feature = "esp32")]
        Some(SocResetReason::Cpu1Cpu0) => {
            info!("reset_reason: Cpu1Cpu0");
        }
        None => info!("reset_reason: None"),
    }
}
