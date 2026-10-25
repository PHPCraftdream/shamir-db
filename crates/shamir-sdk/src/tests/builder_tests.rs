//! Regression tests for the builder→`Db::execute` seam
//! (RV-QUERY-BUILDER-63B52B5C): `Db::execute` must validate via
//! `Batch::try_build`, so deferred registration errors surface as `Err`
//! instead of being sent silently to the host.
//!
//! NOTE: the host-call half of `Db::execute` (`host_imports::db_execute`)
//! is guest-only (panics off-WASM), so these tests exercise the testable
//! seam — `batch_request`, the Batch→`BatchRequest` conversion helper that
//! `Db::execute` uses for its request construction. The full host round-trip
//! is covered by the WASM e2e suite. Note that this test therefore does NOT
//! catch a revert of `Db::execute`'s routing line alone (the guest-only host
//! call cannot run in a unit test); the routing is covered by CI e2e.

use crate::api::db::batch_request;
use shamir_query_builder::batch::{Batch, BuildError};
use shamir_query_builder::query::Query;

// rv_query_builder_63b52b5c_sdk_execute_surfaces_duplicate_alias
#[test]
fn rv_query_builder_63b52b5c_sdk_execute_surfaces_duplicate_alias() {
    let mut b = Batch::new();
    b.query("dup", Query::from("first_table"));
    b.query("dup", Query::from("second_table"));

    let err = batch_request(&b).unwrap_err();
    assert!(
        err.message().contains("already registered"),
        "expected DuplicateAlias surfaced through the execute seam, got: {}",
        err.message()
    );
    // And the exact typed error is a DuplicateAlias (check via try_build).
    assert!(matches!(
        b.try_build().unwrap_err(),
        BuildError::DuplicateAlias { .. }
    ));
}
