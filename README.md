# ranodic

Ranodic is an ESP32* application to drive a HUB75 LED panel and an optional DS3231 RTC chip to keep the time across quick restarts

## Feature: "whack"

Some panels have flipped green and blue color lines and thus the colors won't be right; e.g. green will be blue and yellow (RG) will be purple (RB). This feature will fix that situation

## Feature: "rtcchip"

This will enable the use of an external RTC chip in order to save the time and restore it on boot-up in order to skip the potentially infinite boot logo

## Hardware: Tidbyt (ESP32)

OG Tidbyts are supported. For example, to run one with a whack-panel and soldered RTC chip:

`cd ranodic-xtensa-esp32 && cargo run --release --bin ranodic-tidbyt --features="whack rtcchip"`

the `tidbyt` default-feature contains the pinout information

## Hardware: ESP32S3

I haven't tested this extensively (pun not intended)

`cd ranodic-xtensa-esp32s3 && cargo run --release --bin ranodic-esp32s3 --features="whack rtcchip"`

the `selfwire` default-feature that contains a pinout I arbitrarily decided on. It does not work, lmao

## Hardware: ESP32C6

This has been tested extensively and is the primary target

For example, 
`cd ranodic-xtensa-riscv && cargo run --release --bin ranodic-esp32c6 --features="whack rtcchip"`

the `selfwire` default-feature that contains a pinout I arbitrarily decided on
