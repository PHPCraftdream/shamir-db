//! Test manifest only (repo layout rule): re-exports, no test code.
//!
//! These tests assume the runner's one-test-per-process isolation (the
//! repo-mandated runner), like the repo's other process-global log sinks
//! (see `crates/index/shamir-index/src/tests/support/log_sink.rs` for the same
//! assumption made explicit).

pub mod guard_tests;
pub mod log_capture_tests;
pub mod ring_tests;
pub mod support;
#[cfg(feature = "tracing")]
pub mod tracing_capture_tests;
