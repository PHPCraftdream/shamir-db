//! Integration tests for T4-changes-since — one-shot "changes since version V"
//! journal read (the queryable foundation of #201).
//!
//! Verifies that:
//! - `ChangesSince { changes_since: 0 }` after a few commits returns the
//!   committed journal events (count ≥ the writes, `commit_version` ascends).
//! - A high cursor returns fewer / no events (resumable pull semantics).
//! - The result carries `gap_at` (null when no gap — the normal case).
//! - A principal without `Read` is denied.

use std::time::Duration;

use serde_json::json;

use shamir_db::access::Actor;
use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::write::insert;

/// Standard in-memory test fixture: database `testdb`, repo `main`, table
/// `users`. Mirrors the `purge_history.rs` / `retention_ddl.rs` setup.
async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

/// Insert a single named user inside its own transactional batch (each call
/// is its own commit → one changefeed event).
async fn insert_user(shamir: &ShamirDb, name: &str) {
    let mut b = Batch::new();
    b.id(1);
    b.transactional();
    b.insert(
        "i",
        insert("users").rows([doc! {
            "name" => name,
            "age"  => 30_i64,
        }]),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
}

/// Run `ChangesSince { changes_since: cursor }` against `(testdb, main)` and
/// return the parsed result object `{ changes_since, events, gap_at }`.
async fn run_changes_since(shamir: &ShamirDb, cursor: u64) -> serde_json::Value {
    let mut b = Batch::new();
    b.id(2);
    b.changes_since("cs", ddl::changes_since(cursor));
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .expect("ChangesSince execute");
    serde_json::Value::from(resp.results["cs"].records[0].as_value().into_owned())
}

// ---------------------------------------------------------------------------
// Test 1: changes_since: 0 returns the committed events, ascending
// ---------------------------------------------------------------------------

/// After three transactional inserts, `ChangesSince { changes_since: 0 }`
/// returns at least three events whose `commit_version`s strictly ascend.
///
/// The durable journal is flushed asynchronously by a background writer, so
/// the read is retried until the expected count is observed (mirrors the
/// polling pattern in `changefeed_e2e.rs`).
#[tokio::test]
async fn changes_since_zero_returns_committed_events_ascending() {
    let shamir = setup_shamir().await;

    insert_user(&shamir, "alice").await;
    insert_user(&shamir, "alice2").await;
    insert_user(&shamir, "alice3").await;

    // Poll until the journal has caught up.
    let mut result = serde_json::Value::Null;
    for _ in 0..100 {
        result = run_changes_since(&shamir, 0).await;
        if result["events"].as_array().map(|a| a.len()).unwrap_or(0) >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let events = result["events"].as_array().expect("events is an array");
    assert!(
        events.len() >= 3,
        "expected at least 3 events, got {}; journal may have failed to flush",
        events.len()
    );

    // commit_version strictly ascends.
    let versions: Vec<u64> = events
        .iter()
        .map(|e| e["commit_version"].as_u64().expect("commit_version is u64"))
        .collect();
    for w in versions.windows(2) {
        assert!(
            w[0] < w[1],
            "commit_version must strictly ascend ({} < {})",
            w[0],
            w[1]
        );
    }

    // The echoed cursor matches the request.
    assert_eq!(result["changes_since"], json!(0u64));
    // gap_at is present in the result shape (null when no gap — the normal
    // case for these low-volume writes).
    assert!(
        result.get("gap_at").is_some(),
        "gap_at must be surfaced to the client (null or a version)"
    );
    assert_eq!(result["gap_at"], json!(null), "no gap expected here");

    // Sanity: each event names the repo and the users table.
    for ev in events {
        assert_eq!(ev["repo"], json!("main"));
        let changes = ev["changes"].as_array().expect("changes is an array");
        assert!(!changes.is_empty(), "each event carries >= 1 record change");
        assert_eq!(changes[0]["table"], json!("users"));
    }
}

// ---------------------------------------------------------------------------
// Test 2: a high cursor returns fewer / no events
// ---------------------------------------------------------------------------

/// `ChangesSince` at a cursor at/above the highest committed version returns
/// an empty events list (resumable pull past the tail).
#[tokio::test]
async fn changes_since_high_cursor_returns_fewer_events() {
    let shamir = setup_shamir().await;

    insert_user(&shamir, "bob").await;

    // First find the highest committed version via a changes_since: 0 read.
    let mut highest: u64 = 0;
    for _ in 0..100 {
        let result = run_changes_since(&shamir, 0).await;
        if let Some(arr) = result["events"].as_array() {
            if !arr.is_empty() {
                if let Some(Some(v)) = arr.iter().map(|e| e["commit_version"].as_u64()).max() {
                    highest = v;
                    break;
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(highest > 0, "expected at least one committed event");

    // Cursor AT the highest version → strict-after semantics → empty.
    let result = run_changes_since(&shamir, highest).await;
    let events = result["events"].as_array().expect("events is an array");
    assert!(
        events.is_empty(),
        "cursor at the highest version must return no events, got {}",
        events.len()
    );
    assert_eq!(result["changes_since"], json!(highest));
    assert_eq!(result["gap_at"], json!(null));
}

// ---------------------------------------------------------------------------
// Test 3: gap_at is surfaced (null in the no-gap case)
// ---------------------------------------------------------------------------

/// Confirms the result always carries a `gap_at` field. In the normal
/// low-volume case it is `null`; the CF-1 re-sync signal is never swallowed
/// by the wire shape. (A non-null gap requires engineering a journal-channel
/// overflow, which is exercised at the changefeed unit-test layer.)
#[tokio::test]
async fn result_always_carries_gap_at_field() {
    let shamir = setup_shamir().await;
    insert_user(&shamir, "carol").await;

    let result = run_changes_since(&shamir, 0).await;
    assert!(
        result.get("gap_at").is_some(),
        "gap_at MUST be present in the ChangesSince result (CF-1 re-sync signal)"
    );
}

// ---------------------------------------------------------------------------
// Test 4: access denied without Read permission
// ---------------------------------------------------------------------------

/// A principal without `Read` permission is denied `ChangesSince`.
/// Mirrors `purge_denied_without_permission` in `purge_history.rs`.
#[tokio::test]
async fn changes_since_denied_without_read() {
    let shamir = setup_shamir().await;
    insert_user(&shamir, "dave").await;

    // Lock the database to owner-only (mode 0o700); chown to uid=1.
    // User(999) falls into "other" → no access bits → Read denied.
    let mut b = Batch::new();
    b.id(1);
    let chown_h = b.chown("chown", ddl::chown(ddl::res::database("testdb"), 1));
    let chmod_h = b.chmod("chmod", ddl::chmod(ddl::res::database("testdb"), 0o700));
    b.after(&chmod_h, &chown_h);
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    // Non-owner tries ChangesSince → access_denied.
    let mut b = Batch::new();
    b.id(2);
    b.changes_since("cs", ddl::changes_since(0));
    let err = shamir
        .execute_as(Actor::User(999), "testdb", &b.to_request_via_msgpack())
        .await
        .expect_err("User(999) must be denied ChangesSince");
    assert_eq!(
        err.code(),
        Some("access_denied"),
        "expected code 'access_denied', got: {:?} ({})",
        err.code(),
        err
    );
}
