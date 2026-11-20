//! R4b-1 activity-view integration tests (`shamir-server` `tests/admin/`).
//!
//! `activity_view_under_load` and `permission_denial_activity` (design §6
//! names). The third §6 test, `activity_privacy_no_literals_or_credentials`,
//! lives in the private handler context (deterministic byte-budget hold):
//! `src/engine/db_handler/tests/handlers/activity/`. Manifest only.

mod activity_view_under_load;
mod cancel_commit_test;
mod cancel_fixture;
mod cancel_lock_tests;
mod common;
mod permission_denial_activity;
