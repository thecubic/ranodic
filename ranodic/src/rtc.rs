// ds323x::

use crate::{RTCREF, ntp::TIME_SYNCED};
use anyhow::{Result, anyhow};
use chrono::{DateTime, NaiveDateTime, TimeDelta};
use core::sync::atomic::Ordering;
use defmt::{error, info};
use ds323x::{DateTimeAccess, Ds323x, ic::DS3231, interface::I2cInterface};
use embassy_time::{Duration, Timer};
use esp_hal::{
    i2c::master::{Config, I2c},
    peripherals::*,
};

pub fn rtcread() -> Result<NaiveDateTime> {
    let mut rtcic = get_rtcic()?;
    let temp = rtcic.temperature().expect("couldn't read rtc temperature");
    info!("rtc says the temperature is {}", temp);

    match rtcic.enable() {
        Ok(_) => {
            info!("rtcread: RTC enabled");
        }
        Err(e) => {
            error!("rtcread: RTC enable error {}", e);
        }
    };
    match rtcic.datetime() {
        Ok(ndt) => Ok(ndt),
        Err(ds323x::Error::InvalidInputData) => Err(anyhow!("InvalidInputData")),
        Err(ds323x::Error::InvalidDeviceState) => Err(anyhow!("InvalidDeviceState")),
        Err(ds323x::Error::Comm(e)) => Err(anyhow::Error::msg(e)),
    }
}

pub async fn sys_to_ic() -> Result<()> {
    let mut rtcic = get_rtcic()?;

    let dts = {
        let r = &**RTCREF.get().await;
        DateTime::from_timestamp_micros(r.current_time_us().try_into().unwrap())
            .expect("couldn't create DateTime")
            .naive_utc()
    };

    match rtcic.set_datetime(&dts) {
        Ok(_) => Ok(()),
        Err(_e) => Err(anyhow!("couldn't set datetime")),
    }
}

pub fn micros_to_ic(micros: u64) -> Result<()> {
    let mut rtcic = get_rtcic()?;
    info!("micros_to_rtc: ds3231");

    let dts = {
        DateTime::from_timestamp_micros(micros as i64)
            .expect("couldn't create DateTime")
            .naive_utc()
    };
    info!("micros_to_rtc: dts");

    match rtcic.set_datetime(&dts) {
        Ok(_) => {
            info!("micros_to_rtc: set_datetime OK");
            Ok(())
        }
        Err(_e) => Err(anyhow!("couldn't set datetime")),
    }
}

pub async fn ic_to_sys() -> Result<()> {
    info!("ic_to_sys: dts");
    let mut rtcic = get_rtcic()?;
    info!("ic_to_sys: set_current_time_us");
    let sysrtc = &**RTCREF.get().await;
    sysrtc.set_current_time_us(
        rtcic
            .datetime()
            .expect("couldn't get datetime")
            .and_utc()
            .timestamp_micros()
            .try_into()
            .unwrap(),
    );
    TIME_SYNCED.store(true, Ordering::Relaxed);

    Ok(())
}

fn get_rtcic() -> Result<Ds323x<I2cInterface<I2c<'static, esp_hal::Async>>, DS3231>> {
    #[cfg(all(feature = "selfwire", not(feature = "esp32")))]
    return Ok(Ds323x::new_ds3231(
        I2c::new(unsafe { I2C0::steal() }, Config::default())
            .expect("couldn't init I2C")
            .with_sda(unsafe { GPIO6::steal() })
            .with_scl(unsafe { GPIO7::steal() })
            .into_async(),
    ));

    #[cfg(feature = "tidbyt")]
    return Ok(Ds323x::new_ds3231(
        I2c::new(unsafe { I2C0::steal() }, Config::default())
            .expect("couldn't init I2C")
            .with_sda(unsafe { GPIO13::steal() })
            .with_scl(unsafe { GPIO14::steal() })
            .into_async(),
    ));
}

const DESYNC_INTERVAL: u64 = 600;
const DESYNC_THRESHOLD: TimeDelta = TimeDelta::new(10, 0).unwrap();
#[embassy_executor::task]
#[cfg(feature = "rtcchip")]
pub async fn desync_failsafe() {
    loop {
        let mut rtcic = get_rtcic().expect("couldn't get RTC IC");
        let sysrtc = &**RTCREF.get().await;

        let systime = DateTime::from_timestamp_micros(sysrtc.current_time_us().try_into().unwrap())
            .expect("couldn't create DateTime");

        let ictime = rtcic.datetime().expect("couldn't get datetime").and_utc();

        // let delta = ictime - systime;
        if (ictime - systime).abs() > DESYNC_THRESHOLD {
            // bye
        }

        Timer::after(Duration::from_secs(DESYNC_INTERVAL)).await;
    }
}
