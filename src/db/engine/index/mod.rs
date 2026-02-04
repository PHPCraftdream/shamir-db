//! Index module
//!
//! Provides index configuration types.

mod def;
mod target;

pub use def::IndexDef;
pub use target::{IndexTarget, IndexMode, IndexStatus};
