//! Format stamp: data-directory format epoch and startup rules
//! (release-1 R1a). Design:
//! `docs/dev-artifacts/design/format-stamp.md`.

pub mod check;
pub mod codec;
pub mod io;
pub mod migration;
pub mod model;

#[cfg(test)]
mod tests;

pub use check::{check_data_dir, inspect_gate, StampMode, StampOutcome};
pub use codec::{decode, encode, read_header, STAMP_FILE_NAME, STAMP_TMP_NAME};
pub use io::{
    data_present, inspect, observe_stamp_for_backup, restore_snapshot_gate, write_stamp,
    BackupStampObservation, IgnoreSet, StampState,
};
pub use model::{
    now_unix_ms, CorruptKind, EpochPolicy, FormatStamp, LastFormatMigration, StampActor,
    StampError, BASE_FORMAT_EPOCH, CURRENT_FORMAT_EPOCH,
};
