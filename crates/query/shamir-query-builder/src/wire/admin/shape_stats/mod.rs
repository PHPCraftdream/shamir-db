//! Builders for the R4b step 4 shape-statistics admin ops.
//!
//! Grouped in a subfolder to keep `wire/admin/` inside its layout budget;
//! both constructors are re-exported from `wire/admin/mod.rs`.

#![warn(missing_docs)]

mod list_shape_stats;
mod reset_shape_stats;

pub use list_shape_stats::list_shape_stats;
pub use reset_shape_stats::reset_shape_stats;
