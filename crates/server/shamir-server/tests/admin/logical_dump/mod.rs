//! Offline logical-dump CI tests (R3 step 4): shared fixtures and the
//! budget/pinned-property tests built on them. Manifest only — no logic.

#[path = "budget_tests.rs"]
mod budget_tests;
#[path = "fixture.rs"]
mod fixture;
#[path = "pinned_properties.rs"]
mod pinned_properties;
