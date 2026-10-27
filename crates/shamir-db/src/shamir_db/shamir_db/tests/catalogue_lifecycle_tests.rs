use crate::access::{Action, Actor, ResourceMeta, ResourcePath};
use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::{TableConfig, TableManager};
use crate::shamir_db::shamir_db::{ArtifactKind, KIND_FIELD};
use crate::shamir_db::{FunctionSource, ShamirDb, SystemStoreConfig};
use base64::Engine;
use shamir_engine::function::CreateFunctionOptions;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

/// Not a latency assertion: these tests need the clean shutdown to finish.
/// Windows CI runners stall every process's fsync for 10+ s at times, so any
/// wall-clock budget here is a false failure; nextest's per-test timeout is
/// the hang detector and names the test.
const BOOT_TEST_FLUSH_BUDGET: std::time::Duration = std::time::Duration::from_secs(3600);

/// Re-open the system store expecting a boot rejection, retrying while the
/// dropped instance still holds the fjall file lock (released lazily after
/// the drop). Returns the rejection's message.
async fn boot_rejection(path: std::path::PathBuf) -> String {
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(path.clone())).await {
            Ok(_) => panic!("boot must be rejected"),
            Err(e) => {
                let message = e.to_string();
                if !message.contains("Locked") {
                    return message;
                }
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
        }
    }
    panic!("fjall lock was not released within the retry window");
}

const TEST_WASM: &str = r#"
(module
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 1024))
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr))
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or (i64.shl (i64.extend_i32_u (local.get $ptr)) (i64.const 32))
            (i64.extend_i32_u (local.get $len))))
)
"#;
const TEST_WASM_REPLACEMENT: &str = r#"
(module
  (memory (export "memory") 2)
  (func (export "shamir_alloc") (param i32) (result i32) (i32.const 1024))
  (func (export "shamir_call") (param i32 i32) (result i64) (i64.const 0))
)
"#;

/// Flush-fault double: delegates every operation to an inner in-memory store
/// except `flush()`, which fails while armed. This isolates the DDL
/// durability tail (the card's fix site) from the committing write path.
///
/// EXACT REVERT COUNTERFACTUALS — the production barrier is
/// `SystemStore::flush_system_repo_durable`, which runs `repo.sync_wal()`
/// FIRST and `table.data_store_read_view().flush()` LAST:
/// - Reverting ONLY the helper's final `flush()` line (leaving sync_wal)
///   makes every rv_db_0486e3f9 FlushFaultStore test below fail: sync_wal
///   alone succeeds on the in-memory system repo, so the armed flush fault
///   is never reached and the DDL ack returns Ok.
/// - Reverting ONLY the helper's `sync_wal()` line (leaving flush) makes
///   `rv_db_0486e3f9_sync_wal_fault_propagates` fail: the armed sync_wal
///   fault fires on the removed call, so create_db would return Ok.
mod flush_fault_store {
    use async_trait::async_trait;
    use bytes::Bytes;
    use shamir_storage::error::{DbError, DbResult};
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::{KvOp, RecordKey, Store};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};

    use futures::stream::Stream;

    pub struct FlushFaultStore {
        inner: InMemoryStore,
        pub armed: AtomicBool,
    }

    impl FlushFaultStore {
        pub fn new() -> Self {
            Self {
                inner: InMemoryStore::new(),
                armed: AtomicBool::new(false),
            }
        }
    }

    #[async_trait]
    impl Store for FlushFaultStore {
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
        async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
            self.inner.transact(ops).await
        }
        fn iter_stream(
            &self,
            batch_size: usize,
        ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
            self.inner.iter_stream(batch_size)
        }
        fn scan_prefix_stream(
            &self,
            prefix: Bytes,
            batch_size: usize,
        ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
            self.inner.scan_prefix_stream(prefix, batch_size)
        }
        async fn flush(&self) -> DbResult<()> {
            if self.armed.load(Ordering::SeqCst) {
                Err(DbError::Storage("injected flush fault".into()))
            } else {
                Ok(())
            }
        }
    }
}

use flush_fault_store::FlushFaultStore;

/// Install a `FlushFaultStore`-backed system table on the SYSTEM repo,
/// returning the `ShamirDb` plus the armed/disarm handle. Mirrors the
/// `install_table_for_test` splice pattern used by the ACL corruption tests.
async fn shamir_with_flush_fault_table(
    table_name: &str,
) -> (ShamirDb, std::sync::Arc<FlushFaultStore>) {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let data_store = std::sync::Arc::new(FlushFaultStore::new());
    let info_store: std::sync::Arc<dyn shamir_storage::types::Store> =
        std::sync::Arc::new(shamir_storage::storage_in_memory::InMemoryStore::new());
    let system_repo = shamir.system_store().system_repo().unwrap();
    let table = TableManager::create_with_domain(
        table_name.to_string(),
        data_store.clone() as std::sync::Arc<dyn shamir_storage::types::Store>,
        info_store,
        std::sync::Arc::clone(system_repo.write_domain()),
    )
    .await
    .unwrap();
    system_repo.install_table_for_test(table_name, table);
    (shamir, data_store)
}

#[tokio::test]
async fn rv_db_0486e3f9_sync_wal_fault_propagates() {
    let (shamir, _fault) = shamir_with_flush_fault_table("databases").await;
    shamir_engine::repo::RepoInstance::arm_sync_wal_fault(1);
    let result = shamir.create_db("syncwalfail").await;
    assert!(
        result.is_err(),
        "save_database must surface the sync_wal failure"
    );
}

#[tokio::test]
async fn rv_db_0486e3f9_create_database_flush_failure_propagates() {
    let (shamir, fault) = shamir_with_flush_fault_table("databases").await;
    fault.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let result = shamir.create_db("flushfail").await;
    assert!(
        result.is_err(),
        "save_database must surface the flush failure"
    );
}

#[tokio::test]
async fn rv_db_0486e3f9_remove_database_flush_failure_propagates() {
    let (shamir, fault) = shamir_with_flush_fault_table("databases").await;
    shamir
        .create_db("flushfail")
        .await
        .expect("disarmed create must succeed");
    fault.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let result = shamir.remove_db("flushfail").await;
    assert!(
        result.is_err(),
        "remove_database must surface the flush failure"
    );
}

#[tokio::test]
async fn rv_db_0486e3f9_replication_profile_create_flush_failure_propagates() {
    let (shamir, fault) = shamir_with_flush_fault_table("replication_profiles").await;
    shamir.create_db("testdb").await.expect("create_db");
    fault.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut b = Batch::new();
    b.id(1);
    b.op(
        "profile",
        ddl::replication_profile("profile_flush").stream(
            ddl::repl_scope("testdb").repo("main").build(),
            ReplDirection::Pull,
            ReplMode::ReadOnly,
        ),
    );
    let result = shamir
        .execute(
            "testdb",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await;
    assert!(
        result.is_err(),
        "create-replication-profile must surface the flush failure"
    );
}

#[tokio::test]
async fn rv_db_bba04407_namespace_create_cannot_replace_private_function() {
    let db = ShamirDb::init_memory().await.unwrap();
    let wasm = wat::parse_str(TEST_WASM).unwrap();
    let owner = Actor::User(41);
    db.create_function_from_wasm_as("private_owned", &wasm, false, owner.clone())
        .await
        .unwrap();
    db.set_resource_meta(
        &ResourcePath::function("private_owned"),
        &ResourceMeta {
            owner: owner.clone(),
            group: Some(7),
            mode: 0o740,
        },
    )
    .await
    .unwrap();
    let before = db
        .system_store()
        .load_function("private_owned")
        .await
        .unwrap()
        .unwrap();
    assert!(db
        .authorize_access(
            &Actor::User(99),
            &ResourcePath::FunctionNamespace,
            Action::Create,
        )
        .await
        .is_ok());
    assert!(db
        .authorize_access(
            &Actor::User(99),
            &ResourcePath::function("private_owned"),
            Action::Write,
        )
        .await
        .is_err());
    assert!(db
        .create_function_with_opts_as(
            "private_owned",
            FunctionSource::Wasm(&wasm),
            CreateFunctionOptions {
                replace: true,
                ..CreateFunctionOptions::default()
            },
            Actor::User(99),
        )
        .await
        .is_err());
    let after = db
        .system_store()
        .load_function("private_owned")
        .await
        .unwrap()
        .unwrap();
    let expected = ResourceMeta {
        owner: owner.clone(),
        group: Some(7),
        mode: 0o740,
    };
    assert_eq!(ResourceMeta::from_record(&after), expected);
    assert_eq!(before["wasm_b64"], after["wasm_b64"]);
    let replacement_wasm = wat::parse_str(TEST_WASM_REPLACEMENT).unwrap();
    db.create_function_from_wasm_as("private_owned", &replacement_wasm, true, owner.clone())
        .await
        .unwrap();
    let replaced = db
        .system_store()
        .load_function("private_owned")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        ResourceMeta::from_record(&replaced),
        ResourceMeta {
            owner,
            group: Some(7),
            mode: 0o740,
        }
    );
    assert_ne!(before["wasm_b64"], replaced["wasm_b64"]);
}

#[tokio::test]
async fn rv_db_bba04407_live_builtin_cannot_be_durably_replaced() {
    let db = ShamirDb::init_memory().await.unwrap();
    let wasm = wat::parse_str(TEST_WASM).unwrap();
    let name = "argon2id";
    assert!(db.functions().contains(name));
    assert!(db
        .create_function_with_opts_as(
            name,
            FunctionSource::Wasm(&wasm),
            CreateFunctionOptions {
                replace: true,
                ..CreateFunctionOptions::default()
            },
            Actor::System,
        )
        .await
        .is_err());
    assert!(db
        .system_store()
        .load_function(name)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn rv_db_a7a29b02_boot_rejects_missing_parent_repository() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("system.redb");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(path.clone()))
        .await
        .unwrap();
    db.system_store()
        .save_repository(
            "missing_parent",
            "orphan_repo",
            "in_memory",
            None,
            &ResourceMeta::open(),
            None,
        )
        .await
        .unwrap();
    db.system_store()
        .flush_buffers_with_quiesce_timeout(BOOT_TEST_FLUSH_BUDGET)
        .await
        .unwrap();
    drop(db);
    let error = boot_rejection(path).await;
    assert!(
        error.contains("missing_parent/orphan_repo"),
        "unexpected boot error: {error}"
    );
}

#[tokio::test]
async fn rv_db_d378c582_boot_rejects_builtin_name_collision() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("system.redb");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(path.clone()))
        .await
        .unwrap();
    let wasm = wat::parse_str(TEST_WASM).unwrap();
    let mut row = new_map();
    row.insert("name".into(), QueryValue::Str("argon2id".into()));
    row.insert(
        "wasm_b64".into(),
        QueryValue::Str(base64::engine::general_purpose::STANDARD.encode(wasm)),
    );
    row.insert(KIND_FIELD.into(), ArtifactKind::Wasm.as_query_value());
    db.system_store()
        .save_function(
            "argon2id",
            &QueryValue::Map(row),
            &ResourceMeta::owned_enforced(Actor::User(41)),
        )
        .await
        .unwrap();
    db.system_store()
        .flush_buffers_with_quiesce_timeout(BOOT_TEST_FLUSH_BUDGET)
        .await
        .unwrap();
    drop(db);
    let error = boot_rejection(path).await;
    assert!(
        error.contains("collides with a builtin"),
        "unexpected boot error: {error}"
    );
}

#[tokio::test]
async fn rv_db_b26124d9_rename_preserves_schema_catalogue_fields() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("system.redb");
    let before_schema;
    let before_validator_id;
    let before_schema_version;
    {
        let db = ShamirDb::init(SystemStoreConfig::Fjall(path.clone()))
            .await
            .unwrap();
        db.create_db("before").await.unwrap();
        let repo_config = RepoConfig::new("old", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("items"));
        db.add_repo("before", repo_config).await.unwrap();

        // Real schema DDL through the query builder — this stamps
        // schema / schema_validator_id / schema_version in the table catalogue.
        let rules: Vec<_> = vec![ddl::field(["email"]).string().required().build()];
        let mut b = Batch::new();
        b.id(1);
        b.set_table_schema(
            "ss",
            ddl::set_table_schema("items").repo("old").rules(rules),
        );
        db.execute(
            "before",
            &b.to_request_via_msgpack().expect("batch request encoding"),
        )
        .await
        .unwrap();

        let before = db
            .system_store()
            .load_table_record("before", "old", "items")
            .await
            .unwrap()
            .expect("table row must exist pre-rename");
        before_schema = before.get("schema").cloned();
        before_validator_id = before.get("schema_validator_id").cloned();
        before_schema_version = before.get("schema_version").cloned();

        db.rename_repo_as("before", "old", "new", Actor::System)
            .await
            .unwrap();

        db.flush_all_for_test().await.unwrap();
    }
    let db = ShamirDb::init(SystemStoreConfig::Fjall(path))
        .await
        .unwrap();
    let saved = db
        .system_store()
        .load_table_record("before", "new", "items")
        .await
        .unwrap()
        .expect("renamed table row must survive reopen");
    // Exact equality with the pre-rename row's three schema fields.
    assert_eq!(saved.get("schema"), before_schema.as_ref());
    assert_eq!(
        saved.get("schema_validator_id"),
        before_validator_id.as_ref()
    );
    assert_eq!(saved.get("schema_version"), before_schema_version.as_ref());
}

#[tokio::test]
async fn rv_db_55b0a5d9_catalogue_only_function_counts_as_existing() {
    let db = ShamirDb::init_memory().await.unwrap();
    let mut orphan = new_map();
    orphan.insert("name".to_string(), QueryValue::Str("orphan".to_string()));
    orphan.insert("wasm_b64".to_string(), QueryValue::Str("bad".to_string()));
    db.system_store()
        .save_function("orphan", &QueryValue::Map(orphan), &ResourceMeta::open())
        .await
        .unwrap();
    assert!(db.drop_function_as("orphan", Actor::System).await.unwrap());
    assert!(db
        .system_store()
        .load_function("orphan")
        .await
        .unwrap()
        .is_none());
}

/// Admin-route IF EXISTS over a durable catalogue-only function: the first
/// drop reports `existed: true` and removes the row; a second IF EXISTS drop
/// is a silent `existed: false` no-op (it must NOT skip catalogue-only rows).
#[tokio::test]
async fn rv_db_55b0a5d9_admin_if_exists_second_drop_reports_false() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await.unwrap();
    let mut orphan = new_map();
    orphan.insert("name".to_string(), QueryValue::Str("orphan".to_string()));
    orphan.insert("wasm_b64".to_string(), QueryValue::Str("bad".to_string()));
    db.system_store()
        .save_function("orphan", &QueryValue::Map(orphan), &ResourceMeta::open())
        .await
        .unwrap();
    let mut first = Batch::new();
    first.id(1);
    first.op(
        "drop",
        ddl::drop_function("orphan")
            .hmac("test-hmac")
            .if_exists()
            .build()
            .expect("build DropFunction"),
    );
    let resp = db
        .execute(
            "testdb",
            &first
                .to_request_via_msgpack()
                .expect("batch request encoding"),
        )
        .await
        .unwrap();
    let dropped = resp.results["drop"].records[0].as_value().into_owned();
    assert_eq!(dropped.get("existed").and_then(|v| v.as_bool()), Some(true));

    let mut second = Batch::new();
    second.id(2);
    second.op(
        "drop",
        ddl::drop_function("orphan")
            .hmac("test-hmac")
            .if_exists()
            .build()
            .expect("build DropFunction"),
    );
    let resp = db
        .execute(
            "testdb",
            &second
                .to_request_via_msgpack()
                .expect("batch request encoding"),
        )
        .await
        .unwrap();
    let dropped = resp.results["drop"].records[0].as_value().into_owned();
    assert_eq!(
        dropped.get("existed").and_then(|v| v.as_bool()),
        Some(false)
    );
}
