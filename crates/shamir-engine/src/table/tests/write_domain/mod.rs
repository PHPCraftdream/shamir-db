//! S5c-X — repository write-admission domain: TABLE layer tests.
//!
//! Manifest for the write-domain test subfolder (mirrors the flat
//! `pub mod <name>_tests;` style of the parent `tests/mod.rs`).
pub mod admission_matrix_tests;
pub mod barrier_compound_guard_tests;
pub mod counter_park_tests;
pub mod shared;
