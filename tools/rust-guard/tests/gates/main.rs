//! Self-test target for the per-gate suites (CI-only).

#[path = "../harness.rs"]
mod harness;

mod layers_tests;
mod modules_tests;
mod orderings_tests;
mod structure_tests;
