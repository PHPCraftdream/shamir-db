//! Tests for transactional batch execution (SI happy path, TxOutcome mapping).

use shamir_query_builder::batch::Batch;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_types::access::Actor;

use crate::query::batch::execute_batch;

use super::common::TxTestResolver;

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
}

#[test]
fn deferred_outcome_maps_to_materialized_false() {
    use crate::tx::tx_outcome::MaterializationState;

    let complete = crate::tx::TxOutcome {
        tx_id: 11,
        snapshot_version: 100,
        commit_version: 101,
        materialization: MaterializationState::Complete,
        background: None,
    };
    let deferred = crate::tx::TxOutcome {
        tx_id: 12,
        snapshot_version: 100,
        commit_version: 102,
        materialization: MaterializationState::Deferred,
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
