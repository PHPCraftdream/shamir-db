//! DEPRECATED: Transform module for UserValue <-> InnerValue conversion.
//!
//! **This entire module is deprecated.** Use the newer codec-based approach
//! in the parent `codecs` module instead.

#![allow(deprecated)]

pub mod tests;
pub mod transform_tools;

pub use transform_tools::{inner_to_user, user_to_inner, TransformResult};
