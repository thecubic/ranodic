use crate::RTCREF;
#[cfg(feature = "rtcchip")]
use crate::rtc::micros_to_ic;
use alloc::string::ToString;
use anyhow::anyhow;
use core::net::SocketAddr;
use core::sync::atomic::{AtomicBool, Ordering};
use defmt::debug;
#[cfg(feature = "defmt")]
use defmt::{error, info};
use embassy_net::IpAddress;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_time::Timer;
use esp_hal::ram;
use esp_hal::rtc_cntl::Rtc;
use jiff::Zoned;
use smoltcp::wire::DnsQueryType;
use sntpc::{
    NtpContext, NtpResult, NtpTimestampGenerator, NtpUdpSocket, sntp_process_response,
    sntp_send_request,
};

// const TZ: &str = env!("TZ");

// and this is how I learned the "fun" of nested macros
// https://github.com/rust-lang/rust/issues/90765

// fuckin' software, amirite
// const TIMEZONE: jiff::tz::TimeZone = jiff::tz::get!("PST8PDT,M3.2.0,M11.1.0");

pub async fn zgettimeofday() -> Zoned {
    gettimeofday().await.to_zoned(TIMEZONE)
}

pub async fn gettimeofday() -> jiff::Timestamp {
    jiff::Timestamp::from_microsecond(RTCREF.get().await.current_time_us() as i64)
        .unwrap_or(jiff::Timestamp::MIN)
}

pub const TIMEZONE: jiff::tz::TimeZone = jiff::tz::get!("PST8PDT");
const NTP_SERVER: &str = env!("NTP_SERVER");
const USEC_IN_SEC: u64 = 1_000_000;
const NTP_INTERVAL: u64 = 120;

pub static TIME_SYNCED: AtomicBool = AtomicBool::new(false);

#[ram(unstable(rtc_fast), unstable(persistent))]
static mut TICK: u64 = 0;
#[embassy_executor::task]
pub async fn tick_writer() {
    let rtc = &**RTCREF.get().await;
    loop {
        if TIME_SYNCED.load(Ordering::Relaxed) {
            unsafe { TICK = rtc.current_time_us() };
        }
        Timer::after_secs(1).await;
    }
}

#[derive(Clone, Copy)]
struct RtcTimestampGen<'a> {
    rtc: &'a Rtc<'a>,
    current_time_us: u64,
}

impl<'a> RtcTimestampGen<'a> {
    async fn new() -> Self {
        let rtcb = &**RTCREF.get().await;
        Self {
            rtc: rtcb,
            current_time_us: rtcb.current_time_us(),
        }
    }
}

impl NtpTimestampGenerator for RtcTimestampGen<'_> {
    fn init(&mut self) {
        self.current_time_us = self.rtc.current_time_us();
    }

    fn timestamp_sec(&self) -> u64 {
        self.current_time_us / 1_000_000
    }

    fn timestamp_subsec_micros(&self) -> u32 {
        (self.current_time_us % 1_000_000) as u32
    }
}

#[embassy_executor::task]
pub async fn ntp_sync(stack: embassy_net::Stack<'static>) {
    debug!("ntp_sync: TICK = {}", unsafe { TICK });
    debug!("ntp_sync started");
    let mut rx_meta = [PacketMetadata::EMPTY; 16];
    let mut rx_buffer = [0; 4096];
    let mut tx_meta = [PacketMetadata::EMPTY; 16];
    let mut tx_buffer = [0; 4096];
    let mut socket = UdpSocket::new(
        stack,
        &mut rx_meta,
        &mut rx_buffer,
        &mut tx_meta,
        &mut tx_buffer,
    );
    socket.bind(123).unwrap();
    let ntpctx = NtpContext::new(RtcTimestampGen::new().await);
    loop {
        stack.wait_config_up().await;
        debug!("ntp_sync network stack up");
        info!("ntp_sync: DNS");
        let addrs = match stack.dns_query(NTP_SERVER, DnsQueryType::A).await {
            Ok(e) => {
                if e.is_empty() {
                    error!("ntp_sync: empty addresses for {}", NTP_SERVER);
                    Timer::after_secs(NTP_INTERVAL / 10).await;
                    continue;
                }
                e
            }
            Err(e) => {
                error!("ntp_sync: DNS error: {}", e);
                Timer::after_secs(NTP_INTERVAL / 10).await;
                continue;
            }
        };
        info!("ntp_sync: get_time");
        match get_time(addrs, &socket, ntpctx).await {
            Ok(ntptime) => {
                info!("ntp_sync: ok response");

                #[cfg(feature = "rtcchip")]
                let _rtcchipres = micros_to_ic(
                    ntptime.sec() as u64 * USEC_IN_SEC
                        + sntpc::fraction_to_microseconds(ntptime.sec_fraction()) as u64,
                );
                RTCREF.get().await.set_current_time_us(
                    ntptime.sec() as u64 * USEC_IN_SEC
                        + sntpc::fraction_to_microseconds(ntptime.sec_fraction()) as u64,
                );
                TIME_SYNCED.store(true, Ordering::Relaxed);
            }
            Err(e) => {
                error!("ntp_sync: Error getting time: {:?}", e.to_string());
            }
        }
        Timer::after_secs(NTP_INTERVAL).await;
    }
}

const NTP_RETRIES: u64 = 5;
pub async fn get_time<U, T>(
    // addr: net::SocketAddr,
    addrs: heapless::Vec<IpAddress, { smoltcp::config::DNS_MAX_RESULT_COUNT }>,
    socket: &U,
    context: NtpContext<T>,
) -> Result<NtpResult, anyhow::Error>
where
    U: NtpUdpSocket,
    T: NtpTimestampGenerator + Copy,
{
    for retry in 0..NTP_RETRIES {
        for addr in addrs.iter() {
            let saddr = SocketAddr::from((*addr, 123));
            let req_result = sntp_send_request(saddr, socket, context).await;
            if req_result.is_err() {
                error!("sntp_send_request: {}", req_result.unwrap_err());
                Timer::after_secs(retry).await;
                continue;
            }
            info!("sntp_send_request: request sent");

            let resp = embassy_time::with_timeout(
                embassy_time::Duration::from_secs(2),
                sntp_process_response(saddr, socket, context, req_result.unwrap()),
            )
            .await;
            match resp {
                Ok(Ok(ntpr)) => {
                    return Ok(ntpr);
                }
                Err(toerr) => {
                    error!("get_time: attempt timeout {}", toerr);
                }
                Ok(Err(oerr)) => {
                    error!("get_time: other error {}", oerr);
                }
            }
        }
    }
    Err(anyhow!("get_time: exhausted retries"))
}
