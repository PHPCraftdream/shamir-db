//! The startup check and its mode/outcome types (design 3).

pub mod check_data_dir;
pub mod stamp_mode;
pub mod stamp_outcome;

pub use check_data_dir::check_data_dir;
pub use stamp_mode::StampMode;
pub use stamp_outcome::StampOutcome;
