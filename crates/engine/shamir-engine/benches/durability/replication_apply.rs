//! Successful follower apply through the receipt pipeline (the only follower
//! apply protocol since PR6b-3). Fixture shape:
//! tx/tests/apply_replicated_journal_order_tests.rs::put_event.
//! Default 1/16 rows, two increasing leader events per fresh follower
//! (the fresh repo self-adopts a v3 baseline on the first batch).
//! BENCH_REPLICATION_APPLY_SCALE=1 adds 64 rows. Setup/event encoding is
//! untimed; apply, local-version and byte assertions, and teardown are timed.
//! ns/op is one TWO-EVENT round, not per-row or pure acknowledgement latency.
//! Harness 0.1.0 / Tokio 1.49.0 are resolved in Cargo.lock; no new dependencies.

use std::sync::Arc;

use bench_scale_tool::Harness;
use shamir_engine::repo::{BoxRepo, RepoInstance};
use shamir_engine::table::table_manager::table_token_for;
use shamir_engine::table::TableConfig;
use shamir_engine::tx::{apply_replicated_batch, ApplyOutcome, ApplyPolicy, ReplicatedEvent};
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::Store;
use shamir_tx::{ChangelogEvent, IsolationLevel, TxContext, TxId};
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

fn event(n: usize, leader_version: u64, value: &str) -> ChangelogEvent {
    let mut tx = TxContext::new(TxId::new(leader_version), 0, 0, IsolationLevel::Snapshot);
    tx.set_actor(Actor::User(42));
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let staging = tx.ensure_table_staging(table_token_for("items"), "items", store);
    let payload = InnerValue::Str(value.into()).to_bytes().unwrap();
    for i in 0..n {
        let id = record_id(i);
        staging.set(id.to_bytes().into(), payload.clone());
    }
    shamir_tx::project_event(&tx, "leader", leader_version).unwrap()
}

fn record_id(i: usize) -> RecordId {
    // Same 16-byte typed key shape as put_event, expanded without collisions.
    RecordId(u128::try_from(i + 1).unwrap().to_be_bytes())
}

// Apply one event as a ONE-EVENT batch through the receipt pipeline
// (`apply_replicated_batch` is the only follower apply API). Keep the same
// success and byte obligations; never time Skipped instead.
async fn apply(repo: &RepoInstance, event: &ChangelogEvent, bookmark: u64) -> u64 {
    assert!(event.commit_version > bookmark);
    let events = [ReplicatedEvent {
        event: event.clone(),
        field_bindings: Vec::new(),
    }];
    match apply_replicated_batch(repo, &events, bookmark, ApplyPolicy::Trusted)
        .await
        .unwrap()
    {
        ApplyOutcome::Applied { local_version } => local_version,
        other => panic!("benchmark requires Applied, got {other:?}"),
    }
}

fn main() {
    let mut h = Harness::new("replication_apply", env!("CARGO_MANIFEST_DIR"));
    let mut sizes = vec![1, 16];
    if std::env::var("BENCH_REPLICATION_APPLY_SCALE")
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
    {
        sizes.push(64);
    }
    for n in sizes {
        h.bench_batched_async(
            &format!("replication_apply/applied_two_events/rows_{n}"),
            move || async move {
                let repo = RepoInstance::new(
                    "follower".into(),
                    BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
                    vec![TableConfig::new("items")],
                );
                let table = repo.get_table("items").await.unwrap();
                let gate = repo.tx_gate().await.unwrap();
                let before = gate.last_committed();
                (repo, table, before, event(n, 7, "A"), event(n, 9, "B"))
            },
            move |(repo, table, before, a, b)| async move {
                let mut previous = before;
                for (event, watermark, value) in [(&a, 0, "A"), (&b, 7, "B")] {
                    let local = apply(&repo, event, watermark).await;
                    assert!(local > previous, "local versions must advance");
                    assert_eq!(repo.tx_gate().await.unwrap().last_committed(), local);
                    let expected = InnerValue::Str(value.into()).to_bytes().unwrap();
                    for i in 0..n {
                        assert_eq!(
                            table.get_bytes(record_id(i)).await.unwrap(),
                            Some(expected.clone()),
                            "applied row bytes must be visible before return",
                        );
                    }
                    previous = local;
                }
                assert_eq!(table.count().await.unwrap(), n);
            },
        );
    }
    h.run();
}
