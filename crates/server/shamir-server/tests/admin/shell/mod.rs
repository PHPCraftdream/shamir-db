//! Integration tests for the administrative shell (R5 PR1).
//!
//! Declarations ONLY (rust-guard structure gate): logic lives in the
//! child files. `harness.rs` is the shared fixture and helpers,
//! `output.rs` the offline/output-contract acceptance (design §5, §8
//! PR1 rows), `transport.rs` the real transport scenarios.

mod harness;
mod output;
mod transport;
