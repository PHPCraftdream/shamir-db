//! S4: current-state counter deltas, raw replay, and partial multi-table retry.

use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{stream, Stream};
use shamir_query_builder::{select::count_all, Query};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::{InMemoryRepo, InMemoryStore};
use shamir_storage::types::{RecordKey, Store};
use shamir_tx::{ChangeOp, ChangelogEvent, MvccStore, RecordChange};
use shamir_types::access::Actor;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::query::filter::eval_context::FilterContext;
use crate::repo::{BoxRepo, RepoInstance};
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;
use crate::tx::apply_replicated::{apply_replicated, ApplyOutcome, ApplyPolicy};

fn follower(tables: &[&str]) -> RepoInstance {
    RepoInstance::new(
        "follower".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        tables.iter().map(|name| TableConfig::new(*name)).collect(),
    )
}

fn change(table: &str, id: u8, op: ChangeOp) -> RecordChange {
    RecordChange {
        table: table.into(),
        key: RecordId([id; 16]).to_bytes(),
        op,
        value: (op == ChangeOp::Put).then(|| InnerValue::Str("row".into()).to_bytes().unwrap()),
    }
}

fn event(version: u64, changes: Vec<RecordChange>) -> ChangelogEvent {
    ChangelogEvent {
        repo: "leader".into(),
        commit_version: version,
        tx_id: version,
        actor: Actor::User(42),
        timestamp_ns: 0,
        changes,
    }
}

async fn apply(repo: &RepoInstance, event: &ChangelogEvent) -> u64 {
    // Intentionally never advance the watermark: exercise raw replay, NOT skip.
    match apply_replicated(repo, event, &[], 0, ApplyPolicy::Trusted)
        .await
        .unwrap()
    {
        ApplyOutcome::Applied { local_version } => local_version,
        outcome => panic!("expected raw apply, got {outcome:?}"),
    }
}

async fn count(repo: &RepoInstance, table: &str, expected: u64) {
    assert_eq!(
        repo.get_table(table)
            .await
            .unwrap()
            .counter()
            .get()
            .await
            .unwrap(),
        expected,
    );
}

// Red before S4: data is present but the follower's counter remains zero.
#[tokio::test]
async fn three_inserts_one_delete_count_is_two() {
    let repo = follower(&["items"]);
    apply(
        &repo,
        &event(
            1,
            (1..=3)
                .map(|id| change("items", id, ChangeOp::Put))
                .collect(),
        ),
    )
    .await;
    count(&repo, "items", 3).await;
    apply(&repo, &event(2, vec![change("items", 2, ChangeOp::Delete)])).await;
    count(&repo, "items", 2).await;
    let table = repo.get_table("items").await.unwrap();
    assert!(table.get_bytes(RecordId([1; 16])).await.unwrap().is_some());
    assert_eq!(table.get_bytes(RecordId([2; 16])).await.unwrap(), None);
    assert!(table.get_bytes(RecordId([3; 16])).await.unwrap().is_some());
    let interner = table.interner().get().await.unwrap();
    let refs = new_map();
    let ctx = FilterContext::new(interner, &refs);
    let query = Query::with_repo("follower", "items")
        .select([count_all("n")])
        .build();
    let result = table.read(&query, &ctx).await.unwrap();
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].get_value_u64("n"), Some(2));
}

// Red before S4 at the first count; naive footprint counting fails at replay.
#[tokio::test]
async fn raw_replay_without_advanced_watermark_does_not_double_count() {
    let repo = follower(&["items"]);
    let put = event(1, vec![change("items", 1, ChangeOp::Put)]);
    let first = apply(&repo, &put).await;
    count(&repo, "items", 1).await;
    assert!(apply(&repo, &put).await > first);
    count(&repo, "items", 1).await;
    let delete = event(2, vec![change("items", 1, ChangeOp::Delete)]);
    apply(&repo, &delete).await;
    count(&repo, "items", 0).await;
    apply(&repo, &delete).await;
    count(&repo, "items", 0).await;
}

// Each final operation is compared ONCE with its pre-image. Includes both
// absent and existing keys and an update (Put of an existing row).
#[tokio::test]
async fn repeated_ops_per_key_use_only_the_last_operation() {
    let repo = follower(&["items"]);
    apply(
        &repo,
        &event(
            1,
            vec![
                change("items", 1, ChangeOp::Put),
                change("items", 2, ChangeOp::Put),
            ],
        ),
    )
    .await;
    let mixed = event(
        2,
        vec![
            change("items", 1, ChangeOp::Delete),
            change("items", 1, ChangeOp::Put),
            change("items", 2, ChangeOp::Put),
            change("items", 2, ChangeOp::Delete),
            change("items", 3, ChangeOp::Put),
            change("items", 3, ChangeOp::Delete),
            change("items", 4, ChangeOp::Delete),
            change("items", 4, ChangeOp::Put),
            change("items", 4, ChangeOp::Put),
            change("items", 5, ChangeOp::Delete),
        ],
    );
    apply(&repo, &mixed).await;
    count(&repo, "items", 2).await;
    let table = repo.get_table("items").await.unwrap();
    for id in 1..=5 {
        assert_eq!(
            table.get_bytes(RecordId([id; 16])).await.unwrap().is_some(),
            id == 1 || id == 4,
        );
    }
    apply(&repo, &mixed).await;
    count(&repo, "items", 2).await;
}

// Fail the second table's pre-image I/O, before its data apply. This is a
// terminal partial apply: the first table is published without local WAL.
// A retry must be refused without spending another local version.
#[tokio::test]
async fn second_table_failure_requires_reopen_and_refuses_retry() {
    let repo = follower(&["items", "other"]);
    let gate = repo.tx_gate().await.unwrap();
    let changes = event(
        1,
        vec![
            change("items", 1, ChangeOp::Put),
            change("other", 2, ChangeOp::Put),
        ],
    );
    // Mirror the preflight grouping (same keys, insertion order, and Fx
    // hasher), rather than assume alphabetical or event table order.
    let mut order = shamir_collections::TFxMap::default();
    for change in &changes.changes {
        order
            .entry(change.table.clone())
            .or_insert(Vec::<()>::new());
    }
    let names: Vec<_> = order.keys().cloned().collect();
    let first = &names[0];
    let second = &names[1];
    for name in &names {
        repo.get_table(name).await.unwrap();
    }
    let fail = Arc::new(AtomicBool::new(true));
    let history: Arc<dyn Store> = Arc::new(FailPreImageOnce {
        inner: Arc::new(InMemoryStore::new()),
        fail: Arc::clone(&fail),
    });
    assert!(repo
        .per_table_mvcc()
        .remove_sync(&table_token_for(second))
        .is_some());
    let second_mvcc = Arc::new(MvccStore::new(history, Arc::clone(&gate)));
    assert!(repo
        .per_table_mvcc()
        .insert_sync(table_token_for(second), Arc::clone(&second_mvcc))
        .is_ok());
    let mut downstream = repo.subscribe_changelog().await.unwrap();
    let error = apply_replicated(&repo, &changes, &[], 0, ApplyPolicy::Trusted)
        .await
        .unwrap_err();
    assert!(matches!(error, DbError::RepoRequiresReopen(_)));
    assert!(
        !fail.load(Ordering::Acquire),
        "second table failure was reached",
    );
    let second_key = changes
        .changes
        .iter()
        .find(|change| &change.table == second)
        .unwrap()
        .key
        .clone();
    assert_eq!(
        second_mvcc.get_current_bytes(&second_key).await.unwrap(),
        None,
    );
    count(&repo, first, 1).await;
    count(&repo, second, 0).await;
    assert!(matches!(
        downstream.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
    assert!(gate.requires_reopen());
    let allocated = gate.version_allocation_high_water_mark();
    let retry = apply_replicated(&repo, &changes, &[], 0, ApplyPolicy::Trusted)
        .await
        .unwrap_err();
    assert!(matches!(retry, DbError::RepoRequiresReopen(_)));
    assert_eq!(gate.version_allocation_high_water_mark(), allocated);
    count(&repo, first, 1).await;
    count(&repo, second, 0).await;
}

type HistoryStream = Pin<Box<dyn Stream<Item = DbResult<Vec<(RecordKey, Bytes)>>> + Send>>;

struct FailPreImageOnce {
    inner: Arc<dyn Store>,
    fail: Arc<AtomicBool>,
}

#[async_trait]
impl Store for FailPreImageOnce {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.inner.set(key, value).await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }

    fn iter_stream(&self, batch_size: usize) -> HistoryStream {
        self.inner.iter_stream(batch_size)
    }

    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> HistoryStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }

    fn iter_range_stream(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        batch_size: usize,
    ) -> HistoryStream {
        if self.fail.swap(false, Ordering::AcqRel) {
            return Box::pin(stream::once(async {
                Err(DbError::Storage(
                    "injected second-table pre-image failure".into(),
                ))
            }));
        }
        self.inner.iter_range_stream(start, end, batch_size)
    }

    fn iter_range_stream_reverse(
        &self,
        start: Option<Bytes>,
        end: Option<Bytes>,
        batch_size: usize,
    ) -> HistoryStream {
        self.inner.iter_range_stream_reverse(start, end, batch_size)
    }
}
