#[cfg(feature = "defmt")]
use defmt::info;
use embassy_time::Timer;
#[cfg(feature = "esp32")]
use esp_hal::i2s::AnyI2s;

use esp_hal::gpio::AnyPin;
use esp_hal::peripherals::*;
use esp_hal::time::Rate;
use esp_hub75::{
    Hub75, Hub75Pins16,
    framebuffer::{compute_frame_count, compute_rows, plain::DmaFrameBuffer},
};

use crate::drawing::{FB_PAINT, FB_XMIT};

const ROWS: usize = 32;
const COLS: usize = 64;
// BUG: as of esp_hal 1.0.0 4 bits now murders the stack
// i should ask for a const DmaFramebuffer::new

const BRIGHTNESS_BITS: u8 = 3;

const NROWS: usize = compute_rows(ROWS);
const FRAME_COUNT: usize = compute_frame_count(BRIGHTNESS_BITS);
pub type FBType = DmaFrameBuffer<ROWS, COLS, NROWS, BRIGHTNESS_BITS, FRAME_COUNT>;
pub type Hub75Type<'d> = Hub75<'d, esp_hal::Async>;

pub struct DisplayPeripherals<'d> {
    #[cfg(feature = "esp32")]
    i2s: AnyI2s<'d>,
    #[cfg(feature = "esp32c6")]
    parl_io: PARL_IO<'d>,
    #[cfg(feature = "esp32s3")]
    lcd_cam: LCD_CAM<'d>,
    #[cfg(any(feature = "esp32s3", feature = "esp32c6"))]
    dma_channel: DMA_CH0<'d>,
    #[cfg(feature = "esp32")]
    dma_channel: DMA_I2S0<'d>,
    red1: AnyPin<'d>,
    grn1: AnyPin<'d>,
    blu1: AnyPin<'d>,
    red2: AnyPin<'d>,
    grn2: AnyPin<'d>,
    blu2: AnyPin<'d>,
    addr0: AnyPin<'d>,
    addr1: AnyPin<'d>,
    addr2: AnyPin<'d>,
    addr3: AnyPin<'d>,
    addr4: AnyPin<'d>,
    blank: AnyPin<'d>,
    clock: AnyPin<'d>,
    latch: AnyPin<'d>,
}

impl<'d> Default for DisplayPeripherals<'d> {
    fn default() -> Self {
        #[cfg(all(feature = "esp32c6", feature = "selfwire"))]
        return DisplayPeripherals::esp32c6_selfwire();

        #[cfg(all(feature = "esp32s3", feature = "selfwire"))]
        return DisplayPeripherals::esp32s3_selfwire();

        #[cfg(all(feature = "esp32", feature = "tidbyt"))]
        return DisplayPeripherals::esp32_tidbyt();
    }
}

impl<'d> DisplayPeripherals<'d> {
    #[cfg(feature = "tidbyt")]
    fn esp32_tidbyt() -> Self {
        unsafe {
            Self {
                i2s: I2S0::steal().into(),
                dma_channel: DMA_I2S0::steal(),
                red1: GPIO21::steal().into(),
                grn1: GPIO2::steal().into(),
                blu1: GPIO22::steal().into(),
                red2: GPIO23::steal().into(),
                grn2: GPIO4::steal().into(),
                blu2: GPIO27::steal().into(),
                addr0: GPIO26::steal().into(),
                addr1: GPIO5::steal().into(),
                addr2: GPIO25::steal().into(),
                addr3: GPIO18::steal().into(),
                addr4: GPIO14::steal().into(),
                blank: GPIO32::steal().into(),
                clock: GPIO33::steal().into(),
                latch: GPIO19::steal().into(),
            }
        }
    }

    #[cfg(feature = "esp32c6")]
    fn esp32c6_selfwire() -> Self {
        Self {
            parl_io: unsafe { PARL_IO::steal() },
            dma_channel: unsafe { DMA_CH0::steal() },
            #[cfg(not(feature = "whack"))]
            grn2: unsafe { GPIO18::steal().into() },
            #[cfg(not(feature = "whack"))]
            red1: unsafe { GPIO19::steal().into() },
            #[cfg(not(feature = "whack"))]
            blu1: unsafe { GPIO20::steal().into() },
            #[cfg(not(feature = "whack"))]
            grn1: unsafe { GPIO21::steal().into() },
            #[cfg(not(feature = "whack"))]
            red2: unsafe { GPIO22::steal().into() },
            #[cfg(not(feature = "whack"))]
            blu2: unsafe { GPIO23::steal().into() },

            #[cfg(feature = "whack")]
            blu2: unsafe { GPIO18::steal().into() },
            #[cfg(feature = "whack")]
            red1: unsafe { GPIO19::steal().into() },
            #[cfg(feature = "whack")]
            grn1: unsafe { GPIO20::steal().into() },
            #[cfg(feature = "whack")]
            blu1: unsafe { GPIO21::steal().into() },
            #[cfg(feature = "whack")]
            red2: unsafe { GPIO22::steal().into() },
            #[cfg(feature = "whack")]
            grn2: unsafe { GPIO23::steal().into() },

            addr0: unsafe { GPIO9::steal().into() },
            addr1: unsafe { GPIO2::steal().into() },
            addr2: unsafe { GPIO1::steal().into() },
            addr3: unsafe { GPIO0::steal().into() },
            addr4: unsafe { GPIO15::steal().into() },
            blank: unsafe { GPIO3::steal().into() },
            // clock: unsafe { GPIO11::steal().into() },
            // latch: unsafe { GPIO10::steal().into() },
            clock: unsafe { GPIO13::steal().into() },
            latch: unsafe { GPIO12::steal().into() },
        }
    }

    #[cfg(feature = "esp32s3")]
    fn esp32s3_selfwire() -> Self {
        Self {
            lcd_cam: unsafe { LCD_CAM::steal().into() },
            dma_channel: unsafe { DMA_CH0::steal() },

            // 5 b2
            // 6 g2
            // 7 r2
            // 15 gnd
            // 16 b1
            // 17 g1
            // 18 r1

            // GND . blue
            // OE . green
            blank: unsafe { GPIO38::steal().into() },
            // LATCH . yellow
            latch: unsafe { GPIO39::steal().into() },
            // CLOCK . orange
            clock: unsafe { GPIO40::steal().into() },

            // D . red
            addr3: unsafe { GPIO41::steal().into() },
            // C . brown
            addr2: unsafe { GPIO42::steal().into() },
            // B . black
            addr1: unsafe { GPIO2::steal().into() },
            // A . white
            addr0: unsafe { GPIO1::steal().into() },

            // E . gray
            addr4: unsafe { GPIO4::steal().into() },

            // violet blue12 5
            #[cfg(not(feature = "whack"))]
            blu2: unsafe { GPIO5::steal().into() },
            #[cfg(feature = "whack")]
            grn2: unsafe { GPIO5::steal().into() },
            // blue green12 6
            #[cfg(not(feature = "whack"))]
            grn2: unsafe { GPIO6::steal().into() },
            #[cfg(feature = "whack")]
            blu2: unsafe { GPIO6::steal().into() },
            // green red12 7
            #[cfg(not(feature = "whack"))]
            red2: unsafe { GPIO7::steal().into() },
            #[cfg(feature = "whack")]
            red2: unsafe { GPIO7::steal().into() },
            // yellow gnd 8

            // orange blue01
            #[cfg(feature = "whack")]
            grn1: unsafe { GPIO16::steal().into() },
            #[cfg(not(feature = "whack"))]
            blu1: unsafe { GPIO16::steal().into() },
            // red green01
            #[cfg(not(feature = "whack"))]
            grn1: unsafe { GPIO17::steal().into() },
            #[cfg(feature = "whack")]
            blu1: unsafe { GPIO17::steal().into() },
            // brown red01
            #[cfg(not(feature = "whack"))]
            red1: unsafe { GPIO18::steal().into() },
            #[cfg(feature = "whack")]
            red1: unsafe { GPIO18::steal().into() },
            // 4 e

            // 37 gnd oe
            // 38 oe
            // 39 lat
            // 40 clk
            // 41 d
            // 42 c
            // 2 b
            // 1 a
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        #[cfg(feature = "esp32c6")] parl_io: PARL_IO<'d>,
        #[cfg(feature = "esp32s3")] lcd_cam: LCD_CAM<'d>,
        #[cfg(any(feature = "esp32s3", feature = "esp32c6"))] dma_channel: DMA_CH0<'d>,
        #[cfg(feature = "esp32")] i2s: AnyI2s<'d>,
        #[cfg(feature = "esp32")] dma_channel: DMA_I2S0<'d>,
        red1: AnyPin<'d>,
        grn1: AnyPin<'d>,
        blu1: AnyPin<'d>,
        red2: AnyPin<'d>,
        grn2: AnyPin<'d>,
        blu2: AnyPin<'d>,
        addr0: AnyPin<'d>,
        addr1: AnyPin<'d>,
        addr2: AnyPin<'d>,
        addr3: AnyPin<'d>,
        addr4: AnyPin<'d>,
        blank: AnyPin<'d>,
        clock: AnyPin<'d>,
        latch: AnyPin<'d>,
    ) -> Self {
        Self {
            #[cfg(feature = "esp32c6")]
            parl_io,
            #[cfg(feature = "esp32s3")]
            lcd_cam,
            #[cfg(feature = "esp32")]
            i2s,
            dma_channel,
            red1,
            grn1,
            blu1,
            red2,
            grn2,
            blu2,
            addr0,
            addr1,
            addr2,
            addr3,
            addr4,
            blank,
            clock,
            latch,
        }
    }

    // // tidbyts have a SN74ALVC164245 before the LEDs
    // fn fm6124init(&mut self) {
    //     info!("fm6124init");
    //     let mut red1 = Output::new(
    //         self.red1.reborrow(),
    //         esp_hal::gpio::Level::Low,
    //         OutputConfig::default(),
    //     );
    //     let mut grn1 = Output::new(
    //         self.grn1.reborrow(),
    //         esp_hal::gpio::Level::Low,
    //         OutputConfig::default(),
    //     );
    //     let mut blu1 = Output::new(
    //         self.blu1.reborrow(),
    //         esp_hal::gpio::Level::Low,
    //         OutputConfig::default(),
    //     );
    //     let mut red2 = Output::new(
    //         self.red2.reborrow(),
    //         esp_hal::gpio::Level::Low,
    //         OutputConfig::default(),
    //     );
    //     let mut grn2 = Output::new(
    //         self.grn2.reborrow(),
    //         esp_hal::gpio::Level::Low,
    //         OutputConfig::default(),
    //     );
    //     let mut blu2 = Output::new(
    //         self.blu2.reborrow(),
    //         esp_hal::gpio::Level::Low,
    //         OutputConfig::default(),
    //     );
    //     let mut clock = Output::new(
    //         self.clock.reborrow(),
    //         esp_hal::gpio::Level::Low,
    //         OutputConfig::default(),
    //     );
    //     let mut latch = Output::new(
    //         self.latch.reborrow(),
    //         esp_hal::gpio::Level::Low,
    //         OutputConfig::default(),
    //     );
    //     let mut blank: Output<'_> = Output::new(
    //         self.blank.reborrow(),
    //         esp_hal::gpio::Level::Low,
    //         OutputConfig::default(),
    //     );

    //     // Disable Display
    //     blank.set_high();

    //     let reg1 = [
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::High,
    //         Level::High,
    //         Level::High,
    //         Level::High,
    //         Level::High,
    //         Level::High,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //     ];
    //     let reg2 = [
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::High,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //         Level::Low,
    //     ];

    //     // Send Data to control register REG1
    //     // this sets the matrix brightness actually
    //     for col in 0..COLS {
    //         red1.set_level(reg1[col % 16]);
    //         grn1.set_level(reg1[col % 16]);
    //         blu1.set_level(reg1[col % 16]);
    //         red2.set_level(reg1[col % 16]);
    //         grn2.set_level(reg1[col % 16]);
    //         blu2.set_level(reg1[col % 16]);
    //         // pull the latch 11 clocks before the end of matrix so that REG1 starts counting to save the value
    //         if col > COLS - 12 {
    //             latch.set_high();
    //         }
    //         clock.set_high();
    //         clock.set_low();
    //     }

    //     // drop the latch and save data to the REG1 all over the FM6124 chips
    //     latch.set_low();

    //     // Send Data to control register REG2 (enable LED output)
    //     for col in 0..COLS {
    //         red1.set_level(reg2[col % 16]);
    //         grn1.set_level(reg2[col % 16]);
    //         blu1.set_level(reg2[col % 16]);
    //         red2.set_level(reg2[col % 16]);
    //         grn2.set_level(reg2[col % 16]);
    //         blu2.set_level(reg2[col % 16]);
    //         // pull the latch 12 clocks before the end of matrix so that reg2 stars counting to save the value
    //         if col > COLS - 13 {
    //             latch.set_high();
    //         }
    //         clock.set_high();
    //         clock.set_low();
    //     }

    //     // drop the latch and save data to the REG2 all over the FM6124 chips
    //     latch.set_low();

    //     // blank data regs to keep matrix clear after manipulations
    //     red1.set_low();
    //     grn1.set_low();
    //     blu1.set_low();
    //     red2.set_low();
    //     grn2.set_low();
    //     blu2.set_low();

    //     for _ in 0..COLS {
    //         clock.set_high();
    //         clock.set_low();
    //     }

    //     latch.set_high();

    //     clock.set_high();
    //     clock.set_low();

    //     latch.set_low();
    //     blank.set_low(); // enable display

    //     clock.set_high();
    //     clock.set_low();
    // }
}

#[embassy_executor::task]
pub async fn hub75_task(dispp: DisplayPeripherals<'static>, fb_inc: &'static mut FBType) {
    info!("hub75_task in da house");

    let mut fb = fb_inc;

    let (_, tx_descriptors) = esp_hal::dma_descriptors!(0, FBType::dma_buffer_size_bytes());

    let channel = dispp.dma_channel;
    let pins = Hub75Pins16 {
        red1: dispp.red1,
        grn1: dispp.grn1,
        blu1: dispp.blu1,
        red2: dispp.red2,
        grn2: dispp.grn2,
        blu2: dispp.blu2,
        addr0: dispp.addr0,
        addr1: dispp.addr1,
        addr2: dispp.addr2,
        addr3: dispp.addr3,
        addr4: dispp.addr4,
        blank: dispp.blank,
        clock: dispp.clock,
        latch: dispp.latch,
    };

    #[cfg(any(feature = "esp32s3", feature = "esp32c6"))]
    let mut hub75 = Hub75::<'_, esp_hal::Async>::new_async(
        #[cfg(feature = "esp32s3")]
        dispp.lcd_cam,
        #[cfg(feature = "esp32c6")]
        dispp.parl_io,
        pins,
        channel,
        tx_descriptors,
        #[cfg(any(feature = "esp32s3", feature = "esp32c6"))]
        Rate::from_mhz(20),
    )
    .expect("failed to create hub75");

    #[cfg(feature = "esp32")]
    let mut hub75 = Hub75::<'_, esp_hal::Blocking>::new(
        dispp.i2s,
        pins,
        channel,
        tx_descriptors,
        Rate::from_mhz(10),
    )
    .expect("failed to create hub75")
    .into_async();

    loop {
        // first, toss them bits
        let mut hub75xfer = {
            hub75
                .render(fb)
                .map_err(|(e, _hub75)| e)
                .expect("failed to start render")
        };
        hub75xfer
            .wait_for_done()
            .await
            .expect("hub75 transfer failed");

        let (xferres, new_hub75) = hub75xfer.wait();
        xferres.expect("transfer failed");
        hub75 = new_hub75;

        if FB_XMIT.signaled() {
            let new_fb = FB_XMIT.wait().await;
            FB_PAINT.signal(fb);
            fb = new_fb;
        }

        Timer::after_micros(10000).await;
    }
}
