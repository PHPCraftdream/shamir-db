//! End-to-end tests for the Phase 3b changefeed (hybrid live-push +
//! durable journal) exercised through the public `ShamirDb` API.
//!
//! Two tracks are proven:
//!   * **Live push** — `subscribe_changelog` → write → the subscriber
//!     receives a `ChangelogEvent` with the right repo / commit_version /
//!     table / op / value. insert→Put(value), update→Put(value),
//!     delete→Delete(None). Multiple live subscribers see the same event.
//!   * **Durable journal** — commits with NO live subscriber are still
//!     journaled; `read_changelog_from(0)` returns them in ascending
//!     `commit_version` order (resumable). A late subscriber catches the
//!     past via the journal, then catches new commits live.
//!   * **Non-blocking** — a commit with no subscribers proceeds.

use std::time::Duration;

use serde_json::json;
use tokio::time::timeout;

use shamir_db::query::batch::BatchRequest;
use shamir_db::ShamirDb;
use shamir_engine::ChangeOp;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::doc;
use shamir_query_builder::filter::eq;
use shamir_query_builder::write::{delete, insert, update};

/// In-memory ShamirDb with db "testdb", repo "main", table "users".
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    let setup: BatchRequest = serde_json::from_value(json!({
        "id": "setup",
        "queries": {
            "repo": {
                "create_repo": "main",
                "engine": "in_memory",
                "tables": ["users"]
            }
        }
    }))
    .unwrap();
    shamir.execute("testdb", &setup).await.unwrap();
    shamir
}

async fn insert_alice(shamir: &ShamirDb) {
    let mut batch = Batch::named("ins");
    batch.id("ins");
    batch.transactional();
    batch.insert(
        "i",
        insert("users").rows([doc! {
            "name" => "alice",
            "age" => 30,
        }]),
    );
    shamir.execute("testdb", &batch.build()).await.unwrap();
}

/// Insert a single named user inside a transactional batch (each call is
/// its own commit → one changefeed event).
async fn insert_user(shamir: &ShamirDb, name: &str) {
    let mut batch = Batch::named("ins");
    batch.id("ins");
    batch.transactional();
    batch.insert(
        "i",
        insert("users").rows([doc! {
            "name" => name,
            "age" => 20,
        }]),
    );
    shamir.execute("testdb", &batch.build()).await.unwrap();
}

/// Receive the next live event within a bounded timeout (so a hang in the
/// feed surfaces as a test failure, not an indefinite block).
async fn recv_event(
    rx: &mut tokio::sync::broadcast::Receiver<std::sync::Arc<shamir_engine::ChangelogEvent>>,
) -> std::sync::Arc<shamir_engine::ChangelogEvent> {
    timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("changefeed event within 5s")
        .expect("broadcast receiver healthy")
}

#[tokio::test]
async fn live_insert_update_delete_emit_expected_events() {
    let shamir = setup().await;

    let mut rx = shamir
        .subscribe_changelog("testdb", "main")
        .await
        .expect("repo exists");

    // ── INSERT → Put with the new value ──────────────────────────────
    insert_alice(&shamir).await;

    let ev = recv_event(&mut rx).await;
    assert_eq!(ev.repo, "main");
    assert!(ev.commit_version > 0, "a real version was assigned");
    assert_eq!(ev.changes.len(), 1, "one row inserted");
    let ins = &ev.changes[0];
    assert_eq!(ins.table, "users");
    assert_eq!(ins.op, ChangeOp::Put);
    assert!(ins.value.is_some(), "Put carries the new record bytes");
    assert!(!ins.key.is_empty(), "record key present");
    let insert_version = ev.commit_version;

    // ── UPDATE → Put with the updated value ──────────────────────────
    let mut ubatch = Batch::named("upd");
    ubatch.id("upd");
    ubatch.transactional();
    ubatch.update(
        "u",
        update("users").where_(eq("name", "alice")).set(doc! {
            "age" => 31,
        }),
    );
    shamir.execute("testdb", &ubatch.build()).await.unwrap();

    let ev = recv_event(&mut rx).await;
    assert!(
        ev.commit_version > insert_version,
        "commit_version strictly increases ({} > {insert_version})",
        ev.commit_version
    );
    assert_eq!(ev.changes.len(), 1, "one row updated");
    let upd = &ev.changes[0];
    assert_eq!(upd.table, "users");
    assert_eq!(upd.op, ChangeOp::Put, "an update is a Put of the new value");
    assert!(upd.value.is_some());
    let update_version = ev.commit_version;

    // ── DELETE → Delete with no value ────────────────────────────────
    let mut dbatch = Batch::named("del");
    dbatch.id("del");
    dbatch.transactional();
    dbatch.delete("d", delete("users").where_(eq("name", "alice")));
    shamir.execute("testdb", &dbatch.build()).await.unwrap();

    let ev = recv_event(&mut rx).await;
    assert!(ev.commit_version > update_version);
    assert_eq!(ev.changes.len(), 1, "one row deleted");
    let del = &ev.changes[0];
    assert_eq!(del.table, "users");
    assert_eq!(del.op, ChangeOp::Delete);
    assert_eq!(del.value, None, "Delete carries no value");
}

#[tokio::test]
async fn multiple_live_subscribers_receive_same_event() {
    let shamir = setup().await;

    let mut a = shamir.subscribe_changelog("testdb", "main").await.unwrap();
    let mut b = shamir.subscribe_changelog("testdb", "main").await.unwrap();

    insert_alice(&shamir).await;

    let ea = recv_event(&mut a).await;
    let eb = recv_event(&mut b).await;
    assert_eq!(ea.commit_version, eb.commit_version);
    assert_eq!(ea.changes.len(), 1);
    assert_eq!(eb.changes.len(), 1);
    assert_eq!(ea.changes[0].table, "users");
    assert_eq!(eb.changes[0].op, ChangeOp::Put);
}

#[tokio::test]
async fn commit_without_subscribers_succeeds() {
    let shamir = setup().await;
    // No subscriber at all — the commit must proceed and be journaled.
    insert_alice(&shamir).await;

    // Poll the durable journal until the event lands.
    let mut events = Vec::new();
    for _ in 0..50 {
        events = shamir
            .read_changelog_from("testdb", "main", 0, 100)
            .await
            .unwrap();
        if !events.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(events.len(), 1, "commit without subscribers is journaled");
    assert_eq!(events[0].changes[0].op, ChangeOp::Put);
}

#[tokio::test]
async fn journal_resumable_read_from_in_order() {
    let shamir = setup().await;

    // Three commits, NO live subscription.
    for name in ["alice", "bob", "carol"] {
        insert_user(&shamir, name).await;
    }

    // read_changelog_from(0) returns all three, ascending by version.
    let mut events = Vec::new();
    for _ in 0..50 {
        events = shamir
            .read_changelog_from("testdb", "main", 0, 100)
            .await
            .unwrap();
        if events.len() == 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(events.len(), 3, "all three commits durable + resumable");

    let versions: Vec<u64> = events.iter().map(|e| e.commit_version).collect();
    let mut sorted = versions.clone();
    sorted.sort_unstable();
    assert_eq!(
        versions, sorted,
        "events returned in ascending version order"
    );
    assert!(versions[0] < versions[1] && versions[1] < versions[2]);

    // Resumable: read strictly after the first version.
    let tail = shamir
        .read_changelog_from("testdb", "main", versions[0] + 1, 100)
        .await
        .unwrap();
    assert_eq!(tail.len(), 2, "two events after the first version");
    assert_eq!(tail[0].commit_version, versions[1]);
    assert_eq!(tail[1].commit_version, versions[2]);

    // Limit honoured.
    let limited = shamir
        .read_changelog_from("testdb", "main", 0, 1)
        .await
        .unwrap();
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].commit_version, versions[0]);
}

#[tokio::test]
async fn late_subscriber_catches_past_via_journal_then_live() {
    let shamir = setup().await;

    // Two commits BEFORE anyone subscribes.
    for name in ["alice", "bob"] {
        insert_user(&shamir, name).await;
    }

    // Catch the past via the durable journal.
    let mut past = Vec::new();
    for _ in 0..50 {
        past = shamir
            .read_changelog_from("testdb", "main", 0, 100)
            .await
            .unwrap();
        if past.len() == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        past.len(),
        2,
        "late reader recovers the past from the journal"
    );
    let last_seen = past.iter().map(|e| e.commit_version).max().unwrap();

    // Now subscribe and commit a third — caught live.
    let mut rx = shamir.subscribe_changelog("testdb", "main").await.unwrap();
    insert_user(&shamir, "carol").await;

    let live = recv_event(&mut rx).await;
    assert!(
        live.commit_version > last_seen,
        "the live event is newer than everything caught from the journal"
    );
}

#[tokio::test]
async fn subscribe_unknown_repo_returns_none() {
    let shamir = setup().await;
    assert!(shamir.subscribe_changelog("testdb", "nope").await.is_none());
    assert!(shamir.subscribe_changelog("nodb", "main").await.is_none());
    assert!(shamir
        .read_changelog_from("nodb", "main", 0, 10)
        .await
        .is_none());
}
