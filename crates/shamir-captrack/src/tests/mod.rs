// Test manifest — re-exports only (CLAUDE.md §test organisation).

pub mod off_feature_tests;

#[cfg(feature = "capacity-telemetry")]
pub mod on_feature_tests;
