use std::time::Duration;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableManager;
use crate::query::batch::BatchError;
use crate::shamir_db::system_store::{
    DatabaseIdentity, FormatEpoch, SchemaMigrationContext, SystemStoreConfig,
};
use crate::types::value::{InnerValue, QueryValue};
use crate::ShamirDb;
use futures::StreamExt;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::{
    application_schema_plan, application_schema_status, apply_application_schema_migration,
    SchemaMigration,
};
use shamir_query_builder::{filter, write};
use shamir_query_types::admin::types::schema_migration::{
    canonical_body_checksum, SchemaMigrationApplyRequest, SchemaMigrationBody,
    SchemaMigrationPlanRequest, SchemaMigrationPredecessor, SchemaMigrationStatusRequest,
    SchemaMigrationSubmission, SchemaMigrationTarget,
};
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};
use shamir_types::codecs::interned::query_value_to_inner;
use shamir_types::types::record_id::RecordId;

/// The database owner used by the persisted journal regressions: holds
/// `Read` and `Manage` on the database and its store resource (the §5.2
/// preflight). Database-level admin DDL (create/rename/drop) needs the root
/// principal and is run through `execute` (`Actor::System`) instead.
pub(super) const OWNER: Actor = Actor::User(7);

/// The journal needs a persistent system store; the directory must outlive the db.
pub(super) async fn persistent_db() -> (tempfile::TempDir, crate::ShamirDb) {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::ShamirDb::init(crate::shamir_db::system_store::SystemStoreConfig::Fjall(
        dir.path().join("system"),
    ))
    .await
    .unwrap();
    (dir, db)
}

pub(super) fn context() -> SchemaMigrationContext {
    SchemaMigrationContext::new(crate::access::Actor::System, FormatEpoch::Known(7))
}

pub(super) fn body(id: &str, ordinal: u64) -> SchemaMigrationBody {
    SchemaMigration::new(id, ordinal, format!("migration {ordinal}"))
        .op(ddl::create_table(format!("items_{ordinal}")).build())
        .expect("typed create-table operation is admitted")
        .build()
        .expect("test migration body is valid")
}

pub(super) fn checksum(body: &SchemaMigrationBody) -> [u8; 32] {
    canonical_body_checksum(body).expect("test migration body canonicalizes")
}

pub(super) async fn rows(table: &TableManager) -> Vec<(RecordId, QueryValue)> {
    let interner = table
        .info_store_read_view()
        .interner_read_view()
        .await
        .expect("journal interner is readable");
    let mut stream = Box::pin(table.list_stream(128));
    let mut records = Vec::new();
    while let Some(batch) = stream.next().await {
        for (id, record) in batch.expect("journal scan succeeds") {
            let value = match record {
                crate::engine::table::record_cow::RecordCow::Borrowed(bytes) => interner
                    .record_bytes_to_query_value(&bytes)
                    .expect("journal record materialization succeeds")
                    .expect("journal row is a supported value"),
                crate::engine::table::record_cow::RecordCow::Owned(value) => interner
                    .inner_to_query_value(&value)
                    .expect("journal record materializes"),
            };
            records.push((id, value));
        }
    }
    records
}

pub(super) async fn overwrite_row(table: &TableManager, id: RecordId, value: QueryValue) {
    let interner = table
        .interner_for_test()
        .get()
        .await
        .expect("test table interner is loaded");
    let inner: InnerValue =
        query_value_to_inner(&value, interner).expect("raw fixture value interns");
    table
        .set(id, &inner)
        .await
        .expect("raw fixture write succeeds");
}

/// Run a test body on a roomy thread with its own runtime: the apply runner
/// nests the admin handlers' async frames (function compilation among them)
/// deeper than a debug test thread's default stack allows. Same idea as the
/// server's lifecycle fixtures.
pub(super) fn on_big_stack<F, Fut>(body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(body())
        })
        .expect("spawn test thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

// ---------------------------------------------------------------------------
// WAL-BACKED DAMAGE SEAMS — used ONLY by the RESTART fixtures.
//
// HAZARD: the plain raw seams above (`overwrite_row` = `TableManager::set`,
// and a direct `TableManager::delete`) write NO durable transaction WAL
// entry, so after a cold restart the recovery floor (seeded from the last
// committed WAL marker / clean checkpoint) cannot see them — and an OLD WAL
// entry can rematerialize the clean row over the damaged bytes. Every seam
// below therefore routes the damage through the system repo's implicit
// batch transaction (`run_implicit_batch_tx`), whose commit folds the
// mutation into ONE durable `WalEntryV2` exactly like
// `journal.rs::commit_transition`, so it replays on reopen. Operations are
// built through `shamir-query-builder` and filter on the UNIQUE
// `record_key` (exactly one physical record); each asserts `affected == 1`.
// ---------------------------------------------------------------------------

const JOURNAL_TABLE: &str = "schema_migration_events";

fn journal_key_filter(record_key: &str) -> shamir_query_types::filter::Filter {
    filter::eq("record_key", record_key)
}

/// A journal row by record kind (`"control"` / `"event"`) and database
/// identity: (record id, unique `record_key`, full envelope). The ordinary
/// reader (`rows`) is the same view the journal's own partition reader uses.
pub(super) async fn find_journal_row(
    db: &ShamirDb,
    identity: &DatabaseIdentity,
    record_type: &str,
) -> (RecordId, String, QueryValue) {
    let table = journal_table(db).await;
    rows(&table)
        .await
        .into_iter()
        .find_map(|(id, row)| {
            let routed = row.get("record_type").and_then(QueryValue::as_str) == Some(record_type)
                && row.get("database_identity").and_then(QueryValue::as_str)
                    == Some(identity.to_string().as_str());
            routed.then(|| {
                let key = row
                    .get("record_key")
                    .and_then(QueryValue::as_str)
                    .unwrap_or_else(|| panic!("journal {record_type} row carries its record_key"))
                    .to_string();
                (id, key, row)
            })
        })
        .unwrap_or_else(|| panic!("a persisted {record_type} row of {identity} exists"))
}

/// WAL-backed UPDATE of one journal row, addressed by its UNIQUE
/// `record_key`. `patch` is the partial set-map of `execute_update_tx`
/// (merge-patch onto the stored record). Asserts exactly one row changed.
pub(super) async fn wal_patch_journal_row(db: &ShamirDb, record_key: &str, patch: QueryValue) {
    let table = journal_table(db).await;
    let op = write::update(JOURNAL_TABLE)
        .where_(journal_key_filter(record_key))
        .set(patch)
        .build()
        .expect("typed journal row update operation");
    let result = db
        .system_store()
        .update_via_implicit_tx(&table, &op)
        .await
        .expect("WAL-backed journal row update commits through the implicit tx");
    assert_eq!(
        result.affected, 1,
        "the unique-key update must hit exactly one row (key {record_key})"
    );
}

/// WAL-backed physical DELETE of one journal row, addressed by its UNIQUE
/// `record_key`. The store's `delete_via_implicit_tx` is not visible to the
/// tests, so the same production path is spelled out here (mirroring
/// `SystemStore::delete_via_implicit_tx` / the `query_runner` Delete arm).
pub(super) async fn wal_delete_journal_row(db: &ShamirDb, record_key: &str) {
    let table = journal_table(db).await;
    let op = write::delete(JOURNAL_TABLE)
        .where_(journal_key_filter(record_key))
        .build()
        .expect("typed journal row delete operation");
    let repo = db.system_store().system_repo().expect("system repo");
    let owned = (table.clone(), op);
    let result = repo
        .run_implicit_batch_tx(Actor::System, "test_journal_row_delete", move |tx| {
            Box::pin(async move {
                let (table, op) = owned;
                let refs = shamir_types::types::common::new_map();
                let ctx = table.info_store_read_view().filter_context(&refs).await?;
                table
                    .execute_delete_tx(&op, &ctx, tx, None, &Actor::System)
                    .await
            })
        })
        .await
        .expect("WAL-backed journal row delete commits through the implicit tx");
    assert_eq!(
        result.affected, 1,
        "the unique-key delete must hit exactly one row (key {record_key})"
    );
}

/// WAL-backed DELETE + re-INSERT of one journal row inside ONE implicit
/// transaction, with the new envelope carried as-is (`row_without_key`).
/// Needed where merge-patch semantics cannot express the damage — a
/// MISSING `record_key` field cannot be produced by an update (a partial
/// set-map only adds/replaces fields), so the row is atomically replaced.
/// `commit_tx` emits ONE durable WAL entry for the whole pair, so the
/// key-less replacement replays on reopen exactly as committed.
pub(super) async fn wal_replace_journal_row_without_key(
    db: &ShamirDb,
    record_key: &str,
    row_without_key: QueryValue,
) {
    let table = journal_table(db).await;
    let delete = write::delete(JOURNAL_TABLE)
        .where_(journal_key_filter(record_key))
        .build()
        .expect("typed journal row delete operation");
    let insert = write::insert(JOURNAL_TABLE).row(row_without_key).build();
    let repo = db.system_store().system_repo().expect("system repo");
    let owned = (table.clone(), delete, insert);
    let result = repo
        .run_implicit_batch_tx(Actor::System, "test_journal_row_replace", move |tx| {
            Box::pin(async move {
                let (table, delete, insert) = owned;
                let refs = shamir_types::types::common::new_map();
                let ctx = table.info_store_read_view().filter_context(&refs).await?;
                let deleted = table
                    .execute_delete_tx(&delete, &ctx, tx, None, &Actor::System)
                    .await?;
                assert_eq!(
                    deleted.affected, 1,
                    "the unique-key delete leg must hit exactly one row"
                );
                let inserted = table
                    .execute_insert_tx(&insert, tx, false, None, &Actor::System)
                    .await?;
                assert_eq!(
                    inserted.affected, 1,
                    "the replacement insert leg must affect exactly one row"
                );
                Ok(crate::query::write::WriteResult {
                    affected: 1,
                    records: Vec::new(),
                    execution_time_us: 0,
                })
            })
        })
        .await
        .expect("WAL-backed journal row replace commits through the implicit tx");
    assert_eq!(result.affected, 1, "the replace transaction commits once");
}

// ---------------------------------------------------------------------------
// Shared helpers of the persisted journal regressions (isolation, control
// integrity, lifecycle refusals). Tight `pub(super)` visibility: reachable
// only inside `schema_migrations`.
// ---------------------------------------------------------------------------

/// Prepare one database for migration traffic: repository `main` plus owner
/// grants on the database and the store resource.
pub(super) async fn prepare_database(db: &ShamirDb, name: &str) {
    db.add_repo(name, RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    let meta = ResourceMeta {
        owner: OWNER,
        group: None,
        mode: 0o700,
    };
    db.set_resource_meta(&ResourcePath::database(name), &meta)
        .await
        .unwrap();
    db.set_resource_meta(&ResourcePath::store(name, "main"), &meta)
        .await
        .unwrap();
}

/// The raw `schema_migration_events` system table, for the persisted-damage
/// seams (`rows` / `overwrite_row` / `TableManager::delete`).
pub(super) async fn journal_table(db: &ShamirDb) -> TableManager {
    db.system_store()
        .system_repo()
        .unwrap()
        .get_table("schema_migration_events")
        .await
        .unwrap()
}

/// Flush the journal table durably, drop the engine and REOPEN the store.
pub(super) async fn flush_and_reopen(
    dir: &std::path::Path,
    db: ShamirDb,
    table: TableManager,
) -> ShamirDb {
    db.system_store()
        .flush_system_repo_durable(&table)
        .await
        .expect("the journal rows flush durably before the restart");
    drop(table);
    drop(db);
    init_waiting_for_locks(dir.join("system")).await
}

/// Reopen the persistent system store after a cold restart: a just-dropped
/// engine can hold its fjall lock until the background handles finish
/// (`FjallError: Locked`, seen on Windows), so retry ONLY that error under a
/// bounded deadline; every other error fails at once. Never a bare reopen.
/// Result-returning variant: a startup that is EXPECTED to fail closed
/// (unattributable journal row) observes the typed error through
/// [`try_init_waiting_for_locks`].
pub(super) async fn init_waiting_for_locks(system_path: std::path::PathBuf) -> ShamirDb {
    try_init_waiting_for_locks(system_path)
        .await
        .unwrap_or_else(|error| panic!("reopen the system store: {error}"))
}

/// Like [`init_waiting_for_locks`], but returns the open error instead of
/// panicking: the bounded Locked-only retry still applies, and every other
/// error is surfaced to the caller untouched (typed journal failures keep
/// their code, no message matching).
pub(super) async fn try_init_waiting_for_locks(
    system_path: std::path::PathBuf,
) -> Result<ShamirDb, crate::DbError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        match tokio::time::timeout(
            Duration::from_secs(120),
            ShamirDb::init(SystemStoreConfig::Fjall(system_path.clone())),
        )
        .await
        .expect("reopen attempt timed out")
        {
            Ok(db) => return Ok(db),
            Err(error)
                if error.to_string().contains("Locked")
                    && tokio::time::Instant::now() < deadline =>
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(error) => return Err(error),
        }
    }
}

pub(super) fn target(identity: &DatabaseIdentity) -> SchemaMigrationTarget {
    SchemaMigrationTarget {
        database_identity: identity.0,
    }
}

pub(super) fn predecessor(
    id: Option<&str>,
    ordinal: u64,
    digest: [u8; 32],
) -> SchemaMigrationPredecessor {
    SchemaMigrationPredecessor {
        migration_id: id.map(str::to_string),
        ordinal,
        checksum: digest,
    }
}

pub(super) fn submission(migration: &SchemaMigrationBody) -> SchemaMigrationSubmission {
    SchemaMigrationSubmission::new(migration, checksum(migration))
}

/// The apply generation a fresh apply must expect, taken from STATUS —
/// never a magic constant in the tests.
pub(super) async fn fence_generation_from_status(
    db: &ShamirDb,
    database: &str,
    identity: &DatabaseIdentity,
) -> u64 {
    let status = run_status(db, database, identity)
        .await
        .unwrap_or_else(|error| panic!("{database} status for the fence generation: {error}"));
    let generation = status
        .get("fence_generation")
        .and_then(QueryValue::as_i64)
        .unwrap_or_else(|| panic!("status reports its fence_generation"));
    u64::try_from(generation)
        .unwrap_or_else(|_| panic!("{database} fence_generation is negative: {generation}"))
}

pub(super) async fn run_status(
    db: &ShamirDb,
    database: &str,
    identity: &DatabaseIdentity,
) -> Result<QueryValue, BatchError> {
    let mut b = Batch::new();
    b.op(
        "status",
        application_schema_status(SchemaMigrationStatusRequest {
            protocol_version: 1,
            target: target(identity),
        }),
    );
    let request = b.to_request_via_msgpack().unwrap();
    let response = db.execute(database, &request).await?;
    Ok(response.results["status"].records[0]
        .as_value()
        .into_owned())
}

pub(super) async fn run_plan(
    db: &ShamirDb,
    database: &str,
    expected: SchemaMigrationPredecessor,
    migrations: Vec<SchemaMigrationSubmission>,
    identity: &DatabaseIdentity,
) -> Result<QueryValue, BatchError> {
    let mut b = Batch::new();
    b.op(
        "plan",
        application_schema_plan(SchemaMigrationPlanRequest {
            protocol_version: 1,
            target: target(identity),
            expected_predecessor: expected,
            migrations,
        }),
    );
    let request = b.to_request_via_msgpack().unwrap();
    let response = db.execute(database, &request).await?;
    Ok(response.results["plan"].records[0].as_value().into_owned())
}

pub(super) async fn run_apply(
    db: &ShamirDb,
    database: &str,
    expected: SchemaMigrationPredecessor,
    fence_generation: u64,
    migration: SchemaMigrationSubmission,
    identity: &DatabaseIdentity,
) -> Result<QueryValue, BatchError> {
    let mut b = Batch::new();
    b.op(
        "apply",
        apply_application_schema_migration(SchemaMigrationApplyRequest {
            protocol_version: 1,
            target: target(identity),
            expected_predecessor: expected,
            expected_fence_generation: fence_generation,
            migration,
        }),
    );
    let request = b.to_request_via_msgpack().unwrap();
    let response = db.execute_as(OWNER, database, &request).await?;
    Ok(response.results["apply"].records[0].as_value().into_owned())
}

/// Run a single database-level admin op (create/rename/drop db) through the
/// root principal (`db.execute` = `Actor::System`): these ops authorize at
/// `ResourcePath::Root`, which `User(7)` without a root grant cannot.
pub(super) async fn run_admin_op(
    db: &ShamirDb,
    session: &str,
    alias: &str,
    op: shamir_query_types::batch::BatchOp,
) -> Result<QueryValue, BatchError> {
    let mut b = Batch::new();
    b.op(alias, op);
    let request = b.to_request_via_msgpack().unwrap();
    let response = db.execute(session, &request).await?;
    Ok(response.results[alias].records[0].as_value().into_owned())
}

pub(super) fn field<'a>(value: &'a QueryValue, key: &str) -> &'a QueryValue {
    value
        .get(key)
        .unwrap_or_else(|| panic!("field {key} missing"))
}

pub(super) fn str_field<'a>(value: &'a QueryValue, key: &str) -> &'a str {
    field(value, key)
        .as_str()
        .unwrap_or_else(|| panic!("field {key} is not a string"))
}
