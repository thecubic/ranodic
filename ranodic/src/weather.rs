use core::sync::atomic::{AtomicBool, AtomicU8};

use alloc::{format, string::ToString};
#[cfg(feature = "defmt")]
use defmt::{debug, error, info};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::Timer;

use itertools::izip;
use nanofish::HttpMethod;
use nanofish::{HttpHeader, ResponseBody, mime_types};

use serde_json::Value;

use anyhow::{Result, anyhow};

use crate::{
    forecast::{WeatherForecast, WeatherForecastCache},
    net::NET_REQUEST_QUEUE,
};

pub const WEATHER_URL: &str = "https://api.weather.gov/";
pub const WEATHER_LATITUDE: &str = env!("WEATHER_LATITUDE");
pub const WEATHER_LONGITUDE: &str = env!("WEATHER_LONGITUDE");

const FORECAST_SUCCESS_INTERVAL: u64 = 3600;
const FORECAST_FAILURE_INTERVAL: u64 = 60;

static BUFFER_SZ: usize = 8192;
// static BUFFER: Mutex<CriticalSectionRawMutex, [u8; BUFFER_SZ]> = Mutex::new([0u8; BUFFER_SZ]);

pub static FORECASTS: Mutex<CriticalSectionRawMutex, WeatherForecastCache> =
    Mutex::new(WeatherForecastCache::new());

pub static FORECASTS_PRESENT: AtomicBool = AtomicBool::new(false);

pub static QUICKTRIES: AtomicU8 = AtomicU8::new(3);

// static OMETEO_URL: OnceLock<String> = OnceLock::new();

#[embassy_executor::task]
pub async fn weather_query(stack: embassy_net::Stack<'static>) {
    debug!("weather_query alive");
    // let ometeo_url: Arc<String> = Arc::new(format!(
    //     "https://api.open-meteo.com/v1/forecast?\
    //     latitude={}&\
    //     longitude={}&\
    //     daily=sunrise,sunset,daylight_duration,sunshine_duration&\
    //     hourly=temperature_2m,relative_humidity_2m,precipitation,precipitation_probability,weather_code,is_day,sunshine_duration&\
    //     models=best_match&\
    //     timezone=America%2FLos_Angeles&\
    //     forecast_days=2&\
    //     wind_speed_unit=mph&\
    //     temperature_unit=fahrenheit&\
    //     precipitation_unit=inch",
    //     WEATHER_LATITUDE, WEATHER_LONGITUDE
    // ));
    loop {
        stack.wait_config_up().await;
        debug!("weather_query: network stack up");
        match get_forecasts(stack).await {
            Ok(()) => {
                Timer::after_secs(FORECAST_SUCCESS_INTERVAL).await;
            }
            Err(e) => {
                error!("weather_query: {}", e.to_string());
                let qts = QUICKTRIES.load(core::sync::atomic::Ordering::Relaxed);
                if qts > 0 {
                    info!("weather_query: quicktry");
                    Timer::after_secs(5).await;
                    QUICKTRIES.store(qts - 1, core::sync::atomic::Ordering::Relaxed);
                } else {
                    Timer::after_secs(FORECAST_FAILURE_INTERVAL).await;
                }
            }
        }
    }
}

async fn get_forecasts(stack: embassy_net::Stack<'static>) -> anyhow::Result<()> {
    // debug!("url: {}", url);
    // let mut buffer = BUFFER.lock().await;
    let mut buffer = [0u8; BUFFER_SZ];
    let _ = NET_REQUEST_QUEUE.lock().await;
    let (response, bytes_read) = crate::net::WorkingClient::new(&stack)
        .request(
            HttpMethod::GET,
            format!("https://api.open-meteo.com/v1/forecast?\
                            latitude={}&\
                            longitude={}&\
                            daily=sunrise,sunset,daylight_duration,sunshine_duration&\
                            hourly=temperature_2m,relative_humidity_2m,precipitation,precipitation_probability,weather_code,is_day,sunshine_duration&\
                            models=best_match&\
                            timezone=America%2FLos_Angeles&\
                            forecast_days=2&\
                            wind_speed_unit=mph&\
                            temperature_unit=fahrenheit&\
                            precipitation_unit=inch", WEATHER_LATITUDE, WEATHER_LONGITUDE).as_str(),
            &[
                HttpHeader::user_agent("ranodic/0.1"),
                HttpHeader::accept(mime_types::JSON),
            ],
            None,
            &mut buffer,
        )
        .await
        .map_err(anyhow::Error::msg)?;
    debug!("get_forecasts: bytes_read: {}", bytes_read);
    debug!(
        "get_forecasts: content length: {}",
        response.content_length()
    );
    debug!(
        "get_forecasts: response body length: {}",
        response.body.len()
    );
    if response.is_success() {
        if let ResponseBody::Text(jason) = response.body {
            digest_body(jason).await
        } else {
            error!("get_forecasts: unexpected response format");
            Err(anyhow!("get_forecasts: unexpected response format"))
        }
    } else {
        error!(
            "get_forecasts: HTTP response failure: HTTP {} {}: {}",
            response.status_code.as_u16(),
            response.status_code.text(),
            response.body.as_str().unwrap_or("<no body>"),
        );
        Err(anyhow!(
            "get_forecasts: HTTP response failure: HTTP {} {}: {}",
            response.status_code.as_u16(),
            response.status_code.text(),
            response.body.as_str().unwrap_or("<no body>"),
        ))
    }
}

async fn digest_body(jason: &str) -> Result<(), anyhow::Error> {
    // hmm it appears to have the length as a first line in hex
    let mut nline: bool = false;
    'inputlines: for line in jason.lines() {
        if !nline {
            // skip the first line which is suspect
            debug!("digest_body: skipping suspicious first line; next line");
            nline = true;
            continue;
        }
        debug!("subsequent lines");
        match serde_json::from_str(line) {
            Ok::<Value, _>(jobj) => {
                let timezone = if let Some(timezone) = jobj["timezone"].as_str() {
                    timezone
                } else {
                    // lots of stuff technically parses as JSON
                    debug!("digest_body: parsed JSON doesn't contain expected value; next line");
                    continue;
                };
                debug!("digest_body: data has timezone: {}", timezone);
                if let Some(hourly) = jobj["hourly"].as_object() {
                    if let (
                        Some(time),
                        Some(temperature_2m),
                        Some(relative_humidity_2m),
                        Some(precipitation),
                        Some(precipitation_probability),
                        Some(weather_code),
                        Some(is_day),
                        Some(sunshine_duration),
                    ) = (
                        hourly["time"].as_array(),
                        hourly["temperature_2m"].as_array(),
                        hourly["relative_humidity_2m"].as_array(),
                        hourly["precipitation"].as_array(),
                        hourly["precipitation_probability"].as_array(),
                        hourly["weather_code"].as_array(),
                        hourly["is_day"].as_array(),
                        hourly["sunshine_duration"].as_array(),
                    ) {
                        debug!("digest_body: deserialized successfully");
                        for pivot in izip!(
                            time,
                            temperature_2m,
                            relative_humidity_2m,
                            precipitation,
                            precipitation_probability,
                            weather_code,
                            is_day,
                            sunshine_duration
                        ) {
                            let forecast = if let (
                                Some(time),
                                Some(temperature),
                                Some(relative_humidity),
                                Some(precipitation),
                                Some(precipitation_probability),
                                Some(weather_code),
                                Some(is_day),
                                Some(sunshine_duration),
                            ) = (
                                pivot.0.as_str(),
                                pivot.1.as_f64(),
                                pivot.2.as_u64(),
                                pivot.3.as_f64(),
                                pivot.4.as_u64(),
                                pivot.5.as_u64(),
                                pivot.6.as_u64(),
                                pivot.7.as_f64(),
                            ) {
                                WeatherForecast::new(
                                    time,
                                    timezone,
                                    temperature as f32,
                                    relative_humidity.try_into().unwrap(),
                                    precipitation as f32,
                                    precipitation_probability.try_into().unwrap(),
                                    weather_code.try_into().unwrap(),
                                    is_day != 0,
                                    sunshine_duration as f32,
                                )
                            } else {
                                debug!(
                                    "0:{}, 1:{}, 2:{}, 3:{}, 4:{}, 5:{}, 6:{}, 7:{}",
                                    pivot.0.as_str(),
                                    pivot.1.as_f64(),
                                    pivot.2.as_u64(),
                                    pivot.3.as_f64(),
                                    pivot.4.as_u64(),
                                    pivot.5.as_u64(),
                                    pivot.6.as_u64(),
                                    pivot.7.as_f64(),
                                );
                                error!("digest_body: JSON parsing didn't work");
                                return Err(anyhow!("JSON parsing didn't work"));
                            };
                            if forecast.is_ok() {
                                FORECASTS.lock().await.upsert(forecast.unwrap());
                                // debug!("digest_body: upserted forecast");
                            } else {
                                error!(
                                    "digest_body: forecast bogus: {}",
                                    forecast.unwrap_err().to_string()
                                );
                                continue 'inputlines;
                            }
                        }
                        return Ok(());
                    }
                }
            }
            Err(e) => {
                use alloc::string::ToString;
                error!(
                    "digest_body: couldn't deserialize JSON: {}",
                    e.to_string().as_str()
                );
                continue;
            }
        }
    }
    Err(anyhow!("digest_body: never found data"))
}
