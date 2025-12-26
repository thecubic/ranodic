use core::sync::atomic::Ordering;

use alloc::string::ToString;
#[cfg(feature = "defmt")]
use defmt::debug;
use jiff::{ToSpan, Zoned, civil::DateTime};
use num_enum::{TryFromPrimitive, TryFromPrimitiveError};

use crate::{ntp::zgettimeofday, weather::FORECASTS_PRESENT};

#[derive(Debug, Eq, PartialEq, TryFromPrimitive)]
#[repr(u8)]
pub enum WMOCode {
    ClearSky = 0,
    MainlyClear,
    PartlyCloudy,
    Overcast,
    Fog = 45,
    RimeFog = 48,
    LightDrizzle = 51,
    ModerateDrizzle = 53,
    DenseDrizzle = 55,
    LightFreezingDrizzle = 56,
    DenseFreezingDrizzle = 57,
    SlightRain = 61,
    ModerateRain = 63,
    HeavyRain = 65,
    LightFreezingRain = 66,
    HeavyFreezingRain = 67,
    LightSnow = 71,
    ModerateSnow = 73,
    HeavySnow = 75,
    SnowGrains = 77,
    LightShowers = 80,
    ModerateShowers = 81,
    HeavyShowers = 82,
    LightSnowShowers = 85,
    HeavySnowShowers = 86,
    LightThunderstorm = 95,
    ModerateHailThunderstorm = 96,
    HeavyHailThunderstorm = 99,
}

#[cfg(feature = "defmt")]
impl defmt::Format for WMOCode {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(fmt, "{}", self);
    }
}

impl TryFrom<u64> for WMOCode {
    type Error = TryFromPrimitiveError<WMOCode>;
    fn try_from(item: u64) -> Result<Self, Self::Error> {
        WMOCode::try_from_primitive(item as u8)
    }
}

#[derive(Debug)]
pub struct WeatherForecast {
    pub timespan: Zoned,
    pub temperature: f32,
    pub relative_humidity: u8,
    pub precipitation: f32,
    pub precipitation_probability: u8,
    pub weather_code: WMOCode,
    pub is_day: bool,
    pub sunshine_duration: f32,
}

impl Default for WeatherForecast {
    fn default() -> Self {
        Self {
            timespan: Default::default(),
            temperature: 32.0,
            relative_humidity: 100,
            precipitation: 6.5,
            precipitation_probability: 100,
            weather_code: WMOCode::ClearSky,
            is_day: false,
            sunshine_duration: 0.0,
        }
    }
}

impl WeatherForecast {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        time: &str,
        _timezone: &str,
        temperature: f32,
        relative_humidity: u8,
        precipitation: f32,
        precipitation_probability: u8,
        weather_code: WMOCode,
        is_day: bool,
        sunshine_duration: f32,
    ) -> anyhow::Result<Self> {
        let timespan: Zoned = time
            .parse::<DateTime>()
            .map_err(anyhow::Error::msg)?
            .to_zoned(crate::ntp::TIMEZONE)
            .map_err(anyhow::Error::msg)?;
        Ok(Self {
            timespan,
            temperature,
            relative_humidity,
            precipitation,
            precipitation_probability,
            weather_code,
            is_day,
            sunshine_duration,
        })
    }
    pub fn is_during(&self, timestamp: &Zoned) -> bool {
        timestamp >= &self.timespan && timestamp < &self.timespan + 1.hour()
    }
    pub fn is_prior(&self, timestamp: &Zoned) -> bool {
        timestamp < &self.timespan + 1.hour()
    }
}

#[cfg(feature = "defmt")]
impl defmt::Format for WeatherForecast {
    fn format(&self, fmt: defmt::Formatter) {
        defmt::write!(
            fmt,
            "WeatherForecast('{}', {}, {}, {}, {}, {}, {}, {})",
            self.timespan.to_string(),
            self.temperature,
            self.relative_humidity,
            self.precipitation,
            self.precipitation_probability,
            self.weather_code,
            self.is_day,
            self.sunshine_duration
        );
    }
}

impl PartialEq for WeatherForecast {
    fn eq(&self, other: &Self) -> bool {
        self.timespan == other.timespan
    }
}

impl PartialOrd for WeatherForecast {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        match self.timespan.partial_cmp(&other.timespan) {
            Some(core::cmp::Ordering::Equal) => Some(core::cmp::Ordering::Equal),
            ord => ord,
        }
    }
}

impl Eq for WeatherForecast {}

impl Ord for WeatherForecast {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.timespan.cmp(&other.timespan)
    }
}

pub struct WeatherForecastCache {
    forecasts: alloc::vec::Vec<WeatherForecast>,
}

impl Default for WeatherForecastCache {
    fn default() -> Self {
        Self::new()
    }
}

impl WeatherForecastCache {
    pub const fn new() -> Self {
        Self {
            forecasts: alloc::vec::Vec::new(),
        }
    }

    pub fn last_forecast(&self) -> Option<&WeatherForecast> {
        self.forecasts.iter().max()
    }

    pub fn forecasts_end_time(&self) -> Option<Zoned> {
        if let Some(forecast) = self.last_forecast() {
            Some(&forecast.timespan + 1.hour())
        } else {
            None
        }
    }

    pub fn first_forecast(&self) -> Option<&WeatherForecast> {
        self.forecasts.iter().min()
    }

    pub fn forecasts_start_time(&self) -> Option<Zoned> {
        if let Some(forecast) = self.first_forecast() {
            Some(forecast.timespan.clone())
        } else {
            None
        }
    }

    pub fn get_forecast(&self, timestamp: &Zoned) -> Option<&WeatherForecast> {
        self.forecasts
            .iter()
            .find(|forecast| forecast.is_during(timestamp))
    }

    pub async fn get_current_forecast(&self) -> Option<&WeatherForecast> {
        self.get_forecast(&zgettimeofday().await)
    }

    pub fn add(&mut self, forecast: WeatherForecast) {
        self.forecasts.push(forecast);
        if !FORECASTS_PRESENT.load(Ordering::Relaxed) {
            debug!("marking forecasts present");
            FORECASTS_PRESENT.store(true, Ordering::Relaxed);
        }
    }

    pub fn contains(&self, other: &WeatherForecast) -> bool {
        self.position(other).is_some()
    }

    pub fn position(&self, other: &WeatherForecast) -> Option<usize> {
        self.forecasts.iter().position(|forecast| forecast == other)
    }

    pub fn upsert(&mut self, forecast: WeatherForecast) {
        if let Some(idx) = self.position(&forecast) {
            self.forecasts.remove(idx);
        }
        self.add(forecast);
    }

    pub async fn expire(&mut self) {
        let timestamp = zgettimeofday().await;
        self.remove_before(&timestamp);
    }

    pub fn remove_before(&mut self, timestamp: &Zoned) {
        while let Some(idx) = self
            .forecasts
            .iter()
            .position(|forecast| forecast.is_prior(timestamp))
        {
            self.forecasts.remove(idx);
        }
    }

    pub fn between(&self, begin: &Zoned, end: &Zoned) -> alloc::vec::Vec<&WeatherForecast> {
        self.forecasts
            .iter()
            .filter(|x| &x.timespan >= begin && &x.timespan <= end)
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.forecasts.is_empty()
    }

    pub fn today(&self, timestamp: &Zoned) -> alloc::vec::Vec<&WeatherForecast> {
        self.between(
            &timestamp.start_of_day().unwrap(),
            &timestamp.end_of_day().unwrap(),
        )
    }

    pub fn daily_max_temp(&self, timestamp: &Zoned) -> Option<u8> {
        self.forecasts
            .iter()
            // .filter(|x| &x.timespan >= &timestamp.start_of_day().unwrap() && &x.timespan <= &timestamp.end_of_day().unwrap())
            .filter(|x| {
                &x.timespan >= &timestamp.start_of_day().unwrap()
                    && &x.timespan <= &timestamp.end_of_day().unwrap()
            })
            .map(|x| x.temperature as u8)
            .max()
    }

    pub fn day_or_night_max_temp(&self, timestamp: &Zoned, is_day: bool) -> Option<u8> {
        self.forecasts
            .iter()
            // .filter(|x| &x.timespan >= &timestamp.start_of_day().unwrap() && &x.timespan <= &timestamp.end_of_day().unwrap())
            .filter(|x| {
                &x.timespan >= &timestamp.start_of_day().unwrap()
                    && &x.timespan <= &timestamp.end_of_day().unwrap()
                    && x.is_day == is_day
            })
            .map(|x| x.temperature as u8)
            .max()
    }

    pub fn daily_min_temp(&self, timestamp: &Zoned) -> Option<u8> {
        self.forecasts
            .iter()
            // .filter(|x| &x.timespan >= &timestamp.start_of_day().unwrap() && &x.timespan <= &timestamp.end_of_day().unwrap())
            .filter(|x| {
                &x.timespan >= &timestamp.start_of_day().unwrap()
                    && &x.timespan <= &timestamp.end_of_day().unwrap()
            })
            .map(|x| x.temperature as u8)
            .min()
    }

    pub fn day_or_night_min_temp(&self, timestamp: &Zoned, is_day: bool) -> Option<u8> {
        self.forecasts
            .iter()
            // .filter(|x| &x.timespan >= &timestamp.start_of_day().unwrap() && &x.timespan <= &timestamp.end_of_day().unwrap())
            .filter(|x| {
                &x.timespan >= &timestamp.start_of_day().unwrap()
                    && &x.timespan <= &timestamp.end_of_day().unwrap()
                    && x.is_day == is_day
            })
            .map(|x| x.temperature as u8)
            .min()
    }

    // pub fn next_period(&self, timestamp: &Zoned, is_day: bool) {
    //     self.forecasts
    //         .iter()
    //         .filter(|x| &x.timespan >= begin && &x.timespan <= end)
    //         .collect()
    // }

    // pub fn tomorrow(&self, timestamp: &Zoned) -> alloc::vec::Vec<&WeatherForecast> {
    //     self.between(
    //         &timestamp.tomorrow().unwrap().start_of_day().unwrap(),
    //         &timestamp.tomorrow().unwrap().end_of_day().unwrap(),
    //     )
    // }
}
