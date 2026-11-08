#![forbid(unsafe_code)]

//! Internal backend foundation for `shamir-lang`.
//!
//! `testsupport` is TEST-ONLY API (`#[doc(hidden)]`, gated behind the
//! `testsupport` feature, no stability promise): it exists so the host
//! crate's dev-dependency execution tests can build the M1 runtime module
//! and its codec vectors. Nothing outside tests may consume it.

mod back;

#[cfg(feature = "testsupport")]
#[doc(hidden)]
pub mod testsupport;

pub use back::runtime::RuntimeCaps;
pub use back::{emit_runtime_module, RuntimeError};
