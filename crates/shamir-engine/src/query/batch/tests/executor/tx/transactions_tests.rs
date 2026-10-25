//! Tests for transactional batch execution (SI happy path, TxOutcome mapping).

use shamir_query_builder::batch::Batch;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_types::access::Actor;

use crate::query::batch::execute_batch_unchecked as execute_batch;

use super::super::common::TxTestResolver;

// ============================================================================
// execute_batch — transactional SI happy path
// ============================================================================

#[tokio::test]
async fn execute_batch_transactional_si_happy_path() {
    use futures::StreamExt;

    let factory = crate::repo::repo_types::BoxRepoFactory::in_memory();
    let repo = crate::repo::RepoInstance::from_factory(
        "test".into(),
        factory,
        vec![crate::table::TableConfig::new("users")],
    )
    .await
    .unwrap();

    let resolver = TxTestResolver { repo: repo.clone() };

    let mut b = Batch::new();
    b.id(1);
    b.transactional();
    b.insert(
        "ins",
        write::insert("users")
            .row(doc().set("name", "alice"))
            .row(doc().set("name", "bob")),
    );
    let request = b.build();

    let response = execute_batch(&request, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let info = response.transaction.expect("transaction info present");
    assert_eq!(info.status, "committed");
    assert!(info.tx_id > 0);
    assert!(info.commit_version.unwrap_or(0) > 0);
    // Happy path: projections applied inline (`MaterializationState::Complete`)
    // is threaded to the client as materialized=true.
    assert!(
        info.materialized,
        "inline-materialized commit must report materialized=true"
    );

    // Outside the tx, observer reads see the committed records.
    let tbl = repo.get_table("users").await.unwrap();
    let stream = tbl.list_stream(64);
    futures::pin_mut!(stream);
    let mut count = 0usize;
    while let Some(batch) = stream.next().await {
        count += batch.unwrap().len();
    }
    assert_eq!(count, 2, "outside observer must see 2 committed records");
}

// ============================================================================
// A1 — executor maps the commit's MaterializationState to TransactionInfo.
//
// The happy path above proves the `Complete` case end-to-end through a real
// `commit_tx`. A `Deferred` outcome only arises when a projection sub-phase
// fails AFTER the commit point — that fault-injection seam lives in
// `tx::commit::materialize`, which this agent does not own. So we pin the
// executor's mapping (`outcome.materialized()` → `TransactionInfo::committed`)
// directly against a hand-built `TxOutcome` for both states.
// ============================================================================

/// The mapping the executor performs at the commit-success arm of
/// `execute_transactional` (executor.rs). Kept in lock-step with the
/// production call site.
fn map_outcome_to_info(
    outcome: &crate::tx::TxOutcome,
) -> shamir_query_types::batch::TransactionInfo {
    shamir_query_types::batch::TransactionInfo::committed(
        outcome.tx_id,
        outcome.snapshot_version,
        outcome.commit_version,
        outcome.materialized(),
    )
    .with_visibility(outcome.visibility)
}

#[test]
fn published_durability_error_maps_to_committed_metadata() {
    for materialization in [
        crate::tx::tx_outcome::MaterializationState::Complete,
        crate::tx::tx_outcome::MaterializationState::Deferred,
    ] {
        for visibility in [
            shamir_query_types::batch::VisibilityState::Visible,
            shamir_query_types::batch::VisibilityState::Pending {
                visible_through: 100,
                blocking_version: 101,
                reason: shamir_query_types::batch::VisibilityPendingReason::Timeout,
            },
        ] {
            let error = crate::tx::CommitError::WalDurability {
                commit_version: 107,
                materialization,
                visibility,
            };
            // Production helper: a default-visible mapping would lose pending.
            let info = error.into_transaction_info(11, 100);
            assert_eq!(info.status, "committed");
            assert_eq!(info.tx_id, 11);
            assert_eq!(info.snapshot_version, Some(100));
            assert_eq!(info.commit_version, Some(107));
            assert_eq!(info.reason, None);
            assert_eq!(info.error.as_deref(), Some("wal_durability"));
            assert_eq!(info.visibility, Some(visibility));
            assert_eq!(
                info.materialized,
                materialization == crate::tx::tx_outcome::MaterializationState::Complete,
            );
        }
    }
    let info = crate::tx::CommitError::WalRequiresReopen.into_transaction_info(12, 100);
    assert_eq!(info.status, "aborted");
    assert_eq!(info.reason.as_deref(), Some("wal_requires_reopen"));
    assert_eq!(info.error, None);
    assert_eq!(info.commit_version, None);
    assert_eq!(info.snapshot_version, None);
}

#[test]
fn deferred_outcome_maps_to_materialized_false() {
    use crate::tx::tx_outcome::MaterializationState;

    let complete = crate::tx::TxOutcome {
        tx_id: 11,
        snapshot_version: 100,
        commit_version: 101,
        materialization: MaterializationState::Complete,
        visibility: shamir_query_types::batch::VisibilityState::Visible,
        background: None,
    };
    let deferred = crate::tx::TxOutcome {
        tx_id: 12,
        snapshot_version: 100,
        commit_version: 102,
        materialization: MaterializationState::Deferred,
        visibility: shamir_query_types::batch::VisibilityState::Visible,
        background: None,
    };

    let complete_info = map_outcome_to_info(&complete);
    assert!(complete_info.is_committed());
    assert!(
        complete_info.materialized,
        "Complete must map to materialized=true"
    );

    let deferred_info = map_outcome_to_info(&deferred);
    assert!(
        deferred_info.is_committed(),
        "a deferred commit is still COMMITTED, not aborted"
    );
    assert!(
        !deferred_info.materialized,
        "Deferred must map to materialized=false"
    );
}

// RV-WAL-3B29232E: an indeterminate WAL outcome maps to status
// "indeterminate" with the ATTEMPTED commit_version and the
// `wal_outcome_unknown` error — no success claim, never aborted.
#[test]
fn rv_wal_3b29232e_unknown_outcome_is_indeterminate_not_aborted() {
    let info = crate::tx::CommitError::WalOutcomeUnknown {
        commit_version: 107,
    }
    .into_transaction_info(12, 100);
    assert_eq!(info.status, "indeterminate");
    assert_eq!(info.reason, None);
    assert_eq!(info.commit_version, Some(107));
    assert_eq!(info.snapshot_version, Some(100));
    assert_eq!(info.visibility, None);
    assert_eq!(info.error.as_deref(), Some("wal_outcome_unknown"));
    assert_eq!(info.validate(), Ok(()));
    assert!(!info.is_committed());
    assert!(info.is_indeterminate());
}
