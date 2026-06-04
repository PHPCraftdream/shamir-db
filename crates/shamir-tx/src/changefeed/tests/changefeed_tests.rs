//! Unit tests for the per-repo changefeed (live broadcast + journal).

use std::sync::Arc;

use bytes::Bytes;
use scc::HashMap as SccMap;
use shamir_types::access::Actor;

use crate::changefeed::{
    project_event, version_key, ChangeOp, ChangelogEvent, ChangelogStore, RecordChange,
    RepoChangefeed,
};
use crate::staging_store::StagingStore;
use crate::types::{IsolationLevel, TxId};
use crate::TxContext;

/// In-memory `ChangelogStore` fake: an ordered map keyed by the BE-8
/// version bytes, so `range_from` is a simple ascending walk.
#[derive(Default)]
struct MemChangelogStore {
    // BE-8 key bytes -> serialized event bytes.
    inner: SccMap<Vec<u8>, Vec<u8>>,
}

#[async_trait::async_trait]
impl ChangelogStore for MemChangelogStore {
    async fn put(&self, key: Bytes, value: Bytes) -> Result<(), String> {
        let _ = self.inner.upsert(key.to_vec(), value.to_vec());
        Ok(())
    }

    async fn range_from(&self, from_key: Bytes, limit: usize) -> Result<Vec<Bytes>, String> {
        let from = from_key.to_vec();
        let mut pairs: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        self.inner.scan(|k, v| {
            if k.as_slice() >= from.as_slice() {
                pairs.push((k.clone(), v.clone()));
            }
        });
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(pairs
            .into_iter()
            .take(limit)
            .map(|(_, v)| Bytes::from(v))
            .collect())
    }
}

fn mem_base() -> Arc<dyn shamir_storage::types::Store> {
    Arc::new(shamir_storage::storage_in_memory::InMemoryStore::new())
}

/// Build a tx that staged a Put + a Delete on table "users".
async fn tx_with_writes(tx_id: u64) -> TxContext {
    let mut tx = TxContext::new(TxId::new(tx_id), 7, 10, IsolationLevel::Snapshot);
    let staging = StagingStore::new(mem_base());
    // 16-byte record-id-shaped keys.
    staging
        .set(
            Bytes::from_static(b"0123456789abcdef"),
            Bytes::from_static(b"alice"),
        )
        .await;
    staging
        .remove(Bytes::from_static(b"fedcba9876543210"))
        .await;
    let token = 42u64;
    tx.write_set.insert(token, staging);
    tx.table_tokens.insert(token, "users".to_string());
    tx
}

#[tokio::test]
async fn project_event_carries_put_value_and_delete() {
    let tx = tx_with_writes(1).await;
    let ev = project_event(&tx, "main", 5).expect("non-empty footprint projects an event");

    assert_eq!(ev.repo, "main");
    assert_eq!(ev.commit_version, 5);
    assert_eq!(ev.tx_id, 1);
    assert_eq!(ev.actor, Actor::System);
    assert_eq!(ev.changes.len(), 2);

    let put = ev
        .changes
        .iter()
        .find(|c| c.op == ChangeOp::Put)
        .expect("a Put change");
    assert_eq!(put.table, "users");
    assert_eq!(put.key, Bytes::from_static(b"0123456789abcdef"));
    assert_eq!(put.value.as_deref(), Some(b"alice".as_ref()));

    let del = ev
        .changes
        .iter()
        .find(|c| c.op == ChangeOp::Delete)
        .expect("a Delete change");
    assert_eq!(del.table, "users");
    assert_eq!(del.key, Bytes::from_static(b"fedcba9876543210"));
    assert_eq!(del.value, None);
}

#[tokio::test]
async fn project_event_empty_tx_is_none() {
    let tx = TxContext::new(TxId::new(9), 0, 0, IsolationLevel::Snapshot);
    assert!(project_event(&tx, "main", 1).is_none());
}

#[tokio::test]
async fn live_subscriber_receives_emitted_event() {
    let store: Arc<dyn ChangelogStore> = Arc::new(MemChangelogStore::default());
    let feed = RepoChangefeed::new(Arc::clone(&store));

    let mut rx = feed.subscribe();
    assert_eq!(feed.subscriber_count(), 1);

    let tx = tx_with_writes(2).await;
    let ev = project_event(&tx, "main", 11).unwrap();
    feed.emit(ev);

    let got = rx.recv().await.expect("event delivered live");
    assert_eq!(got.commit_version, 11);
    assert_eq!(got.changes.len(), 2);
}

#[tokio::test]
async fn multiple_live_subscribers_receive_same_event() {
    let store: Arc<dyn ChangelogStore> = Arc::new(MemChangelogStore::default());
    let feed = RepoChangefeed::new(store);

    let mut a = feed.subscribe();
    let mut b = feed.subscribe();
    assert_eq!(feed.subscriber_count(), 2);

    let tx = tx_with_writes(3).await;
    feed.emit(project_event(&tx, "main", 20).unwrap());

    let ga = a.recv().await.unwrap();
    let gb = b.recv().await.unwrap();
    assert_eq!(ga.commit_version, 20);
    assert_eq!(gb.commit_version, 20);
    // Same projection shared by both subscribers (Arc).
    assert!(Arc::ptr_eq(&ga, &gb));
}

#[tokio::test]
async fn emit_without_subscribers_does_not_panic() {
    let store: Arc<dyn ChangelogStore> = Arc::new(MemChangelogStore::default());
    let feed = RepoChangefeed::new(store);
    assert_eq!(feed.subscriber_count(), 0);
    let tx = tx_with_writes(4).await;
    // No subscribers: live send errors internally and is ignored; journal
    // try_send still enqueues. Must not panic / block.
    feed.emit(project_event(&tx, "main", 30).unwrap());
}

#[tokio::test]
async fn journal_persists_and_read_from_returns_in_order() {
    let store: Arc<dyn ChangelogStore> = Arc::new(MemChangelogStore::default());
    let feed = RepoChangefeed::new(Arc::clone(&store));

    // Emit N events WITHOUT any live subscriber — journal must still record.
    for v in 1u64..=5 {
        let tx = tx_with_writes(100 + v).await;
        feed.emit(project_event(&tx, "main", v).unwrap());
    }
    feed.shutdown(); // drain the writer

    // Poll until the writer has flushed all 5 (background task).
    let mut events = Vec::new();
    for _ in 0..50 {
        events = feed.read_from(&store, 0, 100).await;
        if events.len() == 5 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(events.len(), 5, "all journal events durable + resumable");

    // Ascending by commit_version.
    let versions: Vec<u64> = events.iter().map(|e| e.commit_version).collect();
    assert_eq!(versions, vec![1, 2, 3, 4, 5]);

    // Resumable: read from version 3 onward.
    let tail = feed.read_from(&store, 3, 100).await;
    let tail_versions: Vec<u64> = tail.iter().map(|e| e.commit_version).collect();
    assert_eq!(tail_versions, vec![3, 4, 5]);

    // Limit respected.
    let limited = feed.read_from(&store, 0, 2).await;
    assert_eq!(limited.len(), 2);
    assert_eq!(limited[0].commit_version, 1);
    assert_eq!(limited[1].commit_version, 2);
}

#[tokio::test]
async fn late_subscriber_catches_up_via_journal_then_live() {
    let store: Arc<dyn ChangelogStore> = Arc::new(MemChangelogStore::default());
    let feed = RepoChangefeed::new(Arc::clone(&store));

    // Three commits happen BEFORE anyone subscribes.
    for v in 1u64..=3 {
        let tx = tx_with_writes(200 + v).await;
        feed.emit(project_event(&tx, "main", v).unwrap());
    }
    feed.flush_hint();

    // Wait for the journal to hold all 3.
    let mut past = Vec::new();
    for _ in 0..50 {
        past = feed.read_from(&store, 0, 100).await;
        if past.len() == 3 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        past.len(),
        3,
        "late subscriber reads the past from the journal"
    );
    assert_eq!(
        past.iter().map(|e| e.commit_version).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );

    // Now subscribe and emit a new (4th) commit — caught live.
    let mut rx = feed.subscribe();
    let tx = tx_with_writes(204).await;
    feed.emit(project_event(&tx, "main", 4).unwrap());
    let live = rx.recv().await.unwrap();
    assert_eq!(live.commit_version, 4);
}

#[tokio::test]
async fn event_round_trips_through_msgpack() {
    let original = ChangelogEvent {
        repo: "main".to_string(),
        commit_version: 77,
        tx_id: 9,
        actor: Actor::User(123),
        timestamp_ns: 42,
        changes: vec![
            RecordChange {
                table: "users".to_string(),
                key: Bytes::from_static(b"k1"),
                op: ChangeOp::Put,
                value: Some(Bytes::from_static(b"v1")),
            },
            RecordChange {
                table: "users".to_string(),
                key: Bytes::from_static(b"k2"),
                op: ChangeOp::Delete,
                value: None,
            },
        ],
    };
    let bytes = rmp_serde::to_vec(&original).unwrap();
    let decoded: ChangelogEvent = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn version_key_is_big_endian_and_ordered() {
    let k1 = version_key(1);
    let k2 = version_key(2);
    let k256 = version_key(256);
    assert!(k1 < k2);
    assert!(k2 < k256);
    assert_eq!(k1.as_ref(), &[0, 0, 0, 0, 0, 0, 0, 1]);
}
