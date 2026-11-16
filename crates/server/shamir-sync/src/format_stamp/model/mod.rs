//! The stamp data model: types, epochs and errors.

pub mod components;
pub mod epoch_policy;
pub mod epochs;
pub mod format_stamp;
pub mod stamp_error;

pub use epoch_policy::EpochPolicy;
pub use epochs::{BASE_FORMAT_EPOCH, CURRENT_FORMAT_EPOCH};
pub use format_stamp::{now_unix_ms, FormatStamp, LastFormatMigration, StampActor};
pub use stamp_error::{CorruptKind, StampError};
