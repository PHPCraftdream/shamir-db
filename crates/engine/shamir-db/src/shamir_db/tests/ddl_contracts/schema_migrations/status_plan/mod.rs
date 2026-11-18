//! Manifest for the status/plan DDL-contract tests (re-exports only; no
//! logic). `helpers` is re-imported so the moved test modules keep resolving
//! `super::helpers` without any change to their content.

use super::helpers;

mod fail_closed_tests;
mod identity_tests;
mod plan_tests;
mod status_tests;
