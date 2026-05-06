use alloc::{format, string::ToString};
use core::any::{type_name, type_name_of_val};
use core::sync::atomic::{AtomicBool, AtomicU8};
use embedded_graphics::mono_font::ascii::{FONT_6X10, FONT_6X12};

use crate::log::{debug, error, info};
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

use embedded_graphics::primitives::StyledDrawable;

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
    let result = crate::net::WorkingClient::new(&stack)
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
        .await;
    if let Err(e) = result {
        match e {
            nanofish::Error::TlsError(te) => {
                match te {
                    embedded_tls::TlsError::HandshakeAborted(lvl, fl) => {
                        error!("TLS alert: {:?} {:?}", lvl, fl);
                    }
                    _ => {}
                }
                error!("result: <{:?}>{:?}", type_name_of_val(&te), te.to_string());
            }
            _ => {}
        }

        return Err(anyhow::Error::msg(e));
    }
    let (response, bytes_read) = result.unwrap();
    // (result, bytes_read)
    // if result.is_
    // let response =
    // .map_err(anyhow::Error::msg)?;
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
    for line in jason.lines() {
        info!("jason line:[{}]", line);
    }
    if let (Some(obrk), Some(cbrk)) = (jason.find('{'), jason.rfind('}')) {
        match serde_json::from_str(&jason[obrk..cbrk]) {
            Ok::<Value, _>(_jobj) => {
                info!("digest_body: got JSON");
            }
            Err(e) => {
                use alloc::string::ToString;
                error!(
                    "digest_body: couldn't deserialize JSON: {}",
                    e.to_string().as_str()
                );
            }
        }
    } else {
        error!("digest_body: hopeless; couldn't find brackets");
    }

    // hmm it appears to have the length as a first line in hex
    let mut nline: bool = false;
    'inputlines: for line in jason.lines() {
        if !nline {
            // skip the first line which is suspect
            debug!(
                "digest_body: skipping suspicious first line \"{}\"; next line",
                line
            );
            nline = true;
            continue;
        }
        debug!("subsequent line: {}", line);
        match serde_json::from_str(line) {
            Ok::<Value, _>(jobj) => {
                let timezone = if let Some(timezone) = jobj["timezone"].as_str() {
                    timezone
                } else {
                    // lots of stuff technically parses as JSON
                    debug!(
                        "digest_body: parsed JSON doesn't contain expected value; cur \"{}\"; next line",
                        line
                    );
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

use core::fmt::Write as _;

use embedded_graphics::{
    Drawable,
    draw_target::DrawTarget,
    geometry::{Point, Size},
    mono_font::{MonoTextStyle, ascii::FONT_5X8},
    pixelcolor::Rgb888,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Baseline, Text},
};
use heapless::String;
use jiff::Zoned;

// ─────────────────────────────────────────────────────────────────────────────
// Colour palette  (Rgb888 — 5R 6G 5B)
// ─────────────────────────────────────────────────────────────────────────────

pub mod palette {
    use embedded_graphics::pixelcolor::Rgb888;

    // Backgrounds
    pub const DAY_BG: Rgb888 = Rgb888::new(0, 32, 80); // muted sky blue
    pub const NIGHT_BG: Rgb888 = Rgb888::new(0, 0, 28); // deep navy

    // Text
    pub const WHITE: Rgb888 = Rgb888::new(255, 255, 255);
    pub const DIM: Rgb888 = Rgb888::new(120, 120, 120); // ~50 % white

    // Divider
    pub const DIVIDER: Rgb888 = Rgb888::new(40, 60, 90);

    // Temperature ramp
    pub const TEMP_COLD: Rgb888 = Rgb888::new(40, 180, 220); // icy cyan
    pub const TEMP_WARM: Rgb888 = Rgb888::new(240, 170, 30); // amber
    pub const TEMP_HOT: Rgb888 = Rgb888::new(230, 60, 20); // coral red

    // Icon accents
    pub const SUN: Rgb888 = Rgb888::new(255, 215, 0); // golden yellow
    pub const CLOUD: Rgb888 = Rgb888::new(160, 175, 190); // light grey-blue
    pub const FOG: Rgb888 = Rgb888::new(140, 150, 155); // pale grey
    pub const RAIN: Rgb888 = Rgb888::new(60, 130, 220); // sky blue
    pub const SNOW: Rgb888 = Rgb888::new(200, 230, 255); // near-white cool
    pub const THUNDER: Rgb888 = Rgb888::new(255, 240, 30); // lightning yellow

    // Row-1 data colours
    pub const HUMID: Rgb888 = Rgb888::new(30, 190, 160); // teal
    pub const PRECIP: Rgb888 = Rgb888::new(60, 140, 220); // rain blue
}

// ─────────────────────────────────────────────────────────────────────────────
// Layout constants
// ─────────────────────────────────────────────────────────────────────────────

const DISPLAY_W: i32 = 64;

/// First row owned by this renderer (top of the lower 16-row region).
const REGION_TOP: i32 = 16;

/// FONT_5X8: 5 px wide glyph + 1 px letter-spacing = 6 px per character.
const CHAR_W: i32 = 6;
/// FONT_5X8 glyph height.
const CHAR_H: i32 = 8;

/// Top of row 0 (icon + temperature), region-relative.
const ROW0_Y: i32 = 1;
/// Top of row 1 (humidity + precip), region-relative.  +1 clears the divider.
const ROW1_Y: i32 = CHAR_H;

// ─────────────────────────────────────────────────────────────────────────────
// Entry point
// ─────────────────────────────────────────────────────────────────────────────

/// Draw `forecast` into the lower 16 rows (rows 16–31) of the display.
///
/// Works with any `DrawTarget<Color = Rgb888>` — hardware LED matrices,
/// TFTs, the `embedded-graphics` simulator, `MockDisplay`, etc.
///
/// The upper 16 rows are left untouched.
///
/// # Errors
/// Propagates `DrawTarget::Error` from the underlying hardware driver.
pub fn draw_forecast<D>(
    now: Zoned,
    forecasts: &WeatherForecastCache,
    forecast: &WeatherForecast,
    target: &mut D,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb888>,
{
    // draw_background(forecast, target)?;
    draw_row0(now, forecasts, forecast, target)?;
    // draw_divider(target)?;
    draw_row1(forecast, target)?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Background
// ─────────────────────────────────────────────────────────────────────────────

fn draw_background<D: DrawTarget<Color = Rgb888>>(
    forecast: &WeatherForecast,
    target: &mut D,
) -> Result<(), D::Error> {
    let bg = if forecast.is_day {
        palette::DAY_BG
    } else {
        palette::NIGHT_BG
    };
    Rectangle::new(Point::new(0, REGION_TOP), Size::new(DISPLAY_W as u32, 16))
        .draw_styled(&PrimitiveStyle::with_fill(bg), target)
}

// ─────────────────────────────────────────────────────────────────────────────
// Row 0 — weather icon · temperature · day/night flag
//
// Character columns (6 px each, 64 px total = 10 cols):
//
//   col 0    col 1   col 2-5          col 9
//  ┌───────┬───────┬───────────────┬───────┐
//  │ icon  │  spc  │  "+23*C"      │  D/N  │
//  └───────┴───────┴───────────────┴───────┘
//
// "+23*C" is always 5 chars; icon + space = 2 chars → 7 total → 42 px,
// leaving col 8 free and D/N at col 9 (px 54).
// ─────────────────────────────────────────────────────────────────────────────

fn draw_row0<D: DrawTarget<Color = Rgb888>>(
    now: Zoned,
    forecasts: &WeatherForecastCache,
    forecast: &WeatherForecast,
    target: &mut D,
) -> Result<(), D::Error> {
    let y = REGION_TOP + ROW0_Y;

    let mut icon = forecast.weather_code.icon();
    let mut icon_color = forecast.weather_code.icon_color();
    let mut offset = 0;
    if icon == '*' && !forecast.is_day {
        icon = 'o';
        icon_color = palette::FOG;
        offset = 1;
    } else if icon == '~' {
        offset = -2;
    }
    Text::with_baseline(
        &icon.to_string(),
        Point::new(3 * CHAR_W - 2, y - offset),
        MonoTextStyle::new(&FONT_6X10, icon_color),
        Baseline::Top,
    )
    .draw(target)?;

    if let Some(relforecast) = forecasts.nextth(forecast, 1) {
        let mut icon = relforecast.weather_code.icon();
        let mut icon_color = relforecast.weather_code.icon_color();
        let mut offset = 0;
        if icon == '*' && !relforecast.is_day {
            icon = 'o';
            icon_color = palette::FOG;
            offset = 1;
        } else if icon == '~' {
            offset = -2;
        }
        Text::with_baseline(
            &icon.to_string(),
            Point::new(4 * CHAR_W - 2, y - offset),
            MonoTextStyle::new(&FONT_6X10, icon_color),
            Baseline::Top,
        )
        .draw(target)?;
    }

    if let Some(relforecast) = forecasts.nextth(forecast, 2) {
        let mut icon = relforecast.weather_code.icon();
        let mut icon_color = relforecast.weather_code.icon_color();
        let mut offset = 0;
        if icon == '*' && !relforecast.is_day {
            icon = 'o';
            icon_color = palette::FOG;
            offset = 1;
        } else if icon == '~' {
            offset = -2;
        }
        Text::with_baseline(
            &icon.to_string(),
            Point::new(5 * CHAR_W - 2, y - offset),
            MonoTextStyle::new(&FONT_6X10, icon_color),
            Baseline::Top,
        )
        .draw(target)?;
    }

    // Temperature (cols 2–6)
    // let mut temp_buf: String<8> = String::new();
    // format_temperature(&mut temp_buf, forecast.temperature);
    Text::with_baseline(
        &format!("{:2}F", forecast.temperature as i8),
        Point::new(0, y),
        MonoTextStyle::new(&FONT_5X8, temperature_color(forecast.temperature)),
        Baseline::Top,
    )
    .draw(target)?;

    let day_min = forecasts.day_or_night_min_temp(&now, true);
    let day_max = forecasts.day_or_night_max_temp(&now, true);
    let night_min = forecasts.day_or_night_min_temp(&now, false);
    let night_max = forecasts.day_or_night_max_temp(&now, false);

    let (day_row, night_row) = if forecast.is_day {
        (ROW0_Y, ROW1_Y)
    } else {
        (ROW1_Y, ROW0_Y)
    };
    // The Silence of Daylight
    // https://www.youtube.com/watch?v=H58lhgcc-nk

    // Monster Dance
    // https://www.youtube.com/watch?v=-HUwLA57paU

    if let (Some(temp_min), Some(temp_max)) = (day_min, day_max) {
        Text::with_baseline(
            &(temp_min as i8).to_string(),
            Point::new(6 * CHAR_W, REGION_TOP + day_row),
            MonoTextStyle::new(&FONT_5X8, temperature_color(temp_min as f32)),
            Baseline::Top,
        )
        .draw(target)?;
        Text::with_baseline(
            &"*",
            Point::new(8 * CHAR_W - 1, REGION_TOP + day_row),
            MonoTextStyle::new(&FONT_6X10, palette::SUN),
            Baseline::Top,
        )
        .draw(target)?;
        Text::with_baseline(
            &(temp_max as i8).to_string(),
            Point::new(9 * CHAR_W, REGION_TOP + day_row),
            MonoTextStyle::new(&FONT_5X8, temperature_color(temp_min as f32)),
            Baseline::Top,
        )
        .draw(target)?;
    }
    if let (Some(temp_min), Some(temp_max)) = (night_min, night_max) {
        Text::with_baseline(
            &(temp_min as i8).to_string(),
            Point::new(6 * CHAR_W, REGION_TOP + night_row),
            MonoTextStyle::new(&FONT_5X8, temperature_color(temp_min as f32)),
            Baseline::Top,
        )
        .draw(target)?;
        Text::with_baseline(
            &"o",
            Point::new(8 * CHAR_W - 1, REGION_TOP + night_row - 1),
            MonoTextStyle::new(&FONT_6X10, palette::FOG),
            Baseline::Top,
        )
        .draw(target)?;
        Text::with_baseline(
            &(temp_min as i8).to_string(),
            Point::new(9 * CHAR_W, REGION_TOP + night_row),
            MonoTextStyle::new(&FONT_5X8, temperature_color(temp_min as f32)),
            Baseline::Top,
        )
        .draw(target)?;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Divider  (1 px line between the two text rows)
// ─────────────────────────────────────────────────────────────────────────────

fn draw_divider<D: DrawTarget<Color = Rgb888>>(target: &mut D) -> Result<(), D::Error> {
    Rectangle::new(
        Point::new(0, REGION_TOP + CHAR_H),
        Size::new(DISPLAY_W as u32, 1),
    )
    .draw_styled(&PrimitiveStyle::with_fill(palette::DIVIDER), target)
}

fn draw_row1<D: DrawTarget<Color = Rgb888>>(
    forecast: &WeatherForecast,
    target: &mut D,
) -> Result<(), D::Error> {
    let y = REGION_TOP + ROW1_Y;

    // humidity
    // let humid_text = format!("{:2}%",);
    Text::with_baseline(
        &forecast.relative_humidity.min(99).to_string(),
        Point::new(0, y),
        MonoTextStyle::new(&FONT_5X8, palette::HUMID),
        Baseline::Top,
    )
    .draw(target)?;
    Text::with_baseline(
        &"%",
        Point::new(2 * CHAR_W - 2, y),
        MonoTextStyle::new(&FONT_6X10, palette::HUMID),
        Baseline::Top,
    )
    .draw(target)?;

    // precipitation
    // let precip_text = format!("{:2}%", forecast.precipitation_probability.min(99));
    Text::with_baseline(
        &forecast.precipitation_probability.min(99).to_string(),
        Point::new(3 * CHAR_W, y),
        MonoTextStyle::new(&FONT_5X8, palette::PRECIP),
        Baseline::Top,
    )
    .draw(target)?;
    Text::with_baseline(
        &"%",
        Point::new(4 * CHAR_W, y),
        MonoTextStyle::new(&FONT_6X10, palette::PRECIP),
        Baseline::Top,
    )
    .draw(target)?;

    // // Optional precipitation amount — fits only when combined length < 10 cols
    // let used_chars = (hum_buf.len() + pp_buf.len()) as i32;
    // let spare_cols = (DISPLAY_W / CHAR_W) - used_chars; // typically 0

    // if spare_cols >= 3 && forecast.precipitation > 0.0 {
    //     let mut mm_buf: String<6> = String::new();
    //     format_mm(&mut mm_buf, forecast.precipitation, spare_cols as usize);
    //     Text::with_baseline(
    //         &mm_buf,
    //         Point::new(used_chars * CHAR_W, y),
    //         MonoTextStyle::new(&FONT_5X8, palette::DIM),
    //         Baseline::Top,
    //     )
    //     .draw(target)?;
    // }

    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Formatting helpers  (heapless — no heap allocation)
// ─────────────────────────────────────────────────────────────────────────────

/// Writes e.g. `"H:72%"` or `"P: 5%"` — always exactly 5 characters.
fn format_percent(buf: &mut String<6>, prefix: char, value: u8) {
    let v = value.min(100);
    let _ = write!(buf, "{prefix}:{v:2}%");
}

/// Writes temperature as `"+23*C"` or `"-05*C"` — always 5 characters.
///
/// `°` is absent from FONT_5X8; `*` is used as the degree symbol.
fn format_temperature(buf: &mut String<8>, celsius: f32) {
    let rounded = libm::roundf(celsius) as i32;
    let sign = if rounded > 0 {
        '+'
    } else if rounded < 0 {
        '-'
    } else {
        ' '
    };
    let abs = rounded.unsigned_abs();
    let _ = write!(buf, "{sign}{abs:02}*C");
}

/// Writes precipitation amount fitting in `max_chars` columns.
fn format_mm(buf: &mut String<6>, mm: f32, max_chars: usize) {
    let _ = if max_chars >= 5 {
        write!(buf, "{mm:.1}mm")
    } else if max_chars >= 4 {
        write!(buf, "{}mm", libm::roundf(mm) as u32)
    } else {
        write!(buf, "{}m", libm::roundf(mm) as u32)
    };
}

// ─────────────────────────────────────────────────────────────────────────────
// Temperature → Rgb888 colour gradient
// ─────────────────────────────────────────────────────────────────────────────

/// Maps temperature to a perceptual colour:
///
/// ```text
///  ≤  0 °C  →  icy cyan
///    15 °C  →  white
///    30 °C  →  warm amber
///  ≥ 40 °C  →  hot coral
/// ```
fn temperature_color(fahrenheit: f32) -> Rgb888 {
    if fahrenheit <= 32.0 {
        palette::TEMP_COLD
    } else if fahrenheit <= 60.0 {
        lerp_color(
            palette::TEMP_COLD,
            palette::WHITE,
            (fahrenheit - 32.0) / 20.0,
        )
    } else if fahrenheit <= 80.0 {
        lerp_color(
            palette::WHITE,
            palette::TEMP_WARM,
            (fahrenheit - 60.0) / 20.0,
        )
    } else {
        lerp_color(
            palette::TEMP_WARM,
            palette::TEMP_HOT,
            ((fahrenheit - 80.0) / 10.0).min(1.0),
        )
    }
}

/// Linear interpolation between two `Rgb888` colours; `t` in `[0.0, 1.0]`.
fn lerp_color(a: Rgb888, b: Rgb888, t: f32) -> Rgb888 {
    use embedded_graphics::pixelcolor::RgbColor;
    let l = |lo: u8, hi: u8| libm::roundf(lo as f32 + (hi as f32 - lo as f32) * t) as u8;
    Rgb888::new(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::mock_display::MockDisplay;

    fn make_forecast(is_day: bool, temp: f32, hum: u8, pp: u8, prec: f32) -> WeatherForecast {
        WeatherForecast {
            timespan: Zoned::now(),
            temperature: temp,
            relative_humidity: hum,
            precipitation: prec,
            precipitation_probability: pp,
            weather_code: WMOCode::PartlyCloudy,
            is_day,
            sunshine_duration: 18_720.0,
        }
    }

    #[test]
    fn renders_day_no_error() {
        let mut display: MockDisplay<Rgb888> = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);
        draw_forecast(&make_forecast(true, 23.0, 72, 30, 0.4), &mut display).unwrap();
    }

    #[test]
    fn renders_night_no_error() {
        let mut display: MockDisplay<Rgb888> = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);
        draw_forecast(&make_forecast(false, -3.0, 88, 60, 1.2), &mut display).unwrap();
    }

    #[test]
    fn temp_format_positive() {
        let mut buf: String<8> = String::new();
        format_temperature(&mut buf, 23.4);
        assert_eq!(buf.as_str(), "+23*C");
    }

    #[test]
    fn temp_format_negative() {
        let mut buf: String<8> = String::new();
        format_temperature(&mut buf, -5.7);
        assert_eq!(buf.as_str(), "-06*C");
    }

    #[test]
    fn temp_format_zero() {
        let mut buf: String<8> = String::new();
        format_temperature(&mut buf, 0.0);
        assert_eq!(buf.as_str(), " 00*C");
    }

    #[test]
    fn percent_format_pads_single_digit() {
        let mut buf: String<6> = String::new();
        format_percent(&mut buf, 'H', 5);
        assert_eq!(buf.as_str(), "H: 5%");
    }

    #[test]
    fn percent_format_three_digits() {
        let mut buf: String<6> = String::new();
        format_percent(&mut buf, 'P', 100);
        assert_eq!(buf.as_str(), "P:100%"); // 6 chars — acceptable overflow case
    }
}
