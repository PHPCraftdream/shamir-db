#![forbid(unsafe_code)]

//! Internal backend foundation and unstable compiler front end for `shamir-lang`.
//!
//! Front-end APIs are unstable until the compile API arrives in milestone 4.
//! `testsupport` is TEST-ONLY API (`#[doc(hidden)]`, gated behind the
//! `testsupport` feature, no stability promise): it exists so the host
//! crate's dev-dependency execution tests can build the M1 runtime module
//! and its codec vectors. Nothing outside tests may consume it.

mod back;

/// Unstable compiler API, until the milestone 4 compile API is available.
#[doc(hidden)]
pub mod api;
/// Unstable language front end, until the milestone 4 compile API is available.
#[doc(hidden)]
pub mod front;
/// Unstable semantic analysis, until the milestone 4 compile API is available.
#[doc(hidden)]
pub mod sem;

#[cfg(feature = "testsupport")]
#[doc(hidden)]
pub mod testsupport;

pub use api::{compile, CompileError, CompileOptions, CompiledModule, Stage};
pub use back::runtime::RuntimeCaps;
pub use back::{emit_runtime_module, RuntimeError};
