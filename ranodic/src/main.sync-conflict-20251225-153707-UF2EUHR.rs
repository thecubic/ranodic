#[esp_rtos::main]
pub async fn main(spawner: embassy_executor::Spawner) {
    info!("RRRRRRRRRRRRRRR");
    info!("AAAAAAAAAAAAAAA");
    info!("NNNNNNNNNNNNNNN");
    info!("OOOOOOOOOOOOOOO");
    info!("DDDDDDDDDDDDDDD");
    info!("IIIIIIIIIIIIIII");
    info!("CCCCCCCCCCCCCCC");
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64000);
    esp_alloc::heap_allocator!(size: 48 * 1024);
    // esp_alloc::heap_allocator!(#[unsafe(link_section = ".dram2_uninit")] size: 64 * 1024);
    // esp_alloc::psram_allocator!(peripherals.PSRAM, esp_hal::hal::psram);

    #[cfg(feature = "rtcchip")]
    crate::rtc::rtcread();

    let rtc = crate::RTC
        .init_with(|| core::cell::UnsafeCell::new(esp_hal::rtc_cntl::Rtc::new(peripherals.LPWR)));
    rtc.get_mut()
        .set_interrupt_handler(crate::RTC_INTERRUPT_HANDLER);
    crate::RTCREF.init(unsafe { rtc.as_mut_unchecked() }).ok();
    let rwdt = unsafe { &mut rtc.as_mut_unchecked().rwdt };

    info!("starting RTC watchdog");
    spawner.must_spawn(watchdog_controller(rwdt));

    #[cfg(feature = "rtcchip")]
    crate::rtc::ic_to_sys().await;

    // crate::storage::init(peripherals.FLASH).await.unwrap();
    // crate::storage::init_nvs_iff_uninit().await.unwrap();

    let hp_executor = {
        #[cfg(target_arch = "riscv32")]
        use esp_hal::interrupt::software::SoftwareInterruptControl;
        let software_interrupt = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
        let timg0 = TimerGroup::new(peripherals.TIMG0);
        esp_rtos::start(
            timg0.timer0,
            #[cfg(target_arch = "riscv32")]
            software_interrupt.software_interrupt0,
        );
        HIPRI_EXECUTOR.init_with(|| {
            InterruptExecutor::<HIPRI_CORE>::new(software_interrupt.software_interrupt2)
        })
    };

    // crate::storage::init(peripherals.FLASH).await;
    // esp_println::println!("PREHUB: {}", esp_alloc::HEAP.stats());

    let hp_spawner =
        HIPRI_SPAWNER.init_with(|| hp_executor.start(esp_hal::interrupt::Priority::Priority3));

    hp_spawner.must_spawn(crate::ntp::tick_writer());

    // 4 brightness bits slays the stack here
    let fb0 = crate::drawing::FB0.init_with(|| crate::hub75::FBType::new());
    let fb1 = crate::drawing::FB1.init_with(|| crate::hub75::FBType::new());
    hp_spawner.must_spawn(crate::hub75::hub75_task(
        // tried to pick least-annoying pinout for both the DevKit-C and DevKit-M
        Default::default(),
        fb1,
    ));
    spawner.must_spawn(crate::drawing::display_painter(fb0));

    if crate::net::SSID.is_empty() || crate::net::SSID == crate::net::DEFAULT_SSID {
        error!("no WIFI configured");
    } else {
        info!(
            "SSID: {} PASSWORD: {}",
            crate::net::SSID,
            crate::net::PASSWORD
        );
        // hardware stack init for wifi [link]
        let wdevice = {
            let (controller, interfaces) = esp_radio::wifi::new(
                WIFI_CONTROLLER.init(esp_radio::init().unwrap()),
                peripherals.WIFI,
                Default::default(),
            )
            .unwrap();
            let wcn_watchdog_task = crate::net::conn_watchdog(controller);
            spawner.must_spawn(wcn_watchdog_task);
            interfaces.sta
        };

        // software stack init for wifi [stack]
        let stack = {
            let rng = Rng::new();
            let mut dhcpcfg: DhcpConfig = Default::default();
            // MAX_HOSTNAME_LEN == 32 but they ain't export that
            crate::MAC_ADDRESS.init(wdevice.mac_address()).unwrap();
            let mac_address = crate::MAC_ADDRESS.get().await;
            dhcpcfg.hostname = Some(
                heapless::String::<32>::try_from(
                    alloc::format!(
                        "ranodic-{:02x}{:02x}{:02x}",
                        mac_address[3],
                        mac_address[4],
                        mac_address[5]
                    )
                    .as_str(),
                )
                .or_else(|_| heapless::String::try_from("ranodic-unknown"))
                .expect("couldn't make heapless strings"), // ,a
                                                           // .into(),
            );

            let netcfg = embassy_net::Config::dhcpv4(dhcpcfg);

            let seed = (rng.random() as u64) << 32 | rng.random() as u64;
            let (stack, netrunner) = embassy_net::new(
                wdevice,
                netcfg,
                STACK_RESOURCES.init(StackResources::<NUM_SOCKS>::new()),
                seed,
            );
            let netrun_task = crate::net::net_task(netrunner);
            spawner.must_spawn(netrun_task);
            stack
        };
        // net tasks have their own net up guards
        spawner.must_spawn(crate::ntp::ntp_sync(stack));
        spawner.must_spawn(crate::nightscout::nightscout_query(stack));
        spawner.must_spawn(crate::weather::weather_query(stack));
    }
    spawner.must_spawn(crate::rtc::desync_failsafe());

    // unsafe {
    //     // #[allow(static_mut_refs)]
    //     crate::storage::FLASHS
    //         .init(UnsafeCell::new(FlashStorage::new(peripherals.FLASH)))
    //         .ok();
    // }

    // let mut buffer = [0u8; esp_bootloader_esp_idf::partitions::PARTITION_TABLE_MAX_LEN];
    // let mut ota = esp_bootloader_esp_idf::ota_updater::OtaUpdater::new(
    //     crate::storage::FLASHS.get().await,
    //     &mut buffer,
    // )
    // .unwrap();

    // //      let mut ota =
    // //     esp_bootloader_esp_idf::ota_updater::OtaUpdater::new(&mut flash, &mut buffer).unwrap();

    // // let current = ota.selected_partition().unwrap();
    // match ota.current_ota_state() {
    //     Ok(state) => {
    //         info!("current state: {}", state);
    //     }
    //     Err(esp_bootloader_esp_idf::partitions::Error::InvalidState) => {
    //         info!("invalid state");
    //     }
    //     Err(e) => error!("{}", e),
    // }

    // // yeah that's an awkward API
    // let x = ota
    //     .with_next_partition(|region, apptype| {
    //         // crate::next_partition_inspector(&region, &apptype).await;
    //         apptype
    //     })
    //     .unwrap();

    // let nextpart = partition_table
    //     .find_partition(esp_bootloader_esp_idf::partitions::PartitionType::App(x))
    //     .unwrap()
    //     .unwrap()
    //     .as_embedded_storage(flash);

    // // Mark the current slot as VALID - this is only needed if the bootloader was
    // // built with auto-rollback support. The default pre-compiled bootloader in
    // // espflash is NOT.
    // if let Ok(state) = ota.current_ota_state() {
    //     if state == esp_bootloader_esp_idf::ota::OtaImageState::New
    //         || state == esp_bootloader_esp_idf::ota::OtaImageState::PendingVerify
    //     {
    //         println!("Changed state to VALID");
    //         ota.set_current_ota_state(esp_bootloader_esp_idf::ota::OtaImageState::Valid)
    //             .unwrap();
    //     }
    // }

    // cfg_if::cfg_if! {
    //     if #[cfg(any(feature = "esp32", feature = "esp32s2", feature = "esp32s3"))] {
    //         let button = peripherals.GPIO0;
    //     } else {
    //         let button = peripherals.GPIO9;
    //     }
    // }

    // let boot_button = Input::new(button, InputConfig::default().with_pull(Pull::Up));

    // println!("Press boot button to flash and switch to the next OTA slot");
    // let mut done = false;
    // loop {
    //     if boot_button.is_low() && !done {
    //         done = true;

    //         let (mut next_app_partition, part_type) = ota.next_partition().unwrap();

    //         println!("Flashing image to {:?}", part_type);

    //         // write to the app partition
    //         for (sector, chunk) in OTA_IMAGE.chunks(4096).enumerate() {
    //             println!("Writing sector {sector}...");

    //             next_app_partition
    //                 .write((sector * 4096) as u32, chunk)
    //                 .unwrap();
    //         }

    //         println!("Changing OTA slot and setting the state to NEW");

    //         ota.activate_next_partition().unwrap();
    //         ota.set_current_ota_state(esp_bootloader_esp_idf::ota::OtaImageState::New)
    //             .unwrap();
    //     }
    // }
    // }
    // esp_println::println!("POSTHUB:{}", esp_alloc::HEAP.stats());

    #[cfg(feature = "heapstats")]
    spawner.spawn(crate::heap_stats_printer()).ok();

    #[cfg(feature = "harakiri")]
    spawner.must_spawn(crate::harakiri());

    info!("steady state; awaiting heat death of the universe");
    loop {
        Timer::after(Duration::from_secs(5)).await;
    }
}
