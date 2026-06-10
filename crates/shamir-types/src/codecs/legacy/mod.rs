//! Legacy transform module - DEPRECATED
//!
//! **Warning:** This entire module is deprecated. Use the newer codec-based approach
//! in the parent `codecs` module instead.

#![allow(deprecated)]

#[cfg(test)]
pub mod tests;
pub mod tools;

pub use tools::{inner_to_user, user_to_inner, TransformResult};
