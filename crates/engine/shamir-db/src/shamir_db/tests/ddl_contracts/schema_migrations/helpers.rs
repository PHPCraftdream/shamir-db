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
