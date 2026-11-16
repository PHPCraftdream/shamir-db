//! Filesystem layer: atomic write, inspection, data presence.

pub mod atomic_write;
pub mod data_presence;
pub mod inspect;

pub use atomic_write::write_stamp;
pub use data_presence::{data_present, IgnoreSet};
pub use inspect::{inspect, StampState};
