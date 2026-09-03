//! Phase C1 — no-deadlock proof for `ValidatorDb`.
//!
//! These tests prove that a validator reading database state through
//! `ctx.db()` during an active write transaction does **not** deadlock.
//! The write path runs validators **before** staging/locking, so the
//! validator's reads on `tx.snapshot_version` never contend with the
//! tx's own (not-yet-acquired) write locks.
//!
//! Two scenarios are covered:
//! - **FK (cross-table)**: validator on table A reads table B via
//!   `exists_in` — independent table, independent lock space.
//! - **Unique (self-table)**: validator on table A reads table A via
//!   `exists_in_self` — same table, committed-only read path (no
//!   `acquire_pessimistic_read_lock`), so no self-deadlock.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use bytes::Bytes;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::RecordKey;
use shamir_tx::IsolationLevel;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::{InnerValue, QueryValue};

use crate::query::TableRef;
use crate::query::TableResolver;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{RepoConfig, RepoInstance};
use crate::table::{TableConfig, TableManager};
use crate::validator::{
    encode::Validation, record_fields::RecordFields, record_validator::RecordValidator,
    record_validator::ValidatorCtx, ValidatorBinding, ValidatorRegistry, WriteOp,
};

// ── Test TableResolver ─────────────────────────────────────────────────────

/// Resolver backed by a single `RepoInstance` — resolves any table in it.
/// Injects the validator registry into every resolved table (mirrors
/// `DbTableResolver` in production).
struct TestResolver {
    repo: RepoInstance,
    validators: Arc<ValidatorRegistry>,
}

#[async_trait::async_trait]
impl TableResolver for TestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        let mut table = self.repo.get_table(&table_ref.table).await?;
        table.set_validator_registry(Arc::clone(&self.validators));
        Ok(table)
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        Ok(self.repo.clone())
    }
}

// ── Async validator that reads ctx.db() ────────────────────────────────────

/// A validator that exercises both `exists_in` (cross-table FK) and
/// `exists_in_self` (self-table unique) inside `validate()`.
///
/// Uses `tokio::time::timeout` internally so a deadlock surfaces as a
/// clean test failure rather than a hung nextest process.
struct DbReadingValidator {
    /// The FK target table (cross-table read).
    fk_table: String,
    /// The field to probe on the FK table.
    fk_field: String,
    /// The field to probe on the self table (unique check).
    unique_field: String,
}

#[async_trait]
impl RecordValidator for DbReadingValidator {
    async fn validate(
        &self,
        new: Option<&dyn RecordFields>,
        _old: Option<&dyn RecordFields>,
        ctx: &ValidatorCtx<'_>,
    ) -> Validation {
        let Some(db) = ctx.db() else {
            // No DB handle — accept (fail-open).
            return Validation::accept();
        };
        let Some(record) = new else {
            return Validation::accept();
        };

        // Extract the FK field value from the record being written.
        let fk_value = match record.scalar(&[&self.unique_field]) {
            Some(scalar) => scalar_to_query_value(&scalar),
            None => return Validation::accept(),
        };

        // Wrap in a timeout so a deadlock is caught cleanly.
        let probe = async {
            // 1. Cross-table FK read: check if the value exists in the
            //    referenced table. This reads a DIFFERENT table on the tx's
            //    snapshot — no write locks held on it.
            let fk_ref = TableRef::new(&self.fk_table);
            let _fk_exists = db
                .exists_in(&fk_ref, &self.fk_field, &fk_value)
                .await
                .unwrap_or(false);

            // 2. Self-table unique read: check if the value exists in the
            //    same table being written. This reads committed state only
            //    (no pessimistic lock on write-path keys).
            let _unique_exists = db
                .exists_in_self(&self.unique_field, &fk_value, None)
                .await
                .unwrap_or(false);

            Ok::<(), shamir_storage::error::DbError>(())
        };

        match tokio::time::timeout(std::time::Duration::from_secs(5), probe).await {
            Ok(Ok(())) => Validation::accept(),
            Ok(Err(_)) => Validation::reject("db_read_error"),
            Err(_) => {
                // Timeout = deadlock.
                let mut v = Validation::reject("deadlock_timeout");
                v.stop();
                v
            }
        }
    }
}

/// Convert a `ScalarRef` to a comparable `QueryValue`.
fn scalar_to_query_value(scalar: &shamir_types::record_view::ScalarRef<'_>) -> QueryValue {
    use shamir_types::record_view::ScalarRef;
    match scalar {
        ScalarRef::Null => QueryValue::Null,
        ScalarRef::Bool(b) => QueryValue::Bool(*b),
        ScalarRef::Int(i) => QueryValue::Int(*i),
        ScalarRef::F64(f) => QueryValue::F64(*f),
        ScalarRef::Str(s) => QueryValue::Str((*s).to_string()),
        ScalarRef::Bin(b) => QueryValue::Bin(b.to_vec()),
    }
}

// ── Test setup ─────────────────────────────────────────────────────────────

/// Build a repo with two tables (`child` and `parent`), a `DbReadingValidator`
/// bound to `child`, and return the resolver + table handles.
async fn setup() -> (TestResolver, TableManager) {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let repo = RepoInstance::from_factory(
        repo_config.name.clone(),
        repo_config.factory,
        repo_config.tables,
    )
    .await
    .unwrap();

    // Register the DbReadingValidator on the `child` table.
    let validator = Arc::new(DbReadingValidator {
        fk_table: "parent".to_string(),
        fk_field: "ref_id".to_string(),
        unique_field: "ref_id".to_string(),
    }) as Arc<dyn RecordValidator>;

    let reg = Arc::new(ValidatorRegistry::new());
    let validator_id = shamir_types::types::record_id::RecordId::system("db_reading_val");
    reg.register(validator_id, "db_reading_val", validator)
        .unwrap();

    // Persist bindings on the `child` table's info store so the validator
    // binding is loaded on every `get_table("child")` call.
    let child_tmp = repo.get_table("child").await.unwrap();
    let bindings = vec![ValidatorBinding {
        validator_id,
        ops: smallvec::smallvec![WriteOp::Insert],
        priority: 1000,
    }];
    // Access the info_store through the table's public API.
    // The persistence layer writes to the info_store the TableManager was
    // built with — but since get_table returns a fresh instance each time
    // (OnceCell), we need to persist BEFORE the OnceCell is initialized.
    // Use the helper from the persistence module which writes raw metadata.
    crate::validator::persistence::save_validators_metadata(&bindings, child_tmp.info_store())
        .await
        .unwrap();

    let resolver = TestResolver {
        repo,
        validators: reg,
    };
    // Resolve child through the resolver so it picks up the registry.
    let child = resolver.resolve(&TableRef::new("child")).await.unwrap();
    (resolver, child)
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Insert into `child` while a validator reads both `parent` (FK) and
/// `child` (unique) through `ctx.db()`. The write must complete without
/// deadlocking. nextest's per-test timeout (default 60s) is the backstop.
#[tokio::test]
async fn validator_db_no_deadlock_cross_and_self_read() {
    let (resolver, child) = setup().await;

    // Pre-populate the parent table with a referenced row.
    let mut parent_row = new_map();
    parent_row.insert("ref_id".to_string(), QueryValue::Int(42));
    parent_row.insert("name".to_string(), QueryValue::Str("alpha".into()));

    let parent_op = crate::query::write::InsertOp {
        insert_into: TableRef::new("parent"),
        values: vec![QueryValue::Map(parent_row)],
        records_idmsgpack: Vec::new(),
        select: None,
    };

    let (mut parent_tx, _guard) = resolver
        .repo
        .begin_tx(IsolationLevel::Snapshot)
        .await
        .unwrap();
    parent_tx.set_implicit(true);
    let parent = resolver.resolve(&TableRef::new("parent")).await.unwrap();
    parent
        .execute_insert_tx(
            &parent_op,
            &mut parent_tx,
            false,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();
    resolver.repo.commit_tx(parent_tx).await.unwrap();

    // Now insert into child — the DbReadingValidator will fire and read
    // both parent (FK) and child (unique) through ctx.db().
    let mut child_row = new_map();
    child_row.insert("ref_id".to_string(), QueryValue::Int(42));

    let child_op = crate::query::write::InsertOp {
        insert_into: TableRef::new("child"),
        values: vec![QueryValue::Map(child_row)],
        records_idmsgpack: Vec::new(),
        select: None,
    };

    let (mut child_tx, _child_guard) = resolver
        .repo
        .begin_tx(IsolationLevel::Snapshot)
        .await
        .unwrap();
    child_tx.set_implicit(true);

    // This is the critical call: it runs the validator, which reads via
    // ctx.db(). If there's a re-entrancy deadlock, this future never
    // completes and nextest kills it after the timeout.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        child.execute_insert_tx(
            &child_op,
            &mut child_tx,
            false,
            None,
            &shamir_types::access::Actor::System,
        ),
    )
    .await;

    // The write must complete (Ok or Err — but NOT timeout).
    assert!(
        result.is_ok(),
        "execute_insert_tx DEADLOCKED (timed out after 10s) — validator DB read re-entered the write pipeline"
    );
    let inner = result.unwrap();
    assert!(
        inner.is_ok(),
        "execute_insert_tx failed (non-deadlock): {:?}",
        inner.err()
    );

    // Clean up.
    resolver.repo.commit_tx(child_tx).await.unwrap();
}

/// FG-3: cross-table FK read-your-own-writes. Insert BOTH the parent row
/// AND the referencing child row in the SAME transaction (parent not yet
/// committed when the child's FK validator fires). Before the FG-3 fix,
/// `ValidatorDb::exists_in`/`exists_in_table` read only the committed store
/// (`target.list_stream` + the committed index) — a same-tx staged parent
/// insert was invisible to it, and the child insert would wrongly reject
/// with an `fk_violation`. After the fix, `exists_in_table` also probes
/// `tx.write_set` for the resolved TARGET table (here, `parent`), so the
/// staged-but-uncommitted parent row is found and the FK check passes.
#[tokio::test]
async fn validator_db_fk_sees_staged_parent_in_same_tx() {
    let (resolver, child) = setup().await;

    let (mut tx, _guard) = resolver
        .repo
        .begin_tx(IsolationLevel::Snapshot)
        .await
        .unwrap();
    tx.set_implicit(true);

    // Stage the parent row — NOT committed yet.
    let mut parent_row = new_map();
    parent_row.insert("ref_id".to_string(), QueryValue::Int(7));
    parent_row.insert("name".to_string(), QueryValue::Str("beta".into()));
    let parent_op = crate::query::write::InsertOp {
        insert_into: TableRef::new("parent"),
        values: vec![QueryValue::Map(parent_row)],
        records_idmsgpack: Vec::new(),
        select: None,
    };
    let parent = resolver.resolve(&TableRef::new("parent")).await.unwrap();
    parent
        .execute_insert_tx(
            &parent_op,
            &mut tx,
            false,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();

    // Stage the child row referencing the just-staged (uncommitted) parent,
    // in the SAME tx. The `DbReadingValidator` bound to `child` calls
    // `db.exists_in(parent, "ref_id", 7)` — this must see the staged parent
    // row and accept the write, since `unwrap_or(false)` in the validator
    // would otherwise mask a real FK read failure as silent success. To
    // prove the fix precisely (not just "insert succeeded"), assert
    // `exists_in` directly via a fresh `ValidatorDb` built on the SAME tx.
    let vdb = crate::validator::ValidatorDb::new(&tx, &child, Some(&resolver));
    let found = vdb
        .exists_in(&TableRef::new("parent"), "ref_id", &QueryValue::Int(7))
        .await
        .unwrap();
    assert!(
        found,
        "FK cross-table check must see this tx's OWN staged (uncommitted) parent row"
    );

    // A DIFFERENT, concurrent tx must NOT see the staged parent — isolation
    // must hold for the cross-table staged-probe exactly as it does for the
    // self-table one.
    let other_tx = shamir_tx::TxContext::new(
        shamir_tx::TxId::new(999),
        0,
        tx.snapshot_version,
        IsolationLevel::Snapshot,
    );
    let vdb_other = crate::validator::ValidatorDb::new(&other_tx, &child, Some(&resolver));
    let found_other = vdb_other
        .exists_in(&TableRef::new("parent"), "ref_id", &QueryValue::Int(7))
        .await
        .unwrap();
    assert!(
        !found_other,
        "a DIFFERENT, concurrent tx must NOT see tx A's staged-but-uncommitted parent row"
    );

    resolver.repo.commit_tx(tx).await.unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════
// Decode-error fail-closed fix — the twin of F-65/#891's read-error fix,
// applied to `ValidatorDb::record_field_matches_by_id`'s two-step decode
// (`RecordView`, falling back to `InnerValue`). Previously, a row whose
// bytes failed BOTH decode attempts was silently treated as "no match" and
// the scan continued past it — for `exists_in`/`exists_in_table` (FK check)
// this could wrongly report "referenced parent does not exist" (spurious FK
// rejection); for `exists_in_self` (unique check) it could wrongly report
// "no duplicate" and let one through. After the fix, a genuinely corrupt
// row aborts the scan with `Err(DbError::Codec)` instead.
//
// `list_stream`/`list_stream_tx` (unlike `collect_parent_values`'s
// WHERE-filtered scans in `fk_restrict.rs`/`fk_on_update.rs`/`fk_actions.rs`)
// have NO upstream filter-decode gate — every row's bytes reach
// `record_field_matches` directly, so corrupting a row here is directly
// reachable through `ValidatorDb`'s public API without any extra
// fault-injection seam.
// ═══════════════════════════════════════════════════════════════════════════

/// Insert a `{ref_id: <value>}` row directly through `TableManager::insert`
/// (bypassing the batch/validator pipeline — this test targets `ValidatorDb`
/// directly) and return its `RecordId`.
async fn insert_ref_id_row(table: &TableManager, ref_id: i64) -> RecordId {
    let interner = table.interner().get().await.unwrap();
    let ref_id_key = interner.touch_ind("ref_id").unwrap().into_key();
    table.interner().persist().await.unwrap();
    let mut m = new_map();
    m.insert(ref_id_key, InnerValue::Int(ref_id));
    table.insert(&InnerValue::Map(m)).await.unwrap()
}

/// Overwrite `id`'s stored bytes with a lone msgpack bin8 marker (`0xc4`)
/// with no length byte behind it — undecodable by BOTH `RecordView::new`
/// (not a map-header byte) and the `InnerValue::from_bytes` fallback
/// (`UnexpectedEof`). Same fixture as
/// `corrupt_record_byte_path_tests.rs::undecodable_bytes`.
async fn corrupt_row(table: &TableManager, id: RecordId) {
    let key = RecordKey::from_slice(id.as_bytes());
    let corrupt = Bytes::from_static(b"\xc4");
    // Repo-backed tables (`RepoInstance`/`BoxRepoFactory::in_memory()`, as
    // built by `setup()` here) are MVCC-backed — `list_stream`/`list_stream`
    // reads for such a table go through `MvccStore::current_stream`, NOT the
    // raw `data_store`, so corrupting via `data_store().set(..)` alone would
    // be invisible to the scan. Write through the MVCC store when present
    // (mirrors `corrupt_record_byte_path_tests.rs::corrupt_in_place_mvcc`);
    // fall back to the raw store for a non-MVCC table.
    if let Some(mvcc) = table.mvcc_store() {
        mvcc.set_versioned(key, corrupt).await.unwrap();
    } else {
        table.data_store().set(key, corrupt).await.unwrap();
    }
}

#[tokio::test]
async fn exists_in_table_fails_closed_on_decode_corrupt_row() {
    let (resolver, child) = setup().await;
    let parent = resolver.resolve(&TableRef::new("parent")).await.unwrap();

    // A valid row that does NOT match the probed value (99) — the scan must
    // continue past it and reach the corrupt row below.
    insert_ref_id_row(&parent, 1).await;

    // A row that, once corrupted, can never match ANY value — but the scan
    // must fail closed rather than silently treat it as "no match" and keep
    // going.
    let corrupt_id = insert_ref_id_row(&parent, 2).await;
    corrupt_row(&parent, corrupt_id).await;

    let (tx, _guard) = resolver
        .repo
        .begin_tx(IsolationLevel::Snapshot)
        .await
        .unwrap();
    let vdb = crate::validator::ValidatorDb::new(&tx, &child, Some(&resolver));

    let result = vdb
        .exists_in(&TableRef::new("parent"), "ref_id", &QueryValue::Int(99))
        .await;

    let err = result.expect_err(
        "a genuinely decode-corrupt row in the FK target table must abort \
         exists_in/exists_in_table with Err, not silently report no-match \
         and keep scanning",
    );
    assert!(
        matches!(err, DbError::Codec(_)),
        "expected DbError::Codec, got: {err:?}"
    );
}

#[tokio::test]
async fn exists_in_self_fails_closed_on_decode_corrupt_row() {
    let (resolver, _child) = setup().await;
    let table = resolver.resolve(&TableRef::new("child")).await.unwrap();

    // A valid row that does NOT match the probed value (99).
    insert_ref_id_row(&table, 1).await;

    // A row that, once corrupted, must fail the unique scan closed.
    let corrupt_id = insert_ref_id_row(&table, 2).await;
    corrupt_row(&table, corrupt_id).await;

    let (tx, _guard) = resolver
        .repo
        .begin_tx(IsolationLevel::Snapshot)
        .await
        .unwrap();
    let vdb = crate::validator::ValidatorDb::new(&tx, &table, Some(&resolver));

    let result = vdb
        .exists_in_self("ref_id", &QueryValue::Int(99), None)
        .await;

    let err = result.expect_err(
        "a genuinely decode-corrupt row in the self-table unique scan must \
         abort exists_in_self with Err, not silently report no-duplicate \
         and keep scanning",
    );
    assert!(
        matches!(err, DbError::Codec(_)),
        "expected DbError::Codec, got: {err:?}"
    );
}
// ── Audit group 25 / defect 1: unindexed FK full-scan fallback ────────────
//
// `exists_in_table` falls back to a full committed-table scan when the
// referenced field has no single-field index. Called once PER CHILD ROW
// from `SchemaValidator::validate`'s per-record FK check, an M-row batch
// write against an unindexed parent field pays O(M x parent_rows). The
// fix landed here is a `log::warn!` naming the hazard (see
// `ValidatorDb::exists_in_table`'s doc for why a true semi-join or a
// DDL-time refusal were rejected as out-of-scope/too risky for this fix).
// These tests prove: (1) the scan's accept/reject decision stays correct
// for MULTIPLE distinct child values, not just fewer scans; (2) the
// warning genuinely fires on that path.

/// Simple in-memory log sink for test assertions — mirrors
/// `query::batch::tests::watchdog_tests::TestLogSink`.
#[derive(Clone)]
struct TestLogSink {
    logs: Arc<Mutex<Vec<String>>>,
}

impl TestLogSink {
    fn new() -> Self {
        Self {
            logs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn contains(&self, pattern: &str) -> bool {
        self.logs
            .lock()
            .unwrap()
            .iter()
            .any(|log| log.contains(pattern))
    }
}

impl log::Log for TestLogSink {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        let msg = format!("[{}] {}", record.level(), record.args());
        self.logs.lock().unwrap().push(msg);
    }

    fn flush(&self) {}
}

/// Insert `rows` (each `(ref_id, name)`) into `parent` as one committed tx.
async fn insert_parent_rows(resolver: &TestResolver, rows: &[(i64, &str)]) {
    let parent = resolver.resolve(&TableRef::new("parent")).await.unwrap();
    let (mut tx, _guard) = resolver
        .repo
        .begin_tx(IsolationLevel::Snapshot)
        .await
        .unwrap();
    tx.set_implicit(true);
    for (ref_id, name) in rows {
        let mut row = new_map();
        row.insert("ref_id".to_string(), QueryValue::Int(*ref_id));
        row.insert("name".to_string(), QueryValue::Str((*name).to_string()));
        let op = crate::query::write::InsertOp {
            insert_into: TableRef::new("parent"),
            values: vec![QueryValue::Map(row)],
            records_idmsgpack: Vec::new(),
            select: None,
        };
        parent
            .execute_insert_tx(
                &op,
                &mut tx,
                false,
                None,
                &shamir_types::access::Actor::System,
            )
            .await
            .unwrap();
    }
    resolver.repo.commit_tx(tx).await.unwrap();
}

/// `parent` has no index on `ref_id` (matches `setup()`'s plain
/// `TableConfig::new`), so every probe below exercises the full-scan
/// fallback path. Several child rows each probe a DIFFERENT FK value
/// against the SAME unindexed field — some present, some absent — proving
/// the scan's accept/reject decision is correct for every one of them, not
/// just that fewer scans happen.
#[tokio::test]
async fn fk_multi_child_rows_unindexed_parent_correct_decisions() {
    let (resolver, child) = setup().await;
    insert_parent_rows(&resolver, &[(10, "a"), (20, "b"), (30, "c")]).await;

    let (probe_tx, _guard) = resolver
        .repo
        .begin_tx(IsolationLevel::Snapshot)
        .await
        .unwrap();
    let vdb = crate::validator::ValidatorDb::new(&probe_tx, &child, Some(&resolver));

    // Simulates M "child rows" in one batch, each probing a different FK
    // value against the SAME unindexed parent field.
    for (ref_id, expect_found) in [
        (10i64, true),
        (20, true),
        (30, true),
        (40, false),
        (50, false),
    ] {
        let found = vdb
            .exists_in(&TableRef::new("parent"), "ref_id", &QueryValue::Int(ref_id))
            .await
            .unwrap();
        assert_eq!(
            found, expect_found,
            "ref_id={ref_id}: expected found={expect_found}, got {found}"
        );
    }
}

/// The full-scan fallback (no single-field index on the FK's referenced
/// field) must log a warning naming the hazard, so it is actionable instead
/// of a silent perf cliff.
#[tokio::test]
async fn fk_unindexed_field_scan_logs_warning() {
    let log_sink = TestLogSink::new();
    // `log::set_boxed_logger` is process-global and can only succeed once;
    // nextest runs each test in its own process, so this is safe here (same
    // pattern as `query::batch::tests::watchdog_tests`).
    let _ = log::set_boxed_logger(Box::new(log_sink.clone()));
    log::set_max_level(log::LevelFilter::Warn);

    let (resolver, child) = setup().await;
    insert_parent_rows(&resolver, &[(1, "only")]).await;

    let (probe_tx, _guard) = resolver
        .repo
        .begin_tx(IsolationLevel::Snapshot)
        .await
        .unwrap();
    let vdb = crate::validator::ValidatorDb::new(&probe_tx, &child, Some(&resolver));

    let found = vdb
        .exists_in(&TableRef::new("parent"), "ref_id", &QueryValue::Int(1))
        .await
        .unwrap();
    assert!(found, "sanity: the referenced value does exist");

    assert!(
        log_sink.contains("no single-field index"),
        "expected a warning naming the unindexed FK field; got: {:?}",
        log_sink.logs.lock().unwrap()
    );
}
