//! Format stamp: data-directory format epoch and startup rules
//! (release-1 R1a). Design:
//! `docs/dev-artifacts/design/format-stamp.md`.

pub mod change_log;
pub mod check;
pub mod codec;
pub mod io;
pub mod model;

#[cfg(test)]
mod tests;

pub use change_log::{FormatChange, FORMAT_CHANGES};
pub use check::{check_data_dir, StampMode, StampOutcome};
pub use codec::{decode, encode, read_header};
pub use io::{data_present, inspect, write_stamp, IgnoreSet, StampState};
pub use model::{
    now_unix_ms, CorruptKind, EpochPolicy, FormatStamp, LastFormatMigration, StampActor,
    StampError, BASE_FORMAT_EPOCH, CURRENT_FORMAT_EPOCH,
};
