use core::sync::atomic::AtomicU8;

use alloc::{
    format,
    string::{String, ToString},
};
#[cfg(feature = "defmt")]
use defmt::{Formatter, debug, error, info, write};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};
use embassy_time::Timer;
use embedded_graphics::pixelcolor::RgbColor;
use esp_hub75::Color;
use serde_json::Value;

use nanofish::{HttpHeader, ResponseBody, mime_types};

use crate::{
    drawing::{ColorMonoTextStyle, bgfontbase},
    ntp::zgettimeofday,
};

const NIGHTSCOUT_TOKEN: &str = env!("NIGHTSCOUT_TOKEN");
const NIGHTSCOUT_URL: &str = env!("NIGHTSCOUT_URL");

pub static QUICKTRIES: AtomicU8 = AtomicU8::new(3);

pub const OKAY_FONT: ColorMonoTextStyle<'static> = bgfontbase()
    .text_color(Color::GREEN)
    .background_color(Color::BLACK)
    .build();

pub const HIGH_FONT: ColorMonoTextStyle<'static> = bgfontbase()
    .text_color(Color::YELLOW)
    .background_color(Color::BLACK)
    .build();

pub const SUPERHIGH_FONT: ColorMonoTextStyle<'static> = bgfontbase()
    .text_color(Color::BLACK)
    .background_color(Color::YELLOW)
    .build();

pub const LOW_FONT: ColorMonoTextStyle<'static> = bgfontbase()
    .text_color(Color::RED)
    .background_color(Color::BLACK)
    .build();

pub const SUPERLOW_FONT: ColorMonoTextStyle<'static> = bgfontbase()
    .text_color(Color::WHITE)
    .background_color(Color::RED)
    .build();

pub const STALE_FONT: ColorMonoTextStyle<'static> = bgfontbase()
    .text_color(Color::WHITE)
    .background_color(Color::BLUE)
    .build();

pub async fn get_style(bgreading: &BgReading) -> ColorMonoTextStyle<'static> {
    let now = zgettimeofday().await;
    let diffsecs = (&now - &bgreading.timestamp)
        .total(jiff::Unit::Second)
        .unwrap() as u64;
    if diffsecs > crate::nightscout::STALE_SECS {
        // trace!("datapoint is stale ({})", diffsecs);
        STALE_FONT
    } else if bgreading.bg <= 60 {
        // trace!("datapoint is superlow");
        SUPERLOW_FONT
    } else if bgreading.bg <= 75 {
        // trace!("datapoint is low");
        LOW_FONT
    } else if bgreading.bg >= 150 {
        // trace!("datapoint is high");
        HIGH_FONT
    } else if bgreading.bg >= 250 {
        // trace!("datapoint is superhigh");
        SUPERHIGH_FONT
    } else {
        // trace!("datapoint is okay");
        OKAY_FONT
    }
}

const STALE_SECS: u64 = 600;

#[derive(Clone, Debug)]
pub struct BgReading {
    pub bg: u64,
    pub units: String,
    pub timestamp: jiff::Zoned,
}

impl defmt::Format for BgReading {
    fn format(&self, fmt: Formatter) {
        write!(fmt, "BgReading({} {} @ ", self.bg, self.units.as_str());
        write!(fmt, "{})", self.timestamp.to_string().as_str());
    }
}

pub static BGDATA: Watch<CriticalSectionRawMutex, BgReading, 2> = Watch::new();
const BG_SUCCESS_INTERVAL: u64 = 150;
const BG_FAILURE_INTERVAL: u64 = 10;

#[embassy_executor::task]
pub async fn nightscout_query(stack: embassy_net::Stack<'static>) {
    debug!("nightscout_query alive");

    let sender = BGDATA.sender();
    let http_client = crate::net::WorkingClient::new(&stack);
    let fullurl = format!(
        "{}api/v1/entries/sgv.json?count=2&token={}",
        NIGHTSCOUT_URL, NIGHTSCOUT_TOKEN
    );
    let mut buffer = [0u8; 8192];
    loop {
        stack.wait_config_up().await;
        debug!("nightscout_query: network stack up");
        let _ = crate::net::NET_REQUEST_QUEUE.lock().await;
        let (response, _) = match http_client
            .request(
                nanofish::HttpMethod::GET,
                &fullurl,
                &[
                    HttpHeader::user_agent("ranodic/0.0"),
                    HttpHeader::accept(mime_types::JSON),
                ],
                None,
                &mut buffer,
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                error!("nightscout_query: request fail: {}", e);
                let qts = QUICKTRIES.load(core::sync::atomic::Ordering::Relaxed);
                if qts > 0 {
                    info!("nightscout_query: quicktry");
                    QUICKTRIES.store(qts - 1, core::sync::atomic::Ordering::Relaxed);
                    Timer::after_secs(5).await;
                }
                Timer::after_secs(BG_FAILURE_INTERVAL).await;
                continue;
            }
        };

        if !response.is_success() {
            error!("nightscout_query: is fail!");
            Timer::after_secs(BG_FAILURE_INTERVAL).await;
            continue;
        }

        if let ResponseBody::Text(jason) = response.body {
            let entries: Value = serde_json::from_str(jason).expect("valued");
            if let (Some(bgda), Some(bgu), Some(bgdt), Some(bgzo)) = (
                entries[0]["sgv"].as_u64(),
                entries[0]["units"].as_str(),
                entries[0]["date"].as_i64(),
                entries[0]["utcOffset"].as_i64(),
            ) {
                let reading = BgReading {
                    bg: bgda,
                    units: bgu.into(),
                    timestamp: jiff::Timestamp::from_millisecond(bgdt)
                        .expect("nightscout_query: couldn't timestamp")
                        .to_zoned(jiff::tz::TimeZone::fixed(
                            jiff::tz::Offset::from_seconds((bgzo * 60).try_into().unwrap())
                                .expect("nightscout_query: couldn't offset zone"),
                        )),
                };
                info!("nightscout_query: sent reading: {:?}", reading);
                sender.send(reading);
                Timer::after_secs(BG_SUCCESS_INTERVAL).await;
            } else {
                error!("nightscout_query: one of the columns wasn't defined");
                Timer::after_secs(BG_FAILURE_INTERVAL).await;
            };
        } else {
            error!("nightscout_query: unexpected response type");
            Timer::after_secs(BG_FAILURE_INTERVAL).await;
        }
    }
}
