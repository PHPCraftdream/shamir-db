//! RV-DB-6540DC7B bootstrap coverage: the boot-owned default-database step
//! ([`ensure_default_database`]) must propagate a catalogue persist
//! failure as `BootError::ShamirDbInit` and must not leave a false-success
//! live `default` registration behind.
//!
//! The fault is real and deterministic, not mocked: the system store's
//! catalogue writes run through the gated `SystemStore` write path, and
//! `arm_catalogue_write_fault(1)` (the `#[doc(hidden)]` test-util seam on
//! `shamir.system_store()`) makes the very next gated write — here the
//! `save_database` prewrite inside `create_db_as_checked` — fail with
//! `DbError::Internal` BEFORE any durable row is written. The checked
//! creation persists BEFORE publishing the live registration, so the
//! failure leaves no live registration, and the helper maps the
//! error onto `BootError::ShamirDbInit`.

use std::sync::Arc;

use shamir_db::engine::repo::{BoxRepoFactory, DurabilityPolicy, RepoConfig};
use shamir_db::ShamirDb;

use super::super::helpers::ensure_default_database;
use crate::server::boot_error::BootError;

#[tokio::test]
async fn rv_db_6540dc7b_bootstrap_default_database_propagates_catalogue_failure() {
    let shamir = Arc::new(ShamirDb::init_memory().await.unwrap());

    // Deterministic prewrite fault: the next gated catalogue write —
    // `save_database("default", ...)` on the boot-owned creation path —
    // fails at the gate before any durable row exists.
    shamir.system_store().arm_catalogue_write_fault(1);

    let result = ensure_default_database(&shamir).await;
    match result {
        Err(BootError::ShamirDbInit(msg)) => {
            assert!(
                msg.contains("create_db default"),
                "expected the create_db default mapping, got: {msg}"
            );
            // The mapping must carry the UNDERLYING catalogue fault, so a
            // helper mutated to ignore the checked-creation error and fall
            // through to the generic "default db missing after ensure"
            // message fails this assertion.
            assert!(
                msg.contains("injected catalogue write failure"),
                "mapping must include the underlying catalogue fault \
                 ('injected catalogue write failure'), got: {msg}"
            );
        }
        Err(other) => panic!("RV-DB-6540DC7B: expected BootError::ShamirDbInit, got {other:?}"),
        Ok(_) => panic!(
            "RV-DB-6540DC7B: boot-owned default-database creation must fail \
             when the catalogue save fails, but it succeeded"
        ),
    }

    // No false success: the checked creation rolls the live registration
    // back, so no live `default` db may survive the failed boot step.
    assert!(
        !shamir.has_db("default"),
        "a failed default-database creation must leave no live registration"
    );
    // And no durable row either — the fault fired on the prewrite.
    assert!(
        shamir
            .system_store()
            .load_database("default")
            .await
            .expect("load_database must not fail")
            .is_none(),
        "a failed default-database creation must leave no durable catalogue row"
    );
}

#[tokio::test]
async fn rv_db_6540dc7b_bootstrap_default_database_success_is_idempotent() {
    let shamir = Arc::new(ShamirDb::init_memory().await.unwrap());
    assert!(!shamir.has_db("default"), "precondition");

    // First call takes the missing branch: creates and registers.
    let first = ensure_default_database(&shamir)
        .await
        .expect("first ensure must succeed");
    assert!(shamir.has_db("default"));
    assert!(
        shamir
            .system_store()
            .load_database("default")
            .await
            .expect("load_database must not fail")
            .is_some(),
        "the durable 'default' catalogue row must exist after a successful ensure"
    );

    // Instance identity by shared observable mutation: mutate the FIRST
    // returned handle and require the SECOND call to observe the same
    // live instance (empty list_repos() equality alone would be weak).
    first
        .add_repo_with_durability(
            RepoConfig::new("probe", BoxRepoFactory::in_memory()),
            DurabilityPolicy::default(),
        )
        .await
        .expect("adding the probe repo must succeed");

    // Second call takes the already-exists branch and succeeds against the
    // same live registration.
    let second = ensure_default_database(&shamir)
        .await
        .expect("second ensure must succeed");
    assert!(
        second.has_repo("probe"),
        "the second ensure must return the SAME live instance the first \
         returned (probe repo visible through the second handle)"
    );
}
