use core::sync::atomic::Ordering;

use alloc::string::ToString;
use defmt::error;
#[cfg(feature = "defmt")]
use defmt::info;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, lazy_lock::LazyLock, signal::Signal,
    watch::Receiver,
};
use embassy_time::Timer;
use embedded_graphics::{
    Drawable,
    geometry::Point,
    image::ImageDrawable,
    mono_font::{
        MonoTextStyle,
        MonoTextStyleBuilder,
        ascii::{FONT_4X6, FONT_5X7, FONT_5X8, FONT_6X10, FONT_7X13},
        // iso_8859_1::FONT_5X8,
    },
    pixelcolor::Rgb888,
    prelude::RgbColor,
    text::{Alignment, Text},
};
use esp_hub75::Color;
use static_cell::StaticCell;
use tinygif::Gif;

use crate::{
    hub75::FBType,
    nightscout::{BGDATA, BgReading, get_style},
    ntp::{TIME_SYNCED, zgettimeofday},
    weather::{FORECASTS, FORECASTS_PRESENT},
};
use jiff::ToSpan;

pub type FrameBufferExchange = Signal<CriticalSectionRawMutex, &'static mut FBType>;
// const CLOCKPOINT: Point = Point::new(0, 7);

// static SHOWME: LazyLock<Gif> =
//     LazyLock::new(|| Gif::<Rgb888>::from_slice(include_bytes!("../../assets/showme.gif")).unwrap());

static LOGO: LazyLock<Gif> = LazyLock::new(|| {
    Gif::<Rgb888>::from_slice(include_bytes!("../../assets/ranodiclogo.gif")).unwrap()
});

// the painter sends the last painted frame down this signal
pub static FB_XMIT: FrameBufferExchange = FrameBufferExchange::new();
// the xmitter sends the obsolete fb back
pub static FB_PAINT: FrameBufferExchange = FrameBufferExchange::new();

pub static FB0: StaticCell<FBType> = StaticCell::new();
pub static FB1: StaticCell<FBType> = StaticCell::new();

// everything: "%a %b %d %y\n%H:%M:%S"
const DATE_FMT: &str = "%a %b %d";
const TIME_FMT: &str = "%H:%M";
const TIME_BLINK_FMT: &str = "%H %M";

async fn past_logo(bgrecvr: &Receiver<'_, CriticalSectionRawMutex, BgReading, 2>) -> bool {
    // soft internet invariant
    TIME_SYNCED.load(Ordering::Relaxed)
        || bgrecvr.contains_value()
        || FORECASTS_PRESENT.load(Ordering::Relaxed)
}

#[embassy_executor::task]
pub async fn display_painter(fb_inc: &'static mut FBType) {
    info!("display painter started");
    let mut bgrecvr = BGDATA.receiver().expect("couldn't get BGDATA recvr");
    let mut fb = fb_inc;
    let mac_address = crate::MAC_ADDRESS.get().await;
    let mac_str = alloc::format!(
        "{:02x}{:02x}{:02x}",
        mac_address[3],
        mac_address[4],
        mac_address[5]
    );
    fb.erase();
    'logo: loop {
        for frame in LOGO.get().frames() {
            frame.draw(fb).unwrap();
            Text::with_alignment(&mac_str, MACPOINT, SMOLFONT, Alignment::Left)
                .draw(fb)
                .expect("failed to draw text");
            Text::with_alignment(
                env!("CARGO_PKG_VERSION"),
                VERSPOINT,
                SMOLFONT,
                Alignment::Right,
            )
            .draw(fb)
            .expect("failed to draw text");
            FB_XMIT.signal(fb);
            fb = FB_PAINT.wait().await;
            fb.erase();
            if past_logo(&bgrecvr).await {
                break 'logo;
            }
            Timer::after_millis(frame.delay_centis as u64 * 10).await;
        }
    }
    // bug: sometimes it forgets
    let mut was_time_ever_synced = false;
    loop {
        was_time_ever_synced = was_time_ever_synced || TIME_SYNCED.load(Ordering::Relaxed);
        if crate::GRACEFUL_SHUTDOWN.load(Ordering::Relaxed) {
            info!("blanking buffers");
            fb.erase();
            FB_XMIT.signal(fb);
            fb = FB_PAINT.wait().await;
            fb.erase();
            break;
        }
        let now = zgettimeofday().await;
        if was_time_ever_synced {
            Text::with_alignment(
                now.strftime(DATE_FMT).to_string().as_str(),
                DATEPOINT,
                DATEFONTB,
                Alignment::Center,
            )
            .draw(fb)
            .expect("failed to draw text");

            Text::with_alignment(
                now.strftime(if now.millisecond() < 500 {
                    TIME_FMT
                } else {
                    TIME_BLINK_FMT
                })
                .to_string()
                .as_str(),
                TIMEPOINT,
                TIMEFONT,
                Alignment::Left,
            )
            .draw(fb)
            .expect("failed to draw text");
        }
        if bgrecvr.contains_value() {
            let bgreading = bgrecvr.get().await;
            let character_style = get_style(&bgreading).await;
            Text::with_alignment(
                bgreading.bg.to_string().as_str(),
                BGPOINT,
                character_style,
                Alignment::Right,
            )
            .draw(fb)
            .expect("failed to draw text");
        }
        if was_time_ever_synced && FORECASTS_PRESENT.load(Ordering::Relaxed) {
            // debug!("drawing, forecasts present");
            let forecasts = FORECASTS.lock().await;
            let mut is_day = true;
            if let Some(forecast) = forecasts.get_forecast(&now) {
                is_day = forecast.is_day;
                Text::with_alignment(
                    (forecast.temperature as u8).to_string().as_str(),
                    TEMPPOINT,
                    BIGFONT,
                    Alignment::Left,
                )
                .draw(fb)
                .expect("failed to draw text");
            } else {
                error!("no relevant forecast in cache");
            }
            if let Some(mintemp) = forecasts.day_or_night_min_temp(&now, is_day) {
                Text::with_alignment(
                    mintemp.to_string().as_str(),
                    TEMP_LOW_POINT,
                    DATEFONT,
                    Alignment::Left,
                )
                .draw(fb)
                .expect("failed to draw text");
            }
            if let Some(maxtemp) = forecasts.day_or_night_max_temp(&now, is_day) {
                Text::with_alignment(
                    maxtemp.to_string().as_str(),
                    TEMP_HIGH_POINT,
                    DATEFONT,
                    Alignment::Left,
                )
                .draw(fb)
                .expect("failed to draw text");
            }
            if let Some(mintemp) = forecasts.day_or_night_min_temp(&(&now + 12.hours()), !is_day) {
                Text::with_alignment(
                    mintemp.to_string().as_str(),
                    NEXT_TEMP_LOW_POINT,
                    DATEFONT,
                    Alignment::Left,
                )
                .draw(fb)
                .expect("failed to draw text");
            }
            if let Some(maxtemp) = forecasts.day_or_night_max_temp(&(&now + 12.hours()), !is_day) {
                Text::with_alignment(
                    maxtemp.to_string().as_str(),
                    NEXT_TEMP_HIGH_POINT,
                    DATEFONT,
                    Alignment::Left,
                )
                .draw(fb)
                .expect("failed to draw text");
            }
        }

        FB_XMIT.signal(fb);
        fb = FB_PAINT.wait().await;
        fb.erase();
        Timer::after_millis(50).await;
    }
    info!("display_painter: terminating");
}

pub enum DrawEvent {
    Clock,
}

pub type ColorMonoTextStyle<'a> = MonoTextStyle<'a, Rgb888>;

const DATEPOINT: Point = Point::new(32, 6);

const TIMEPOINT: Point = Point::new(0, 15);
const BGPOINT: Point = Point::new(64, 15);

const VERSPOINT: Point = Point::new(64, 31);
const MACPOINT: Point = Point::new(0, 31);

const TEMPPOINT: Point = Point::new(27, 28);

const TEMP_LOW_POINT: Point = Point::new(1, 23);
const TEMP_HIGH_POINT: Point = Point::new(54, 23);

const NEXT_TEMP_LOW_POINT: Point = Point::new(1, 30);
const NEXT_TEMP_HIGH_POINT: Point = Point::new(54, 30);

const DATEFONT: ColorMonoTextStyle<'static> = MonoTextStyleBuilder::new()
    .font(&FONT_5X7)
    .text_color(Color::WHITE)
    .background_color(Color::BLACK)
    .build();

const DATEFONTB: ColorMonoTextStyle<'static> = MonoTextStyleBuilder::new()
    .font(&FONT_5X8)
    .text_color(Color::WHITE)
    .background_color(Color::BLACK)
    .build();

const TIMEFONT: ColorMonoTextStyle<'static> = MonoTextStyleBuilder::new()
    .font(&FONT_6X10)
    .text_color(Color::WHITE)
    .background_color(Color::BLACK)
    .build();

pub const fn bgfontbase() -> MonoTextStyleBuilder<'static, Rgb888> {
    MonoTextStyleBuilder::new().font(&FONT_6X10)
}

const SMOLFONT: ColorMonoTextStyle<'static> = MonoTextStyleBuilder::new()
    .font(&FONT_4X6)
    .text_color(Color::WHITE)
    .background_color(Color::BLACK)
    .build();

const BIGFONT: ColorMonoTextStyle<'static> = MonoTextStyleBuilder::new()
    .font(&FONT_7X13)
    .text_color(Color::WHITE)
    .background_color(Color::BLACK)
    .build();
