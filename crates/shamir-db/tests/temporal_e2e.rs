//! T6 — temporal end-to-end lifecycle.
//!
//! ONE integration test that drives the WHOLE temporal feature through the
//! full `ShamirDb::execute` stack, proving the individually-built pieces
//! (T3 retention DDL, T4-asof/history/purge/changes-since, T5 builder reads)
//! compose into a single lifecycle:
//!
//! create+retention → write history → as_of → history → changes_since
//! → set_retention → purge → verify
//!
//! Clock control: each table owns an `MvccStore` whose wall clock is
//! overridable via `set_test_now(ms)` (0 restores the real clock). We freeze
//! it at increasing timestamps so every write gets a distinct, deterministic
//! commit ts. Write VERSIONS (the monotonic counter from `RepoTxGate`) are
//! captured synchronously from the live changefeed subscriber — no polling.
//!
//! Mirrors `retention_ddl.rs` / `purge_history.rs` / `changes_since.rs` /
//! `changefeed_e2e.rs` for setup, MvccStore access, and clock control.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio::time::timeout;

use shamir_db::engine::repo::repo_types::BoxRepoFactory;
use shamir_db::engine::repo::RepoConfig;
use shamir_db::engine::table::table_manager::table_token_for;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::doc;
use shamir_query_builder::filter::eq;
use shamir_query_builder::write::{insert, update};
use shamir_query_builder::Query;
use shamir_query_types::admin::{PurgeScope, Retention};

// ---------------------------------------------------------------------------
// Fixtures & helpers
// ---------------------------------------------------------------------------

/// In-memory ShamirDb with database `testdb`, repo `main`, NO pre-created
/// table (the test creates its own table via DDL to exercise the retention
/// knob on `CreateTable`).
async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config = RepoConfig::new("main", BoxRepoFactory::in_memory());
    db.add_repo(repo_config).await.unwrap();
    shamir
}

/// Force the lazy `MvccStore` entry for `(repo, table)` and return the
/// `Arc<MvccStore>`. Mirrors `purge_history.rs::get_mvcc` exactly: the entry
/// is populated lazily on first `get_table`, so we touch it first.
async fn get_mvcc(
    shamir: &ShamirDb,
    db_name: &str,
    repo: &str,
    table: &str,
) -> Arc<shamir_tx::MvccStore> {
    let db = shamir.get_db(db_name).expect("db exists");
    let _ = db.get_table(repo, table).await.expect("table exists");
    let repo_instance = db.get_repo(repo).expect("repo exists");
    let token = table_token_for(table);
    // Clone the Arc while the scc entry guard is still in scope (the guard
    // borrows the map which borrows repo_instance — can't return the chained
    // expression). Mirrors `purge_history.rs::get_mvcc` exactly.
    let mvcc_arc = repo_instance
        .per_table_mvcc()
        .get(&token)
        .expect("mvcc entry exists")
        .clone();
    mvcc_arc
}

/// Snapshot the table's live retention policy from the `MvccStore`
/// (lock-free `ArcSwap` load). Mirrors `retention_ddl.rs::table_retention`
/// but returns the `shamir_tx::Retention` directly.
async fn table_retention(
    shamir: &ShamirDb,
    db_name: &str,
    repo: &str,
    table: &str,
) -> shamir_tx::Retention {
    let mvcc = get_mvcc(shamir, db_name, repo, table).await;
    **mvcc.retention()
}

/// Insert a single record `(name, age)` into `table` via its own batch.
async fn insert_record(shamir: &ShamirDb, db: &str, table: &str, name: &str, age: i64) {
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "i",
        insert(table).rows([doc! {
            "name" => name,
            "age"  => age,
        }]),
    );
    shamir
        .execute(db, &b.to_request_via_msgpack())
        .await
        .unwrap();
}

/// Update the record named `name` to `new_age` via its own batch.
async fn update_record(shamir: &ShamirDb, db: &str, table: &str, name: &str, new_age: i64) {
    let mut b = Batch::new();
    b.id(2);
    b.update(
        "u",
        update(table)
            .where_(eq("name", name))
            .set(doc! { "name" => name, "age" => new_age }),
    );
    shamir
        .execute(db, &b.to_request_via_msgpack())
        .await
        .unwrap();
}

/// Receive the next live changefeed event within 5s and return its
/// `commit_version`. Mirrors `changefeed_e2e.rs::recv_event`. This is how we
/// capture the monotonic write VERSION for each write — synchronously, no
/// polling, fully deterministic.
async fn recv_version(
    rx: &mut tokio::sync::broadcast::Receiver<Arc<shamir_engine::ChangelogEvent>>,
) -> u64 {
    let ev = timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("changefeed event within 5s")
        .expect("broadcast receiver healthy");
    ev.commit_version
}

/// Run a `Query` (any temporal mode) filtered by `name` and return its
/// record list. The query already carries its target table via `Query::from`.
async fn run_query(shamir: &ShamirDb, db: &str, q: Query) -> Vec<serde_json::Value> {
    let mut b = Batch::new();
    b.id(3);
    b.query("q", q.where_eq("name", "alice").clone());
    let resp = shamir
        .execute(db, &b.to_request_via_msgpack())
        .await
        .expect("query execute");
    resp.results["q"]
        .records
        .iter()
        .map(|r| r.as_json().into_owned())
        .collect()
}

/// Run `ChangesSince { changes_since: cursor }` against `(db, main)` and
/// return the parsed result object. Mirrors `changes_since.rs::run_changes_since`.
async fn run_changes_since(shamir: &ShamirDb, db: &str, cursor: u64) -> serde_json::Value {
    let mut b = Batch::new();
    b.id(4);
    b.changes_since("cs", ddl::changes_since(cursor));
    let resp = shamir
        .execute(db, &b.to_request_via_msgpack())
        .await
        .expect("ChangesSince execute");
    resp.results["cs"].records[0].as_json().into_owned()
}

// ---------------------------------------------------------------------------
// The lifecycle
// ---------------------------------------------------------------------------

/// The full temporal lifecycle, end to end through `ShamirDb::execute`.
///
/// Clock plan (frozen `MvccStore` clock, ms):
///   v1 insert  @ ts = 1_000
///   v2 update  @ ts = 10_000
///   v3 update  @ ts = 20_000
///   v4 update  @ ts = 30_000
///
/// Each step's assertion is non-vacuous: it fails if the corresponding
/// feature (CreateTable-retention, AsOf, History, ChangesSince, SetRetention,
/// PurgeHistory) regresses.
#[tokio::test]
async fn temporal_lifecycle_e2e() {
    let shamir = setup_shamir().await;

    // Subscribe to the live changefeed BEFORE any write so we can capture
    // each write's commit_version synchronously (no journal-flush polling).
    let mut rx = shamir
        .subscribe_changelog("testdb", "main")
        .await
        .expect("repo exists");

    // ── 1. CREATE table with retention { max_count: 5 } ──────────────────
    // Exercises T3: `CreateTable` carrying `retention` wires the policy into
    // the table's `MvccStore` at creation time.
    let mut b = Batch::new();
    b.id(1);
    b.create_table(
        "ct",
        ddl::create_table("users").retention(Retention {
            max_count: Some(5),
            ..Default::default()
        }),
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        resp.results["ct"].records[0].as_json()["created_table"],
        json!("users")
    );
    assert_eq!(
        resp.results["ct"].records[0].as_json()["created"],
        json!(true)
    );
    // Fails if CreateTable silently drops the retention knob.
    let policy = table_retention(&shamir, "testdb", "main", "users").await;
    assert_eq!(policy.max_count, Some(5), "CreateTable retention applied");

    // Grab the MvccStore handle for clock control (frozen for the whole
    // history-writing phase).
    let mvcc = get_mvcc(&shamir, "testdb", "main", "users").await;

    // ── 2. WRITE HISTORY — insert + 3 updates at increasing frozen ts ────
    // Each write is its own batch → its own commit → a distinct version.
    // We capture the version via the live changefeed.
    mvcc.set_test_now(1_000);
    insert_record(&shamir, "testdb", "users", "alice", 30).await; // age=30
    let v1 = recv_version(&mut rx).await;
    mvcc.set_test_now(10_000);
    update_record(&shamir, "testdb", "users", "alice", 31).await; // age=31
    let v2 = recv_version(&mut rx).await;
    mvcc.set_test_now(20_000);
    update_record(&shamir, "testdb", "users", "alice", 32).await; // age=32
    let v3 = recv_version(&mut rx).await;
    mvcc.set_test_now(30_000);
    update_record(&shamir, "testdb", "users", "alice", 33).await; // age=33
    let v4 = recv_version(&mut rx).await;

    // Versions strictly ascend (would fail if the changefeed reused / skipped
    // a version, or if writes were coalesced).
    assert!(
        v1 < v2 && v2 < v3 && v3 < v4,
        "versions ascend: {v1}<{v2}<{v3}<{v4}"
    );

    // ── 3. ASOF — early version returns early value; latest differs ──────
    // Exercises T4-asof: `ReadQuery { temporal: AsOf { at: Version(v2) } }`
    // resolves the record state AS IT WAS at v2.
    let early = run_query(&shamir, "testdb", Query::from("users").as_of_version(v2)).await;
    assert_eq!(early.len(), 1, "as_of returns the one alice record");
    let early_age = early[0]["age"].as_i64().expect("age is int");
    // At v2 the age was 31. Fails if AsOf returns the current (33) or v1 (30).
    assert_eq!(early_age, 31, "as_of(v2) returns the v2 value (age=31)");

    let late = run_query(&shamir, "testdb", Query::from("users").as_of_version(v4)).await;
    let late_age = late[0]["age"].as_i64().expect("age is int");
    // Fails if AsOf and Latest disagree on the current state.
    assert_eq!(late_age, 33, "as_of(v4) returns the latest value (age=33)");
    assert_ne!(early_age, late_age, "as_of early vs latest differ");

    // ── 4. HISTORY — full timeline ascending, _version rows ──────────────
    // Exercises T4-history: `ReadQuery { temporal: History }` returns one row
    // per version with `_version` injected, ascending.
    let hist = run_query(&shamir, "testdb", Query::from("users").history()).await;
    // 4 writes → 4 timeline rows (current only retention is OFF, max_count=5).
    assert_eq!(hist.len(), 4, "history covers all 4 writes");
    let versions: Vec<u64> = hist
        .iter()
        .filter_map(|r| r.get("_version").and_then(|v| v.as_u64()))
        .collect();
    // _version must be present and ascending. Fails if History stops
    // injecting _version or returns out-of-order rows.
    assert_eq!(
        versions,
        vec![v1, v2, v3, v4],
        "history _version ascends across all writes"
    );
    let ages: Vec<i64> = hist
        .iter()
        .filter_map(|r| r.get("age").and_then(|v| v.as_i64()))
        .collect();
    // Fails if History returns the wrong value at a version.
    assert_eq!(
        ages,
        vec![30, 31, 32, 33],
        "history values match each write"
    );

    // ── 5. CHANGESSINCE — committed events ascending, carries gap_at ─────
    // Exercises T4-changes-since. The durable journal is flushed async by a
    // background writer, so we poll until the expected count lands (mirrors
    // `changes_since.rs`).
    let mut result = serde_json::Value::Null;
    for _ in 0..100 {
        result = run_changes_since(&shamir, "testdb", 0).await;
        if result["events"].as_array().map(|a| a.len()).unwrap_or(0) >= 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let events = result["events"].as_array().expect("events is an array");
    assert!(
        events.len() >= 4,
        "ChangesSince returns >= 4 events, got {}",
        events.len()
    );
    let cs_versions: Vec<u64> = events
        .iter()
        .map(|e| e["commit_version"].as_u64().expect("commit_version is u64"))
        .collect();
    for w in cs_versions.windows(2) {
        // Fails if ChangesSince returns events out of order.
        assert!(w[0] < w[1], "commit_version ascends ({} < {})", w[0], w[1]);
    }
    // gap_at MUST be present in the result shape (CF-1 re-sync signal).
    // Fails if the wire shape ever swallows gap_at.
    assert!(
        result.get("gap_at").is_some(),
        "gap_at present in ChangesSince result"
    );
    assert_eq!(
        result["gap_at"],
        json!(null),
        "no gap in the low-volume case"
    );

    // ── 6. PURGEHISTORY — absolute cutoff removes old versions ──────────
    // Exercises T4-purge. Use `OlderThan { timestamp }` (absolute, deterministic
    // — independent of the store clock). This runs BEFORE the SetRetention
    // tighten below so the full 4-version history is still intact (vacuuming
    // under a tight max_count would otherwise reclaim the very versions we
    // want to purge, making purged == 0). With cutoff = 25_000 the versions
    // at ts 1_000/10_000/20_000 are eligible; the snapshot anchor (largest
    // eligible) is kept but the rest are reclaimed.
    //
    // Restore the real clock so the execute call isn't accidentally affected
    // by the frozen value (OlderThan ignores it, but we stay defensive).
    mvcc.set_test_now(0);

    // Snapshot the history length BEFORE purge so we can prove it shrank.
    let before_purge = run_query(&shamir, "testdb", Query::from("users").history()).await;
    let before_len = before_purge.len();
    assert_eq!(before_len, 4, "full history present before purge");

    let mut b = Batch::new();
    b.id(5);
    b.purge_history(
        "ph",
        ddl::purge_history("users", PurgeScope::OlderThan { timestamp: 25_000 }),
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    let result = resp.results["ph"].records[0].as_json();
    assert_eq!(result["purge_history"], json!("users"));
    assert_eq!(result["repo"], json!("main"));
    let purged = result["purged"].as_u64().expect("purged is u64");
    // At least one old version (ts < 25_000) must be reclaimed.
    // Fails if purge_below_ts's ts-predicate path is broken (purged == 0).
    assert!(
        purged >= 1,
        "PurgeHistory removed >= 1 old version (ts < 25_000), got {purged}"
    );

    // ── 7. VERIFY — history shrank, current value intact ─────────────────
    let after_purge = run_query(&shamir, "testdb", Query::from("users").history()).await;
    // The purge must have actually removed rows from the readable history.
    // Fails if purge reported a count but didn't delete from the store.
    assert!(
        after_purge.len() < before_len,
        "history shrank after purge (before={}, after={})",
        before_len,
        after_purge.len()
    );
    // The versions that survive must all have ts >= cutoff (25_000) OR be the
    // snapshot anchor. Concretely: v4 (ts=30_000) survives; v1/v2/v3 are gone
    // except possibly the anchor. So at most 2 rows remain (v4 + anchor).
    assert!(
        after_purge.len() <= 2,
        "at most current + anchor survive (got {})",
        after_purge.len()
    );

    // The CURRENT value is untouched by purge (purge_below_ts never touches
    // the in-main current version). A Latest read returns age=33.
    let latest = run_query(&shamir, "testdb", Query::from("users")).await;
    assert_eq!(latest.len(), 1, "Latest read returns the one alice record");
    let latest_age = latest[0]["age"].as_i64().expect("age is int");
    assert_eq!(
        latest_age, 33,
        "Latest read still correct after purge (age=33)"
    );
    // Latest must NOT attach _version (only History does).
    // Fails if a regression accidentally leaks _version into Latest reads.
    assert!(
        latest[0].get("_version").is_none(),
        "Latest read must not carry _version"
    );

    // ── 8. SETRETENTION on the fly — policy changes, history bounded ─────
    // Exercises T3: `SetRetention` swaps the live policy (lock-free ArcSwap).
    // Tighten to max_count: 2, then write two MORE updates. Each write
    // archives the prior current version and vacuum_key reclaims down to
    // max_count (+ the snapshot anchor). The live policy must change and the
    // subsequent history must be bounded by it.
    let mut b = Batch::new();
    b.id(6);
    b.set_retention(
        "sr",
        ddl::set_retention(
            "users",
            Retention {
                max_count: Some(2),
                ..Default::default()
            },
        )
        .repo("main"),
    );
    let resp = shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
    assert_eq!(
        resp.results["sr"].records[0].as_json()["set_retention"],
        json!("users")
    );
    assert_eq!(resp.results["sr"].records[0].as_json()["ok"], json!(true));
    // Fails if SetRetention didn't take effect on the MvccStore.
    let after = table_retention(&shamir, "testdb", "main", "users").await;
    assert_eq!(after.max_count, Some(2), "live policy now max_count=2");

    // Two more updates at later timestamps. Each archives the prior current
    // version, then vacuum_key reclaims down to max_count (+ the anchor).
    mvcc.set_test_now(40_000);
    update_record(&shamir, "testdb", "users", "alice", 34).await; // v5
    let _v5 = recv_version(&mut rx).await;
    mvcc.set_test_now(50_000);
    update_record(&shamir, "testdb", "users", "alice", 35).await; // v6
    let _v6 = recv_version(&mut rx).await;
    mvcc.set_test_now(0);

    let hist_bounded = run_query(&shamir, "testdb", Query::from("users").history()).await;
    // max_count=2 bounds the ARCHIVED (old) versions; the current version is
    // always in main. The vacuum floor also keeps the single anchor (largest
    // version < min_alive) so the effective history size is
    //   <= max_count (archived) + anchor + current.
    // We assert a documented UPPER BOUND (not exact count) since the precise
    // survivor set depends on the snapshot floor at each vacuum. The bound is
    // non-vacuous: an UNbounded policy (None) with 6 total writes would yield
    // 6 history rows; max_count=2 keeps it well under that.
    assert!(
        hist_bounded.len() <= 4,
        "bounded history ({}) must stay small under max_count=2 (archived + \
         anchor + current), not the unbounded 6",
        hist_bounded.len()
    );
    // The current value is still correct after the retention tighten.
    let cur_age = hist_bounded
        .last()
        .and_then(|r| r.get("age").and_then(|v| v.as_i64()))
        .expect("current value present");
    assert_eq!(
        cur_age, 35,
        "current value preserved after retention tighten"
    );
}

// ---------------------------------------------------------------------------
// Focused sub-test: AsOf by timestamp mirrors the version path
// ---------------------------------------------------------------------------

/// `as_of_timestamp(ts)` resolves the state as of a wall-clock millisecond
/// cutoff, complementing the version-based AsOf in the lifecycle test.
///
/// Uses the SAME frozen-clock setup: v1 @ ts=1_000, v2 @ ts=10_000. An
/// `as_of_timestamp(5_000)` read falls between them and must resolve to v1's
/// value (age=30), proving the timestamp path of AsOf, not just the version
/// path.
///
/// Fails if `as_of_timestamp` returns the latest value instead of the
/// point-in-time value.
#[tokio::test]
async fn as_of_timestamp_resolves_point_in_time() {
    let shamir = setup_shamir().await;

    let mut b = Batch::new();
    b.id(1);
    b.create_table(
        "ct",
        ddl::create_table("users").retention(Retention {
            max_count: Some(5),
            ..Default::default()
        }),
    );
    shamir
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();

    let mvcc = get_mvcc(&shamir, "testdb", "main", "users").await;

    mvcc.set_test_now(1_000);
    insert_record(&shamir, "testdb", "users", "alice", 30).await;
    mvcc.set_test_now(10_000);
    update_record(&shamir, "testdb", "users", "alice", 31).await;
    mvcc.set_test_now(0);

    // ts=5_000 is AFTER v1 (1_000) but BEFORE v2 (10_000) → resolves to v1.
    let rows = run_query(
        &shamir,
        "testdb",
        Query::from("users").as_of_timestamp(5_000),
    )
    .await;
    assert_eq!(rows.len(), 1);
    let age = rows[0]["age"].as_i64().expect("age is int");
    // Fails if as_of_timestamp returns the current (31) instead of v1 (30).
    assert_eq!(age, 30, "as_of_timestamp(5_000) resolves to v1 (age=30)");

    // ts=15_000 is after both → resolves to v2 (current).
    let rows = run_query(
        &shamir,
        "testdb",
        Query::from("users").as_of_timestamp(15_000),
    )
    .await;
    let age = rows[0]["age"].as_i64().expect("age is int");
    assert_eq!(age, 31, "as_of_timestamp(15_000) resolves to v2 (age=31)");
}
