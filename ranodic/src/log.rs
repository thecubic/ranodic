use alloc::collections::btree_map::BTreeMap;
#[cfg(feature = "defmt")]
pub(crate) use defmt::{debug, error, info, println, warn};

#[cfg(feature = "log")]
pub(crate) use log::{debug, error, info, println, warn};

pub async fn logtest() {}
