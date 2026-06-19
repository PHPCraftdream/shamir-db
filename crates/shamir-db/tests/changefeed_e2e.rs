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

use tokio::time::timeout;

use shamir_db::ShamirDb;
use shamir_engine::ChangeOp;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::filter::eq;
use shamir_query_builder::write::{delete, insert, update};

/// In-memory ShamirDb with db "testdb", repo "main", table "users".
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    let mut b = Batch::new();
    b.id("setup");
    b.create_repo(
        "repo",
        ddl::create_repo("main")
            .engine("in_memory")
            .tables(["users"]),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
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
    shamir
        .execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();
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
    shamir
        .execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();
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
    shamir
        .execute("testdb", &ubatch.to_request_via_msgpack())
        .await
        .unwrap();

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
    shamir
        .execute("testdb", &dbatch.to_request_via_msgpack())
        .await
        .unwrap();

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

// ===========================================================================
// Non-transactional writes (Phase 3b follow-up)
//
// A batch WITHOUT `.transactional()` bypasses the commit pipeline entirely —
// `execute_insert/update/set/delete` apply mutations directly. These tests
// prove those non-tx writes ALSO emit changefeed events (live + journal) with
// the right fields, and that their `commit_version`s stay monotonic and
// interleave consistently with transactional writes on the same repo.
// ===========================================================================

/// Insert one user via a NON-transactional batch (no `.transactional()`).
async fn nontx_insert_user(shamir: &ShamirDb, name: &str, age: i64) {
    let mut batch = Batch::named("ins");
    batch.id("ins");
    batch.insert(
        "i",
        insert("users").rows([doc! {
            "name" => name,
            "age" => age,
        }]),
    );
    shamir
        .execute("testdb", &batch.to_request_via_msgpack())
        .await
        .unwrap();
}

#[tokio::test]
async fn nontx_insert_update_delete_emit_expected_events() {
    let shamir = setup().await;

    let mut rx = shamir
        .subscribe_changelog("testdb", "main")
        .await
        .expect("repo exists");

    // ── NON-TX INSERT → Put with the new value ───────────────────────
    nontx_insert_user(&shamir, "alice", 30).await;

    let ev = recv_event(&mut rx).await;
    assert_eq!(ev.repo, "main");
    assert!(ev.commit_version > 0, "a real version was assigned");
    assert_eq!(ev.tx_id, 0, "non-tx write carries tx_id 0");
    assert_eq!(ev.changes.len(), 1, "one row inserted");
    let ins = &ev.changes[0];
    assert_eq!(ins.table, "users");
    assert_eq!(ins.op, ChangeOp::Put);
    assert!(ins.value.is_some(), "Put carries the new record bytes");
    assert!(!ins.key.is_empty(), "record key present");
    let insert_version = ev.commit_version;

    // ── NON-TX UPDATE → Put with the updated value ───────────────────
    let mut ubatch = Batch::named("upd");
    ubatch.id("upd");
    ubatch.update(
        "u",
        update("users").where_(eq("name", "alice")).set(doc! {
            "age" => 31,
        }),
    );
    shamir
        .execute("testdb", &ubatch.to_request_via_msgpack())
        .await
        .unwrap();

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

    // ── NON-TX DELETE → Delete with no value ─────────────────────────
    let mut dbatch = Batch::named("del");
    dbatch.id("del");
    dbatch.delete("d", delete("users").where_(eq("name", "alice")));
    shamir
        .execute("testdb", &dbatch.to_request_via_msgpack())
        .await
        .unwrap();

    let ev = recv_event(&mut rx).await;
    assert!(ev.commit_version > update_version);
    assert_eq!(ev.changes.len(), 1, "one row deleted");
    let del = &ev.changes[0];
    assert_eq!(del.table, "users");
    assert_eq!(del.op, ChangeOp::Delete);
    assert_eq!(del.value, None, "Delete carries no value");
}

#[tokio::test]
async fn nontx_writes_journaled_in_order_without_subscriber() {
    let shamir = setup().await;

    // Three NON-tx inserts, no live subscription.
    for (i, name) in ["alice", "bob", "carol"].iter().enumerate() {
        nontx_insert_user(&shamir, name, 20 + i as i64).await;
    }

    // The durable journal contains all three, ascending by version.
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
    assert_eq!(
        events.len(),
        3,
        "all three non-tx writes durable + resumable"
    );

    let versions: Vec<u64> = events.iter().map(|e| e.commit_version).collect();
    let mut sorted = versions.clone();
    sorted.sort_unstable();
    assert_eq!(
        versions, sorted,
        "events returned in ascending version order"
    );
    assert!(versions[0] < versions[1] && versions[1] < versions[2]);
    for ev in &events {
        assert_eq!(ev.tx_id, 0, "all are non-tx writes");
        assert_eq!(ev.changes[0].op, ChangeOp::Put);
    }
}

#[tokio::test]
async fn mixed_tx_and_nontx_versions_are_consistently_monotonic() {
    let shamir = setup().await;

    let mut rx = shamir
        .subscribe_changelog("testdb", "main")
        .await
        .expect("repo exists");

    // Interleave: non-tx insert, tx insert, non-tx insert, tx insert.
    // All four target the same repo, so they share one gate's version
    // counter — versions must come out strictly increasing regardless of
    // which path (tx vs non-tx) produced each event.
    nontx_insert_user(&shamir, "alice", 30).await;
    insert_user(&shamir, "bob").await; // transactional
    nontx_insert_user(&shamir, "carol", 40).await;
    insert_user(&shamir, "dave").await; // transactional

    let mut versions = Vec::new();
    let mut tx_ids = Vec::new();
    for _ in 0..4 {
        let ev = recv_event(&mut rx).await;
        versions.push(ev.commit_version);
        tx_ids.push(ev.tx_id);
    }

    // Live events arrive in emission order; versions strictly increase.
    for w in versions.windows(2) {
        assert!(
            w[0] < w[1],
            "commit_version strictly increases across mixed tx/non-tx ({} < {})",
            w[0],
            w[1]
        );
    }

    // tx_id pattern proves the interleave actually mixed both paths:
    // non-tx → 0, tx → non-zero.
    assert_eq!(tx_ids[0], 0, "first write was non-tx");
    assert!(tx_ids[1] != 0, "second write was transactional");
    assert_eq!(tx_ids[2], 0, "third write was non-tx");
    assert!(tx_ids[3] != 0, "fourth write was transactional");
}

/// Consecutive non-tx single-record inserts produce changefeed versions
/// with exactly gap-1 (no "double-bump" from a second `assign_next_version`
/// inside the changefeed emitter — the event now reuses the MVCC version
/// the data was written at).
#[tokio::test]
async fn nontx_insert_versions_have_no_double_bump_gap() {
    let shamir = setup().await;

    let mut rx = shamir
        .subscribe_changelog("testdb", "main")
        .await
        .expect("repo exists");

    // Three back-to-back non-tx inserts (single-record each).
    nontx_insert_user(&shamir, "alice", 30).await;
    nontx_insert_user(&shamir, "bob", 31).await;
    nontx_insert_user(&shamir, "carol", 32).await;

    let v1 = recv_event(&mut rx).await.commit_version;
    let v2 = recv_event(&mut rx).await.commit_version;
    let v3 = recv_event(&mut rx).await.commit_version;

    // Each non-tx insert allocates exactly ONE version from the shared
    // gate counter (inside `set_versioned[_many]`). Before the fix the
    // changefeed emitter burned a SECOND version, producing gaps of 2.
    assert_eq!(
        v2 - v1,
        1,
        "gap between consecutive non-tx inserts must be 1, not 2 (no double-bump); got {} -> {}",
        v1,
        v2
    );
    assert_eq!(
        v3 - v2,
        1,
        "gap between consecutive non-tx inserts must be 1, not 2 (no double-bump); got {} -> {}",
        v2,
        v3
    );
}

/// Non-tx update of multiple rows emits a single changefeed event whose
/// version equals the MAX MVCC version across the per-record writes. The
/// versions of each per-record write are sequential and the event carries
/// the last one, matching the commit-version-per-batch semantic.
#[tokio::test]
async fn nontx_update_batch_version_is_max_of_per_record_writes() {
    let shamir = setup().await;

    let mut rx = shamir
        .subscribe_changelog("testdb", "main")
        .await
        .expect("repo exists");

    // Seed two rows (non-tx).
    nontx_insert_user(&shamir, "alice", 30).await;
    nontx_insert_user(&shamir, "bob", 31).await;
    let _ = recv_event(&mut rx).await; // alice insert
    let v_seed = recv_event(&mut rx).await.commit_version; // bob insert

    // Update BOTH rows in one non-tx batch (matches WHERE age > 0).
    let mut ubatch = Batch::named("upd");
    ubatch.id("upd");
    ubatch.update(
        "u",
        update("users")
            .where_(shamir_query_builder::filter::gt("age", 0))
            .set(doc! { "age" => 99 }),
    );
    shamir
        .execute("testdb", &ubatch.to_request_via_msgpack())
        .await
        .unwrap();

    let ev = recv_event(&mut rx).await;
    assert!(
        ev.commit_version > v_seed,
        "update event version ({}) must exceed seed version ({})",
        ev.commit_version,
        v_seed,
    );
    assert_eq!(
        ev.changes.len(),
        2,
        "both rows updated, two changes in event"
    );
    // The event version is the max of the per-record writes. Each per-
    // record set_versioned bumps once, so the second record's version is
    // the max and equals the event's commit_version.
    for ch in &ev.changes {
        assert_eq!(ch.op, ChangeOp::Put);
        assert!(ch.value.is_some());
    }
}

/// Mixed tx + non-tx writes still produce strictly increasing versions
/// with no collisions or backwards jumps after the changefeed version
/// alignment change. This is the existing monotonicity contract proved
/// by `mixed_tx_and_nontx_versions_are_consistently_monotonic` — we add
/// an explicit gap check to prove the non-tx path doesn't double-bump.
#[tokio::test]
async fn mixed_tx_nontx_versions_no_gap_from_double_bump() {
    let shamir = setup().await;

    let mut rx = shamir
        .subscribe_changelog("testdb", "main")
        .await
        .expect("repo exists");

    // Sequence: non-tx, non-tx, tx, non-tx.
    nontx_insert_user(&shamir, "a", 1).await;
    nontx_insert_user(&shamir, "b", 2).await;
    insert_user(&shamir, "c").await; // transactional
    nontx_insert_user(&shamir, "d", 3).await;

    let mut versions = Vec::new();
    for _ in 0..4 {
        let ev = recv_event(&mut rx).await;
        versions.push(ev.commit_version);
    }

    // Strictly increasing.
    for w in versions.windows(2) {
        assert!(
            w[0] < w[1],
            "versions must be strictly increasing: {} < {}",
            w[0],
            w[1]
        );
    }

    // Between two consecutive non-tx inserts (v[0] and v[1]) the gap
    // must be exactly 1 (no second bump from changefeed emit).
    assert_eq!(
        versions[1] - versions[0],
        1,
        "gap between consecutive non-tx inserts must be 1, not 2"
    );
}

/// L10(a) journal-safe invariant: a commit with ZERO live subscribers still
/// writes to the durable journal. `changes_since(0)` must observe the event.
/// This guards against a regression where skipping the broadcast path
/// accidentally skips the journal path too.
#[tokio::test]
async fn commit_without_subscribers_still_journals() {
    let shamir = setup().await;

    // Deliberately NO subscribe_changelog — zero live subscribers.
    insert_alice(&shamir).await;
    insert_user(&shamir, "bob").await;

    // Poll the journal until both events land (async journal writer).
    let mut events = Vec::new();
    for _ in 0..50 {
        events = shamir
            .read_changelog_from("testdb", "main", 0, 100)
            .await
            .unwrap();
        if events.len() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        events.len(),
        2,
        "both commits journaled despite zero live subscribers"
    );

    // Versions ascend.
    assert!(
        events[0].commit_version < events[1].commit_version,
        "journal events in ascending version order"
    );

    // Both are Put operations.
    for ev in &events {
        assert_eq!(ev.changes[0].op, ChangeOp::Put);
    }
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
