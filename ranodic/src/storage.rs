use core::cell::UnsafeCell;
use core::iter::Once;
use core::sync::atomic::AtomicBool;

use alloc::rc::Rc;
use alloc::string::ToString;
use alloc::sync::Arc;
use alloc::{boxed::Box, vec::Vec};
use anyhow::{Error, Result, anyhow};
use critical_section::Mutex;
use defmt::println;
#[cfg(feature = "defmt")]
use defmt::{debug, error, info};
use embassy_sync::blocking_mutex::{Mutex as BlockingMutex, raw::CriticalSectionRawMutex};
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_sync::once_lock::OnceLock;
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_bootloader_esp_idf::partitions::{
    AppPartitionSubType, DataPartitionSubType, FlashRegion, PARTITION_TABLE_MAX_LEN,
    PartitionEntry, PartitionTable, PartitionType, read_partition_table,
};
use esp_hal::Async;
use esp_hal::peripherals::{self, FLASH};
use esp_storage::FlashStorage;
use jiff::Zoned;
use num_enum::TryFromPrimitive;
use serde_json::{Value, json};
use static_cell::StaticCell;
use tickv::TicKV;

use crate::ntp::zgettimeofday;

// pub static FLASH: StaticCell<Mutex<CriticalSectionRawMutex, FlashStorage>> = StaticCell::new();
// pub static FLASHREF: OnceLock<&'static mut FlashStorage> = OnceLock::new();

// pub static FLASHP: OnceLock<peripherals::FLASH> = OnceLock::new();
// pub static FLASH: OnceLock<AsyncMutex<CriticalSectionRawMutex, FlashStorage>> = OnceLock::new();
// pub static FLASH_NVS: OnceLock<BlockingMutex<CriticalSectionRawMutex, FlashRegion<FlashStorage>>> =
// OnceLock::new();

pub static FLASHS: StaticCell<UnsafeCell<FlashStorage>> = StaticCell::new();
// jesus christ
type FlashRefLock =
    OnceLock<AsyncMutex<CriticalSectionRawMutex, &'static mut FlashStorage<'static>>>;
pub static FLASHSREF: FlashRefLock = FlashRefLock::new();

pub static mut NVS: UnsafeCell<NvsConfiguration> = UnsafeCell::new(NvsConfiguration::new());

// ?let mut pbuffer = [0u8; PARTITION_TABLE_MAX_LEN];
// // jesus christ
// type FlashRefLock =
//     OnceLock<AsyncMutex<CriticalSectionRawMutex, &'static mut FlashStorage<'static>>>;

// pub async fn init(flashp: peripherals::FLASH<'static>) -> Result<()> {
//     // peripherals::FLASH::steal();
//     let flash = FLASHS.init_with(|| UnsafeCell::new(FlashStorage::new(flashp)));
//     FLASHSREF
//         .init(AsyncMutex::new(unsafe { flash.as_mut_unchecked() }))
//         .ok();
//     debug!("flash init");
//     Ok(())
// }

pub async fn next_partition_inspector(
    region: &FlashRegion<'static, FlashStorage<'static>>,
    apptype: &AppPartitionSubType,
) {
    info!("would select {:?} of type {:?}", region, apptype);
}

pub async fn init_nvs_iff_uninit() -> Result<()> {
    debug!("init_nvs_iff_uninit start");
    #[allow(static_mut_refs)]
    let nvs = unsafe { NVS.get_mut() };
    debug!("init_nvs_iff_uninit get initial state");
    debug!("nvs.get_state: {} [start]", nvs.get_state());
    // match nvs.get_state() {
    //     NvsConfigurationState::UninitNeverLoaded => todo!(),
    //     NvsConfigurationState::NeverLoaded => todo!(),
    //     _ => {}
    // }
    nvs.read_from_flashstorage().await?;
    debug!("NVS read");
    debug!("nvs.get_state: {} [after read]", nvs.get_state());
    // match nvs.get_state() {
    //     NvsConfigurationState::UninitLoaded => todo!(),
    //     NvsConfigurationState::Loaded => todo!(),
    //     NvsConfigurationState::Dirty => todo!(),
    //     NvsConfigurationState::Synced => todo!(),
    //     _ => {}
    // }

    match nvs.get_state() {
        NvsConfigurationState::UninitLoaded | NvsConfigurationState::Invalid => {
            nvs.initialize()?;
            debug!("nvs.get_state: {} [prewrite]", nvs.get_state());
            nvs.write_to_flashstorage().await?;
        }
        _ => {}
    }
    // if nvs.get_state() == NvsConfigurationState::UninitLoaded {
    // } else {
    //     debug!("wasn't uninit!");
    // }
    debug!("nvs.get_state: {} [after maybewrite]", nvs.get_state());

    match nvs.get_state() {
        NvsConfigurationState::Synced | NvsConfigurationState::Valid => {
            let nvscfg = nvs.get()?;
            info!("nvs: {}", nvscfg.to_string());
        }
        _ => error!("NVS not initialized"),
    }
    match nvs.put(json!({
        "WIFI_PASSWORD": "flowers by irine",
        "WIFI_SSID": "fbi",
        "anumber": 6
    })) {
        Ok(_) => {
            info!("nvs.put would succeed");
        }
        Err(e) => {
            error!("nvs.put would fail: {}", e.to_string().as_str());
            return Err(e);
        }
    }
    Ok(())
}

// use tickv::error_codes::ErrorCode;
// use tickv::flash_controller::FlashController;

// struct FlashCtrl<'a> {
//     flashdev: FlashRegion<'a, FlashStorage<'a>>,
// }

// impl<'a> FlashCtrl<'a> {
//     fn new(flashref: FlashRegion<'a, FlashStorage<'a>>) -> Self {
//         Self { flashdev: flashref }
//     }
// }

// impl FlashController<1024> for FlashCtrl<'_> {
//     /// This function must read the data from the flash region specified by
//     /// `region_number` into `buf`. The length of the data read should be the
//     /// same length as buf.
//     ///
//     /// On success it should return nothing, on failure it
//     /// should return ErrorCode::ReadFail.
//     ///
//     /// If the read operation is to be complete asynchronously then
//     /// `read_region()` can return `ErrorCode::ReadNotReady(region_number)`.
//     /// By returning `ErrorCode::ReadNotReady(region_number)`
//     /// `read_region()` can indicate that the operation should be retried in
//     /// the future.
//     /// After running the `continue_()` functions after a async
//     /// `read_region()` has returned `ErrorCode::ReadNotReady(region_number)`
//     /// the `read_region()` function will be called again and this time should
//     /// return the data.
//     // b.read(offset, bytes);
//     // b.write(offset, bytes);
//     // b.erase(from, to);
//     fn read_region(&self, region_number: usize, buf: &mut [u8; 1024]) -> Result<(), ErrorCode> {
//         let a = unsafe { &mut self.flashdev }.read(region_number as u32, buf);
//         // FLASH_NVS.get()
//         Ok(())
//     }

//     /// This function must write the length of `buf` to the specified address
//     /// in flash.
//     /// If the length of `buf` is smaller then the minimum supported write size
//     /// the implementation can write a larger value. This should be done by first
//     /// reading the value, making the changed from `buf` and then writing it back.
//     ///
//     /// On success it should return nothing, on failure it
//     /// should return ErrorCode::WriteFail.
//     ///
//     /// If the write operation is to be complete asynchronously then
//     /// `write()` can return `ErrorCode::WriteNotReady(region_number)`.
//     /// By returning `ErrorCode::WriteNotReady(region_number)`
//     /// `read_region()` can indicate that the operation should be retried in
//     /// the future. Note that that region will not be written
//     /// again so the write must occur otherwise the operation fails.
//     fn write(&self, address: usize, buf: &[u8]) -> Result<(), ErrorCode> {
//         let a = self.flashdev.write(address as u32, buf);
//         Ok(())
//     }

//     /// This function must erase the region specified by `region_number`.
//     ///
//     /// On success it should return nothing, on failure it
//     /// should return ErrorCode::WriteFail.
//     ///
//     /// If the erase is going to happen asynchronously then this should return
//     /// `EraseNotReady(region_number)`. Note that that region will not be erased
//     /// again so the erasure must occur otherwise the operation fails.
//     fn erase_region(&self, region_number: usize) -> Result<(), ErrorCode> {
//         Ok(())
//     }
// }

#[derive(Debug, Eq, PartialEq)]
pub enum NvsConfigurationState {
    UninitNeverLoaded,
    ZeroNeverLoaded,
    NeverLoaded,
    UninitLoaded,
    Valid,
    Invalid,
    Dirty,
    Synced,
}

#[cfg(feature = "defmt")]
impl defmt::Format for NvsConfigurationState {
    fn format(&self, fmt: defmt::Formatter) {
        match self {
            NvsConfigurationState::UninitNeverLoaded => defmt::write!(fmt, "UninitNeverLoaded"),
            NvsConfigurationState::ZeroNeverLoaded => defmt::write!(fmt, "ZeroNeverLoaded"),
            NvsConfigurationState::NeverLoaded => defmt::write!(fmt, "NeverLoaded"),
            NvsConfigurationState::UninitLoaded => defmt::write!(fmt, "UninitLoaded"),
            NvsConfigurationState::Valid => defmt::write!(fmt, "Valid"),
            NvsConfigurationState::Invalid => defmt::write!(fmt, "Invalid"),
            NvsConfigurationState::Dirty => defmt::write!(fmt, "Dirty"),
            NvsConfigurationState::Synced => defmt::write!(fmt, "Synced"),
        }
    }
}

const NVS_SIZE: usize = 0x4000;
const NVS_UNINIT_BYTE: u8 = 0xff;

static mut PTABLE: [u8; PARTITION_TABLE_MAX_LEN] = [0u8; PARTITION_TABLE_MAX_LEN];
static PARTITIONS: OnceLock<PartitionTable> = OnceLock::new();
static NVS_PARTITION: OnceLock<PartitionEntry> = OnceLock::new();
static mut FLASHSTORAGE: OnceLock<FlashStorage> = OnceLock::new();

pub fn init2() {
    let mut flashstorage = FlashStorage::new(unsafe { esp_hal::peripherals::FLASH::steal() });
    let ptable = read_partition_table(&mut flashstorage, unsafe { &mut PTABLE })
        .expect("couldn't read partition table");

    let nvs = ptable
        .find_partition(PartitionType::Data(DataPartitionSubType::Nvs))
        .expect("searching partitions failed!")
        .expect("no NVS partition found!");

    NVS_PARTITION.init(nvs);
    PARTITIONS.init(ptable);
    unsafe { FLASHSTORAGE.init(flashstorage).ok() };
}

pub struct NvsConfiguration {
    nvs_copy: [u8; NVS_SIZE],
    dirty: bool,
    loaded: Option<Zoned>,
    written: Option<Zoned>,
}

impl NvsConfiguration {
    pub const fn new() -> Self {
        Self {
            nvs_copy: [0u8; NVS_SIZE],
            dirty: false,
            loaded: None,
            written: None,
        }
    }

    fn get_state(&self) -> NvsConfigurationState {
        if let Some(loaded) = &self.loaded {
            debug!("get_state: loaded:{}", loaded.to_string().as_str());
            if let Some(written) = &self.written {
                debug!("get_state: written:{}", written.to_string().as_str());
                if written > loaded {
                    // I just wrote it
                    NvsConfigurationState::Synced
                } else {
                    // please write it
                    NvsConfigurationState::Dirty
                }
            } else {
                debug!("get_state: !written");
                if self.is_uninit() {
                    // never touched
                    NvsConfigurationState::UninitLoaded
                } else {
                    // never touched by us
                    match self.get() {
                        Ok(_) => NvsConfigurationState::Valid,
                        Err(_) => NvsConfigurationState::Invalid,
                    }
                }
            }
        } else {
            debug!("get_state: !loaded");
            // local-only states
            if self.is_uninit() {
                // uninited without loading
                NvsConfigurationState::UninitNeverLoaded
            } else if self.is_zero() {
                // untouched
                NvsConfigurationState::ZeroNeverLoaded
            } else {
                // it was fucked with
                NvsConfigurationState::NeverLoaded
            }
        }
    }

    // fn read_from_peripheral(&mut self, flash: peripherals::FLASH) -> Result<()> {
    //     let flashs = FlashStorage::new(flash);
    //     self.read_from_flashstorage(flashs)
    // }

    async fn read_from_flashstorage(&mut self) -> Result<()> {
        let before_state = self.get_state();
        #[allow(static_mut_refs)]
        let mut flashstorage = unsafe { FLASHSTORAGE.take().expect("flashstorage not there") };
        NVS_PARTITION
            .get()
            .await
            .as_embedded_storage(&mut flashstorage)
            .read(0, &mut self.nvs_copy)
            .expect("couldn't read NVS");
        info!(
            "read from storage [{} -> {}]",
            before_state,
            self.get_state()
        );
        self.dirty = false;
        self.loaded = Some(zgettimeofday().await);
        #[allow(static_mut_refs)]
        unsafe {
            FLASHSTORAGE.init(flashstorage).ok();
        };
        Ok(())
        // // let mut flash = FlashStorage::new(peripherals::FLASH.steal());
        // // let mut flash = FLASHSREF.get().await.lock().await;
        // // let mut pbuffer = [0u8; PARTITION_TABLE_MAX_LEN];
        // match read_partition_table(*flash, &mut pbuffer) {
        //     Ok(partition_table) => {
        //         match partition_table.find_partition(PartitionType::Data(DataPartitionSubType::Nvs))
        //         {
        //             Ok(Some(nvsp)) => {
        //                 info!("NVS partition found, reading");
        //                 match nvsp.as_embedded_storage(*flash).read(0, &mut self.nvs_copy) {
        //                     Ok(()) => {
        //                         info!(
        //                             "read from storage [{} -> {}]",
        //                             before_state,
        //                             self.get_state()
        //                         );
        //                         self.dirty = false;
        //                         self.loaded = Some(zgettimeofday().await);
        //                         Ok(())
        //                     }
        //                     Err(e) => {
        //                         error!("error reading NVS partition: {}", e);
        //                         Err(anyhow!("error reading NVS partition: {}", e))
        //                     }
        //                 }
        //             }
        //             Ok(None) => {
        //                 error!("NVS partition not found");
        //                 Err(anyhow!("NVS partition not found"))
        //             }
        //             Err(e) => {
        //                 error!("error searching for partition table: {}", e);
        //                 Err(anyhow!("error searching for partition table: {}", e))
        //             }
        //         }
        //     }
        //     Err(e) => {
        //         error!("{}", e);
        //         Err(anyhow!("error reading partition table: {}", e))
        //     }
        // }
    }

    // fn with_nvs(&mut self, f: FnOnce) {

    // }

    // async fn with_nvs_async(&mut self, f: AsyncFnOnce) -> Result<()> {

    // }

    fn deinit(&mut self) -> Result<()> {
        let before_state = self.get_state();
        self.nvs_copy.fill(NVS_UNINIT_BYTE);
        self.dirty = true;
        info!("deinited [{} -> {}]", before_state, self.get_state());
        Ok(())
    }

    fn initialize(&mut self) -> Result<()> {
        let before_state = self.get_state();
        // self.nvs_copy.
        let myjson = serde_json::to_vec_pretty(&serde_json::json!({}))?;
        if myjson.len() > NVS_SIZE {
            return Err(anyhow!("config too big"));
        }
        // noooo! you must only submit a complete JSON document!
        // fine, we fill it with newlines and move
        // your close bracket to the end
        // enjoy your
        // { json is the
        // greatest
        // }
        self.nvs_copy.fill(b'\n');
        self.nvs_copy[..myjson.len() - 1].copy_from_slice(&myjson[..myjson.len() - 1]);
        // alas, the json has ended
        self.nvs_copy[NVS_SIZE - 1] = b'}';
        self.dirty = true;
        // let a = serde_json::json!("test");
        // a.as_str().
        info!("initialized [{} -> {}]", before_state, self.get_state());
        Ok(())
    }

    async fn write_to_flashstorage(&mut self) -> Result<()> {
        let before_state = self.get_state();
        let mut flash = FLASHSREF.get().await.lock().await;
        if self.dirty {
            let mut pbuffer = [0u8; PARTITION_TABLE_MAX_LEN];
            match read_partition_table(*flash, &mut pbuffer) {
                Ok(partition_table) => {
                    match partition_table
                        .find_partition(PartitionType::Data(DataPartitionSubType::Nvs))
                    {
                        Ok(Some(nvsp)) => {
                            info!("NVS partition found, reading");
                            match nvsp.as_embedded_storage(*flash).write(0, &self.nvs_copy) {
                                Ok(()) => {
                                    info!(
                                        "wrote to storage [{} -> {}]",
                                        before_state,
                                        self.get_state()
                                    );
                                    self.dirty = false;
                                    self.written = Some(zgettimeofday().await);
                                    Ok(())
                                }
                                Err(e) => {
                                    error!("error reading NVS partition: {}", e);
                                    Err(anyhow!("error reading NVS partition: {}", e))
                                }
                            }
                        }
                        Ok(None) => {
                            error!("NVS partition not found");
                            Err(anyhow!("NVS partition not found"))
                        }
                        Err(e) => {
                            error!("error searching for partition table: {}", e);
                            Err(anyhow!("error searching for partition table: {}", e))
                        }
                    }
                }
                Err(e) => {
                    error!("{}", e);
                    // Err(anyhow!("error reading partition table: {}", e))
                    Err(e.into())
                }
            }
        } else {
            Err(anyhow!("no update needed or forced"))
        }
    }

    fn is_zero(&self) -> bool {
        self.nvs_copy.iter().all(|b| *b == 0x0)
    }

    fn is_uninit(&self) -> bool {
        self.nvs_copy.iter().all(|b| *b == NVS_UNINIT_BYTE)
    }

    fn get(&self) -> Result<Value> {
        serde_json::from_slice(&self.nvs_copy).map_err(anyhow::Error::msg)
    }

    fn put(&self, mut v: Value) -> Result<()> {
        v.sort_all_objects();
        let output = serde_json::to_vec(&v)?;
        if output.len() > NVS_SIZE {
            Err(anyhow!(
                "configuration too large for NVS partition: {} > {}",
                output.len(),
                NVS_SIZE
            ))
        } else {
            // let jasonsize = output.len();
            debug!("configuration size ok: {} <= {}", output.len(), NVS_SIZE);
            Ok(())
        }
    }
}

// static NVS_CONFIGURATION: AsyncMutex<CriticalSectionRawMutex, NvsConfiguration> =
// AsyncMutex::new(NvsConfiguration::new().await);

// static FLASH_PERIPHERAL: OnceLockBox<AsyncMutex<CriticalSectionRawMutex, peripherals::FLASH>> = Box::new

// let a = flashp.reborrow();

// {
//     let mut nvs = NVS_CONFIGURATION.lock().await;
//     let mut flashs = FlashStorage::new(flashp.reborrow());
//     let mut uninitted = [0xffu8; NVS_SIZE];
//     let mut pbuffer = [0u8; PARTITION_TABLE_MAX_LEN];
//     match read_partition_table(&mut flashs, &mut pbuffer) {
//         Ok(partition_table) => {
//             match partition_table.find_partition(PartitionType::Data(DataPartitionSubType::Nvs))
//             {
//                 Ok(Some(nvsp)) => {
//                     info!("NVS partition found, reading");
//                     match nvsp.as_embedded_storage(&mut flashs).write(0, &uninitted) {
//                         Ok(()) => {
//                             info!("wrote to storage, now undirty");
//                         }
//                         Err(e) => {
//                             error!("error reading NVS partition: {}", e);
//                         }
//                     }
//                 }
//                 Ok(None) => {
//                     error!("NVS partition not found");
//                 }
//                 Err(e) => {
//                     error!("error searching for partition table: {}", e);
//                 }
//             }
//         }
//         Err(e) => {
//             error!("{}", e);
//         }
//     }

// match nvs.read_from_flashstorage(flashs) {
//     Ok(_) => {
//         info!(
//             "read initial; zero:{} deinit:{}",
//             nvs.is_zero(),
//             nvs.is_uninit()
//         );
//     }
//     Err(e) => {
//         error!("couldn't read initial: {}", e.to_string().as_str())
//     }
// }

// {
//     nvs.deinit()?;
//     match nvs.write_to_flashstorage(flashs) {
//         Ok(_) => {
//             info!("wrote deinit");
//         }
//         Err(e) => {
//             error!("couldn't write deinit: {}", e.to_string().as_str())
//         }
//     }
// }

// match nvs.read_from_flashstorage(flashs) {
//     Ok(_) => {
//         info!(
//             "read after deinit; zero:{} deinit:{}",
//             nvs.is_zero(),
//             nvs.is_uninit()
//         );
//     }
//     Err(e) => {
//         error!("couldn't read after deinit: {}", e.to_string().as_str())
//     }
// }
// if nvs.is_uninit() {
//     info!("NVS configuration is uninitalized");
//     nvs.initialize()?;
//     nvs.write_to_flashstorage(flashs)?;
//     match nvs.read_from_flashstorage(flashs) {
//         Ok(_) => {
//             info!(
//                 "read after init; zero:{} deinit:{}",
//                 nvs.is_zero(),
//                 nvs.is_uninit()
//             );
//         }
//         Err(e) => {
//             error!("couldn't read after init: {}", e.to_string().as_str())
//         }
//     }
//     info!("initialized NVS configuration");
// } else {
//     info!("NVS configuration is initialized");
// }

// {
//     match nvs.get() {
//         Ok(v) => {
//             info!("nvs get Value: {}", v.to_string().as_str());
//             println!("{}", v.to_string().as_str());
//         }
//         Err(e) => {
//             error!("couldn't nvs get: {}", e.to_string().as_str())
//         }
//     }
//     // let v = nvs.get()?;
// }
// }
// match nvs.read_from_peripheral(flashp.reborrow()) {
//     Ok(_) => {
//         info!(
//             "read initial; zero:{} deinit:{}",
//             nvs.is_zero(),
//             nvs.is_uninit()
//         );
//     }
//     Err(e) => {
//         error!("couldn't read initial: {}", e.to_string().as_str())
//     }
// }

// {
//     nvs.deinit()?;
//     match nvs.write_to_peripheral(flashp.reborrow()) {
//         Ok(_) => {
//             info!("wrote deinit");
//         }
//         Err(e) => {
//             error!("couldn't write deinit: {}", e.to_string().as_str())
//         }
//     }
// }

// match nvs.read_from_peripheral(flashp.reborrow()) {
//     Ok(_) => {
//         info!(
//             "read after deinit; zero:{} deinit:{}",
//             nvs.is_zero(),
//             nvs.is_uninit()
//         );
//     }
//     Err(e) => {
//         error!("couldn't read after deinit: {}", e.to_string().as_str())
//     }
// }
// if nvs.is_uninit() {
//     info!("NVS configuration is uninitalized");
//     nvs.initialize()?;
//     nvs.write_to_peripheral(flashp.reborrow())?;
//     match nvs.read_from_peripheral(flashp.reborrow()) {
//         Ok(_) => {
//             info!(
//                 "read after init; zero:{} deinit:{}",
//                 nvs.is_zero(),
//                 nvs.is_uninit()
//             );
//         }
//         Err(e) => {
//             error!("couldn't read after init: {}", e.to_string().as_str())
//         }
//     }
//     info!("initialized NVS configuration");
// } else {
//     info!("NVS configuration is initialized");
// }

// {
//     match nvs.get() {
//         Ok(v) => {
//             info!("nvs get Value: {}", v.to_string().as_str());
//             println!("{}", v.to_string().as_str());
//         }
//         Err(e) => {
//             error!("couldn't nvs get: {}", e.to_string().as_str())
//         }
//     }
//     // let v = nvs.get()?;
// }
// }

// if NVS_CONFIGURATION.lock().await.is_uninit() {

// }
// let _ = FLASHP.init(flashp);

// let a = FLASHP.take().expect("yes");

// NVS_CONFIGURATION.init(NvsConfiguration::new());

// let flashs = FlashStorage::new(flashp);
// FLASH.init(AsyncMutex::new(flashs)).unwrap();

// let flash = FLASH.init(FlashStorage::new(flashp));
// let b = flash.deref_mut();
// let mut flash = FlashStorage::new(flashp);
// let mut flashref = unsafe { &mut flash };
// {
//     use esp_bootloader_esp_idf::partitions::{
//         read_partition_table, PartitionEntry, PARTITION_TABLE_MAX_LEN,
//     };
//     // this is too conservative imho.
//     // the partition table doesn't own anything and embedded storage has the
//     // flash device's lifetime but lifetime flows through all the way to the FlashRegion
//     // but it shouldn't because a const ref can just read the region deets from the PartitionEntry, not
//     // backref it to a read copy of the partition table

//     {
//         //let flash = FLASH.get().await;

//         let mut flash = FLASH.get().await.lock().await;

//         let buffer = Box::new([0u8; PARTITION_TABLE_MAX_LEN]);
//         let partition_table = read_partition_table(&mut *flash, Box::leak(buffer)).unwrap();
//         match partition_table.find_partition(PartitionType::Data(DataPartitionSubType::Nvs)) {
//             Ok(Some(nvsp)) => {
//                 let freed_nvsp = Box::new(nvsp);
//                 FLASH_NVS
//                     .init(BlockingMutex::new(
//                         Box::leak(freed_nvsp).as_embedded_storage(&mut *flash),
//                     ))
//                     .unwrap();
//                 info!("NVS partition reference stored");
//             }
//             Ok(None) => {
//                 error!("NVS partition not found");
//             }
//             Err(e) => {
//                 error!("{}", e)
//             }
//         }
//     }
//     // let rb = Box::new([0u8; 1024]);
// let b = FLASH_NVS.get().await;
// b.read(offset, bytes);
// b.write(offset, bytes);
// b.erase(from, to);
// let t = TicKV::new(
//     controller,
//     Box::leak(rb),
//     FLASH_NVS.get().await.partition_size(),
// );

//     let mut partitions: Vec<PartitionEntry> = Vec::new();
//     let entries = partition_table.len();
//     for i in 0..entries {
//         let partition = partition_table.get_partition(i).unwrap();
//         match partition.partition_type() {
//             PartitionType::Data(DataPartitionSubType::Nvs) => {
//                 info!("NVS found");
//                 FLASH_NVS.init(partition.as_embedded_storage(flash));
//             }
//             _ => {}
//         }
//         info!("{:?}", partition);
//         partitions.push(partition);
//     }
//     info!(
//         "Currently booted partition {:?}",
//         partition_table.booted_partition()
//     );
// }
