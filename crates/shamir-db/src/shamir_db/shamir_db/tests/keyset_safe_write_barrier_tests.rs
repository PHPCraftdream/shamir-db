//! F-37 (#845, P0) — `keyset_safe` activation write-barrier tests (lib scope).
//!
//! These exercise the REAL `ShamirDb::execute` schema DDL path
//! (`SetTableSchema` / `AddSchemaRule`) — the same entry point a client request
//! uses — and prove two things:
//!
//! 1. **Regression (non-racing case):** a `SetTableSchema` (and
//!    `AddSchemaRule`) DDL on a genuinely EMPTY table still stamps
//!    `keyset_safe = true` onto a new/type-changed rule exactly as before
//!    F-37 — no functional regression to the already-correct non-racing path.
//!    (No pre-existing F-17 test covered this: `declarative_schema_stamping_e2e`
//!    is `created_at`/`updated_at` stamping, not `keyset_safe`;
//!    `schema_rollback_tests` uses `keyset_safe: false` literals and never
//!    exercises the empty-table proof through the real DDL handler. Confirmed
//!    by grepping `crates/shamir-db` for `keyset_safe`.)
//!
//! 2. **Barrier engagement:** the DDL genuinely acquires the table's shared
//!    `unique_write_lock` (the SAME lock the non-tx writer path takes when
//!    `needs_write_barrier()` is true). Held externally, a `SetTableSchema`
//!    DDL BLOCKS until the lock is released — proving the F-37 wiring engages
//!    the write barrier the engine-level test
//!    (`table::tests::schema_activation_barrier_tests`) proved serializes
//!    writers. By composition: during the DDL's count→persist→activate window
//!    the barrier flag is up + the lock is held → a concurrent INSERT blocks
//!    on the lock.
//!
//! The mechanical "writer blocks while the barrier flag is up" proof lives at
//! the engine level, where the primitive (`needs_write_barrier()` +
//! `schema_activation_barrier`) lives; these tests prove the DDL path above it
//! actually drives that primitive.

use std::time::Duration;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::types::value::QueryValue;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::query::batch::BatchRequest;
use crate::ShamirDb;

// ── helpers ──────────────────────────────────────────────────────────────

/// In-memory ShamirDb with a single empty table `testdb/main/users`.
async fn setup() -> ShamirDb {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo("testdb", repo_config).await.unwrap();
    db
}

/// Run a `set_table_schema` DDL (one string rule on `email`) and return the
/// response result map.
async fn exec_set_schema(db: &ShamirDb) -> QueryValue {
    let rules: Vec<_> = vec![ddl::field(["email"]).string().required().build()];
    let mut b = Batch::new();
    b.id(1);
    b.set_table_schema("ss", ddl::set_table_schema("users").rules(rules));
    let resp = db
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .expect("set_table_schema should succeed");
    resp.results["ss"].records[0].as_value().as_ref().clone()
}

/// Read back the catalogue schema + version via `GetTableSchema`.
async fn read_schema(db: &ShamirDb) -> QueryValue {
    let mut b = Batch::new();
    b.id(1);
    b.get_table_schema("gs", ddl::get_table_schema("users"));
    let resp = db
        .execute("testdb", &b.to_request_via_msgpack())
        .await
        .expect("get_table_schema should succeed");
    resp.results["gs"].records[0].as_value().as_ref().clone()
}

/// Extract the persisted `keyset_safe` flag for the first rule in a
/// `GetTableSchema` response. Absent (serde default) means `false`.
fn first_rule_keyset_safe(schema_resp: &QueryValue) -> Option<bool> {
    let schema = schema_resp.get("schema")?;
    let first = schema.as_array()?.first()?;
    first.get("keyset_safe").and_then(|v| v.as_bool())
}

/// Build a `set_table_schema` request for one string rule on `email`.
fn set_schema_request() -> BatchRequest {
    let rules: Vec<_> = vec![ddl::field(["email"]).string().required().build()];
    let mut b = Batch::new();
    b.id(1);
    b.set_table_schema("ss", ddl::set_table_schema("users").rules(rules));
    b.to_request_via_msgpack()
}

// ═══════════════════════════════════════════════════════════════════════
// 1. Regression: DDL on a genuinely empty table stamps keyset_safe = true
//    (the non-racing case, unchanged by F-37).
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn keyset_safe_true_for_empty_table_after_set_table_schema() {
    let db = setup().await;

    // Table is genuinely empty: no rows yet.
    let users = db.get_table("testdb", "main", "users").await.unwrap();
    assert_eq!(
        users.counter().get().await.unwrap(),
        0,
        "precondition: empty table"
    );

    let r = exec_set_schema(&db).await;
    assert_eq!(r.get("ok"), Some(&QueryValue::Bool(true)));

    // The new string rule on `email` must carry keyset_safe = true: the table
    // was empty at bind time, so the count()==0 proof holds.
    let schema = read_schema(&db).await;
    assert_eq!(
        first_rule_keyset_safe(&schema),
        Some(true),
        "F-17/F-37: a new rule bound on an empty table must be keyset_safe = true \
         (no regression to the non-racing proof path)"
    );
}

/// AddSchemaRule shares `stamp_keyset_safe`'s count-based proof and therefore
/// F-37's barrier fix. Regression: adding a NEW rule to an empty table also
/// stamps `keyset_safe = true`.
#[tokio::test]
async fn keyset_safe_true_for_empty_table_after_add_schema_rule() {
    let db = setup().await;
    let users = db.get_table("testdb", "main", "users").await.unwrap();
    assert_eq!(users.counter().get().await.unwrap(), 0);

    // Add a single rule via add_schema_rule.
    let rule = ddl::field(["name"]).string().required().build();
    let mut b = Batch::new();
    b.id(1);
    b.add_schema_rule("asr", ddl::add_schema_rule("users").rule(rule));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .expect("add_schema_rule should succeed");

    let schema = read_schema(&db).await;
    assert_eq!(
        first_rule_keyset_safe(&schema),
        Some(true),
        "AddSchemaRule's shared stamp_keyset_safe proof must stamp keyset_safe = true \
         for a new rule on an empty table"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// 2. Barrier engagement: the SetTableSchema DDL acquires unique_write_lock.
//
// Determinism: the test pre-acquires the table's `unique_write_lock` (the SAME
// Arc the DDL's `begin_schema_activation_barrier` locks — `TableManager::clone`
// Arc-shares it), spawns the DDL, and asserts it BLOCKS (it is parked inside
// `begin_schema_activation_barrier` on `unique_write_lock().lock_owned().await`).
// Releasing the lock lets the DDL proceed and stamp keyset_safe = true. This is
// the sibling of `create_index_v2_acquires_write_barrier` (engine) applied to
// the schema DDL — proving the DDL engages the barrier the engine-level test
// proved serializes writers.
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn set_table_schema_ddl_acquires_unique_write_barrier() {
    let db = setup().await;

    // Pre-acquire the table's unique_write_lock — the exact lock the F-37 DDL
    // path takes inside begin_schema_activation_barrier.
    let users = db.get_table("testdb", "main", "users").await.unwrap();
    let held = users.unique_write_lock().lock_owned().await;

    // Spawn the DDL; it must block acquiring the lock we hold.
    let db_ddl = db.clone();
    let ddl_task =
        tokio::spawn(async move { db_ddl.execute("testdb", &set_schema_request()).await });

    // Give the spawned DDL time to dispatch through execute_batch and reach the
    // lock acquisition in begin_schema_activation_barrier. Robust: the DDL
    // structurally cannot complete while we hold the lock (mirrors the engine
    // sibling test's is_finished() probe).
    tokio::time::sleep(Duration::from_millis(120)).await;
    assert!(
        !ddl_task.is_finished(),
        "F-37: set_table_schema must BLOCK on unique_write_lock held externally — \
         proving the DDL engages the schema-activation write barrier (pre-fix it \
         would complete immediately, proving the keyset_safe count-proof race \
         window was open)"
    );

    // Release the lock — the DDL acquires it, raises the barrier, runs the
    // count→persist→activate sequence, and stamps keyset_safe.
    drop(held);
    ddl_task
        .await
        .unwrap()
        .expect("set_table_schema must complete once the lock is released");

    // The non-racing proof still held: empty table → keyset_safe = true.
    let schema = read_schema(&db).await;
    assert_eq!(
        first_rule_keyset_safe(&schema),
        Some(true),
        "after the barrier releases, the DDL must still stamp keyset_safe = true \
         for the empty table"
    );
}
