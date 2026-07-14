use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::query::admin::GroupRef;
use crate::shamir_db::ShamirDb;
use crate::shamir_db::SystemStoreConfig;
use crate::DbError;
use shamir_types::access::{Actor, Mode, PermClass, ResourceMeta, ResourcePath};

// ============================================================================
// Catalogue round-trip: resource_meta returns enforced defaults on create
//
// G.4c (Strategy A): new mode-bearing objects are owner-rwx (0o700), private
// to their creator. Legacy records without a `mode` field still load as OPEN
// via `from_record` — see `root_meta_defaults_to_open` / `unknown_database_*`
// below for the OPEN-default path (Root is synthetic and never persisted).
// ============================================================================

#[tokio::test]
async fn database_resource_meta_defaults_to_enforced() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    let meta = shamir
        .resource_meta(&ResourcePath::database("testdb"))
        .await
        .unwrap();
    // create_db defaults to Actor::System; enforced default is owner-rwx 0o700.
    let enforced = ResourceMeta::owned_enforced(Actor::System);
    assert_eq!(meta.owner, enforced.owner);
    assert_eq!(meta.group, enforced.group);
    assert_eq!(meta.mode, enforced.mode);
}

#[tokio::test]
async fn store_resource_meta_defaults_to_enforced() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config = RepoConfig::new("data", BoxRepoFactory::in_memory());
    shamir.add_repo("testdb", config).await.unwrap();

    let meta = shamir
        .resource_meta(&ResourcePath::store("testdb", "data"))
        .await
        .unwrap();
    // add_repo defaults to Actor::System; enforced default is owner-rwx 0o700.
    let enforced = ResourceMeta::owned_enforced(Actor::System);
    assert_eq!(meta.owner, enforced.owner);
    assert_eq!(meta.group, enforced.group);
    assert_eq!(meta.mode, enforced.mode);
}

#[tokio::test]
async fn table_resource_meta_defaults_to_enforced() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", config).await.unwrap();

    let meta = shamir
        .resource_meta(&ResourcePath::table("testdb", "data", "users"))
        .await
        .unwrap();
    // add_repo defaults to Actor::System; inline tables inherit the enforced
    // owner-rwx 0o700 default (Strategy A).
    let enforced = ResourceMeta::owned_enforced(Actor::System);
    assert_eq!(meta.owner, enforced.owner);
    assert_eq!(meta.group, enforced.group);
    assert_eq!(meta.mode, enforced.mode);
}

// ============================================================================
// set_resource_meta then resource_meta returns the set values
// ============================================================================

#[tokio::test]
async fn set_database_meta_round_trip() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    let custom = ResourceMeta {
        owner: Actor::User(42),
        group: Some(7),
        mode: 0o750,
    };
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &custom)
        .await
        .unwrap();

    let loaded = shamir
        .resource_meta(&ResourcePath::database("testdb"))
        .await
        .unwrap();
    assert_eq!(loaded.owner, Actor::User(42));
    assert_eq!(loaded.group, Some(7));
    assert_eq!(loaded.mode, 0o750);
}

#[tokio::test]
async fn set_store_meta_round_trip() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config = RepoConfig::new("data", BoxRepoFactory::in_memory());
    shamir.add_repo("testdb", config).await.unwrap();

    let custom = ResourceMeta {
        owner: Actor::User(10),
        group: Some(3),
        mode: 0o770,
    };
    shamir
        .set_resource_meta(&ResourcePath::store("testdb", "data"), &custom)
        .await
        .unwrap();

    let loaded = shamir
        .resource_meta(&ResourcePath::store("testdb", "data"))
        .await
        .unwrap();
    assert_eq!(loaded.owner, Actor::User(10));
    assert_eq!(loaded.group, Some(3));
    assert_eq!(loaded.mode, 0o770);
}

#[tokio::test]
async fn set_table_meta_round_trip() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", config).await.unwrap();

    let custom = ResourceMeta {
        owner: Actor::User(1),
        group: Some(2),
        mode: 0o644,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "data", "users"), &custom)
        .await
        .unwrap();

    let loaded = shamir
        .resource_meta(&ResourcePath::table("testdb", "data", "users"))
        .await
        .unwrap();
    assert_eq!(loaded.owner, Actor::User(1));
    assert_eq!(loaded.group, Some(2));
    assert_eq!(loaded.mode, 0o644);
}

#[tokio::test]
async fn set_function_namespace_meta_round_trip() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let custom = ResourceMeta {
        owner: Actor::User(5),
        group: Some(10),
        mode: 0o755,
    };
    shamir
        .set_resource_meta(&ResourcePath::FunctionNamespace, &custom)
        .await
        .unwrap();

    let loaded = shamir
        .resource_meta(&ResourcePath::FunctionNamespace)
        .await
        .unwrap();
    assert_eq!(loaded.owner, Actor::User(5));
    assert_eq!(loaded.group, Some(10));
    assert_eq!(loaded.mode, 0o755);
}

// ============================================================================
// Inheritance: Record/Index inherit their Table's meta
// ============================================================================

#[tokio::test]
async fn record_inherits_table_meta() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", config).await.unwrap();

    let custom = ResourceMeta {
        owner: Actor::User(99),
        group: Some(5),
        mode: 0o600,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "data", "users"), &custom)
        .await
        .unwrap();

    let rec_meta = shamir
        .resource_meta(&ResourcePath::record("testdb", "data", "users", "key1"))
        .await
        .unwrap();
    assert_eq!(rec_meta.owner, Actor::User(99));
    assert_eq!(rec_meta.group, Some(5));
    assert_eq!(rec_meta.mode, 0o600);
}

#[tokio::test]
async fn index_inherits_table_meta() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", config).await.unwrap();

    let custom = ResourceMeta {
        owner: Actor::User(77),
        group: Some(3),
        mode: 0o740,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "data", "users"), &custom)
        .await
        .unwrap();

    let idx_meta = shamir
        .resource_meta(&ResourcePath::index("testdb", "data", "users", "email_idx"))
        .await
        .unwrap();
    assert_eq!(idx_meta.owner, Actor::User(77));
    assert_eq!(idx_meta.group, Some(3));
    assert_eq!(idx_meta.mode, 0o740);
}

// ============================================================================
// Root and unknown paths default to open
// ============================================================================

// Task #552: Root now resolves to full persisted meta (settings key
// "root_meta"), defaulting to System-owned `0o751` (task #615/#620 — no
// Other-Read, so top-level enumeration is closed by default, but
// Other-Execute stays so ancestor-traversal into nested resources an
// actor separately holds rights on is not collaterally broken) when
// absent — NOT the universal-open `ResourceMeta::open()` (`0o777`) this
// test used to assert.
// See `root_user_group_meta_tests.rs::root_meta_defaults_to_system_0o751_when_absent`
// for the dedicated coverage; this test is updated in lock-step so it
// doesn't silently assert stale pre-#552/#620 behavior.
#[tokio::test]
async fn root_meta_defaults_to_system_0o751() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let meta = shamir.resource_meta(&ResourcePath::Root).await.unwrap();
    assert_eq!(meta.owner, Actor::System);
    assert_eq!(meta.group, None);
    assert_eq!(meta.mode, 0o751);
}

#[tokio::test]
async fn unknown_database_defaults_to_open() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let meta = shamir
        .resource_meta(&ResourcePath::database("nonexistent"))
        .await
        .unwrap();
    assert_eq!(meta, ResourceMeta::open());
}

// ============================================================================
// Groups: create, add/remove members, user_in_group
// ============================================================================

#[tokio::test]
async fn create_group_and_check_members() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let gid = shamir.create_group("admins").await.unwrap();
    assert!(gid >= 1);

    let members = shamir.group_members(gid).await.unwrap();
    assert!(members.is_empty());
}

#[tokio::test]
async fn add_and_remove_group_members() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let gid = shamir.create_group("devs").await.unwrap();

    shamir.add_group_member(gid, 10).await.unwrap();
    shamir.add_group_member(gid, 20).await.unwrap();
    // Adding same user twice is idempotent.
    shamir.add_group_member(gid, 10).await.unwrap();

    let members = shamir.group_members(gid).await.unwrap();
    assert!(members.contains(&10));
    assert!(members.contains(&20));
    assert_eq!(members.len(), 2);

    assert!(shamir.user_in_group(10, gid).await.unwrap());
    assert!(shamir.user_in_group(20, gid).await.unwrap());
    assert!(!shamir.user_in_group(99, gid).await.unwrap());

    shamir.remove_group_member(gid, 10).await.unwrap();
    assert!(!shamir.user_in_group(10, gid).await.unwrap());
    assert!(shamir.user_in_group(20, gid).await.unwrap());
}

#[tokio::test]
async fn drop_group_removes_it() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let gid = shamir.create_group("temp").await.unwrap();
    shamir.add_group_member(gid, 1).await.unwrap();
    shamir.drop_group(gid).await.unwrap();

    let members = shamir.group_members(gid).await.unwrap();
    assert!(members.is_empty());
    assert!(!shamir.user_in_group(1, gid).await.unwrap());
}

// ============================================================================
// rename_group — id-keyed display-name swap, no reference rekey
// ============================================================================

#[tokio::test]
async fn rename_group_by_name_preserves_id_and_members() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let gid = shamir.create_group("devs").await.unwrap();
    shamir.add_group_member(gid, 10).await.unwrap();
    shamir.add_group_member(gid, 20).await.unwrap();

    shamir
        .rename_group(
            &GroupRef::Name {
                name: "devs".to_string(),
            },
            "engineers",
        )
        .await
        .unwrap();

    // New name resolves to the SAME group id.
    let gid_after = shamir
        .resolve_group_id(&GroupRef::Name {
            name: "engineers".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(gid_after, gid);

    // Old name no longer resolves.
    let err = shamir
        .resolve_group_id(&GroupRef::Name {
            name: "devs".to_string(),
        })
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::NotFound(_)), "got {err:?}");

    // Members preserved across the rename.
    let members = shamir.group_members(gid).await.unwrap();
    assert!(members.contains(&10));
    assert!(members.contains(&20));
    assert_eq!(members.len(), 2);
}

#[tokio::test]
async fn rename_group_by_id_preserves_members() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let gid = shamir.create_group("qa").await.unwrap();
    shamir.add_group_member(gid, 7).await.unwrap();

    shamir
        .rename_group(&GroupRef::Id { id: gid }, "qa-renamed")
        .await
        .unwrap();

    assert_eq!(
        shamir
            .resolve_group_id(&GroupRef::Name {
                name: "qa-renamed".to_string()
            })
            .await
            .unwrap(),
        gid
    );
    assert_eq!(shamir.group_members(gid).await.unwrap(), vec![7]);
}

#[tokio::test]
async fn rename_group_to_its_own_name_is_idempotent() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let gid = shamir.create_group("self").await.unwrap();
    // Renaming to the current name is a tolerated no-op.
    shamir
        .rename_group(
            &GroupRef::Name {
                name: "self".to_string(),
            },
            "self",
        )
        .await
        .unwrap();
    assert_eq!(
        shamir
            .resolve_group_id(&GroupRef::Name {
                name: "self".to_string()
            })
            .await
            .unwrap(),
        gid
    );
}

#[tokio::test]
async fn rename_group_to_taken_name_is_key_exists() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let _a = shamir.create_group("alpha").await.unwrap();
    let b = shamir.create_group("beta").await.unwrap();

    let err = shamir
        .rename_group(&GroupRef::Id { id: b }, "alpha")
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::KeyExists(_)), "got {err:?}");

    // Source group unchanged on failure.
    assert_eq!(
        shamir
            .resolve_group_id(&GroupRef::Name {
                name: "beta".to_string()
            })
            .await
            .unwrap(),
        b
    );
}

#[tokio::test]
async fn rename_group_missing_is_not_found() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let err = shamir
        .rename_group(
            &GroupRef::Name {
                name: "ghost".to_string(),
            },
            "ghosts",
        )
        .await
        .unwrap_err();
    assert!(matches!(err, DbError::NotFound(_)), "got {err:?}");
}

#[tokio::test]
async fn rename_group_does_not_break_resource_group_ref() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    let gid = shamir.create_group("devs").await.unwrap();

    // Stamp a database resource with group = gid, then rename the group.
    let mut meta = shamir
        .resource_meta(&ResourcePath::database("testdb"))
        .await
        .unwrap();
    meta.group = Some(gid);
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &meta)
        .await
        .unwrap();

    shamir
        .rename_group(&GroupRef::Id { id: gid }, "engineers")
        .await
        .unwrap();

    // The resource still references the same (immutable) group id.
    let after = shamir
        .resource_meta(&ResourcePath::database("testdb"))
        .await
        .unwrap();
    assert_eq!(after.group, Some(gid));
}

#[tokio::test]
async fn group_ids_are_monotonic() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let g1 = shamir.create_group("a").await.unwrap();
    let g2 = shamir.create_group("b").await.unwrap();
    let g3 = shamir.create_group("c").await.unwrap();
    assert!(g1 < g2);
    assert!(g2 < g3);
}

// ============================================================================
// Persistence across re-open (redb)
// ============================================================================

#[tokio::test]
async fn resource_meta_survives_reopen() {
    let sys_dir = tempfile::tempdir().unwrap();
    let sys_path = sys_dir.path().join("system.redb");

    {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        shamir.create_db("testdb").await;
        let config = RepoConfig::new("data", BoxRepoFactory::in_memory())
            .add_table(TableConfig::new("users"));
        shamir.add_repo("testdb", config).await.unwrap();

        let custom = ResourceMeta {
            owner: Actor::User(42),
            group: Some(7),
            mode: 0o750,
        };
        shamir
            .set_resource_meta(&ResourcePath::database("testdb"), &custom)
            .await
            .unwrap();
        shamir
            .set_resource_meta(&ResourcePath::table("testdb", "data", "users"), &custom)
            .await
            .unwrap();
    }

    // Re-open
    let shamir = reinit_with_retry(sys_path).await;

    let db_meta = shamir
        .resource_meta(&ResourcePath::database("testdb"))
        .await
        .unwrap();
    assert_eq!(db_meta.owner, Actor::User(42));
    assert_eq!(db_meta.group, Some(7));
    assert_eq!(db_meta.mode, 0o750);

    let tbl_meta = shamir
        .resource_meta(&ResourcePath::table("testdb", "data", "users"))
        .await
        .unwrap();
    assert_eq!(tbl_meta.owner, Actor::User(42));
    assert_eq!(tbl_meta.group, Some(7));
    assert_eq!(tbl_meta.mode, 0o750);
}

#[tokio::test]
async fn groups_survive_reopen() {
    let sys_dir = tempfile::tempdir().unwrap();
    let sys_path = sys_dir.path().join("system.redb");

    let gid = {
        let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
            .await
            .unwrap();
        let gid = shamir.create_group("devs").await.unwrap();
        shamir.add_group_member(gid, 10).await.unwrap();
        shamir.add_group_member(gid, 20).await.unwrap();
        gid
    };

    let shamir = reinit_with_retry(sys_path).await;

    assert!(shamir.user_in_group(10, gid).await.unwrap());
    assert!(shamir.user_in_group(20, gid).await.unwrap());
    assert!(!shamir.user_in_group(99, gid).await.unwrap());
}

// ============================================================================
// Mode helpers integration
// ============================================================================

#[tokio::test]
async fn mode_helpers_on_real_meta() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    let meta = ResourceMeta {
        owner: Actor::User(1),
        group: Some(2),
        mode: Mode::with_setuid(0o750, true),
    };
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &meta)
        .await
        .unwrap();

    let loaded = shamir
        .resource_meta(&ResourcePath::database("testdb"))
        .await
        .unwrap();
    assert!(Mode::is_setuid(loaded.mode));
    assert!(Mode::is_set(
        loaded.mode,
        PermClass::Owner,
        shamir_types::access::Perm::Read
    ));
    assert!(Mode::is_set(
        loaded.mode,
        PermClass::Group,
        shamir_types::access::Perm::Read
    ));
    assert!(!Mode::is_set(
        loaded.mode,
        PermClass::Other,
        shamir_types::access::Perm::Write
    ));
}

// ============================================================================
// Re-open helper (shared with system_metadata_tests)
// ============================================================================

async fn reinit_with_retry(sys_path: std::path::PathBuf) -> ShamirDb {
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone())).await {
            Ok(shamir) => return shamir,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .expect("system store still locked after retries")
}

// ============================================================================
// Audit #540 — resource_meta / authorize_access fail-CLOSED on a real
// catalogue-read error (not fail-open into ResourceMeta::default()==open()).
//
// A `Store` fault double armed to fail every read is spliced into the
// SYSTEM_REPO's "databases" TableManager via
// `RepoInstance::install_table_for_test` (a #[cfg(test)]-only seam added
// alongside this fix — see `shamir-engine`'s `repo_instance.rs`). This lets
// the test force `system_store.load_database` to return a REAL `Err`
// (not `Ok(None)`) through the exact same code path `resource_meta` uses,
// without a large `BoxRepo`/`BoxRepoFactory` fault-injection rewrite.
// ============================================================================

/// Fault-injecting `Store` double: wraps an in-memory store and, once
/// armed, fails every read (`get`, `get_many`, `iter_stream`,
/// `scan_prefix_stream`) with a `DbError::Storage` — simulating a real
/// catalogue-page-corruption / I/O failure rather than "record absent".
mod failing_store {
    use async_trait::async_trait;
    use bytes::Bytes;
    use shamir_storage::error::{DbError, DbResult};
    use shamir_storage::storage_in_memory::InMemoryStore;
    use shamir_storage::types::{KvOp, RecordKey, Store};
    use std::pin::Pin;
    use std::sync::atomic::{AtomicBool, Ordering};

    use futures::stream::Stream;

    pub struct FailingStore {
        inner: InMemoryStore,
        pub armed: AtomicBool,
    }

    impl FailingStore {
        pub fn new() -> Self {
            Self {
                inner: InMemoryStore::new(),
                armed: AtomicBool::new(false),
            }
        }

        fn injected_error() -> DbError {
            DbError::Storage("injected I/O fault (audit #540 regression test)".into())
        }

        fn check(&self) -> DbResult<()> {
            if self.armed.load(Ordering::SeqCst) {
                Err(Self::injected_error())
            } else {
                Ok(())
            }
        }
    }

    #[async_trait]
    impl Store for FailingStore {
        async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
            self.inner.insert(value).await
        }

        async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
            self.inner.set(key, value).await
        }

        async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
            self.check()?;
            self.inner.get(key).await
        }

        async fn get_many(&self, keys: Vec<RecordKey>) -> DbResult<Vec<Option<Bytes>>> {
            self.check()?;
            self.inner.get_many(keys).await
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
            if self.armed.load(Ordering::SeqCst) {
                let err = Self::injected_error();
                return Box::pin(futures::stream::once(async move { Err(err) }));
            }
            self.inner.iter_stream(batch_size)
        }

        fn scan_prefix_stream(
            &self,
            prefix: Bytes,
            batch_size: usize,
        ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
            if self.armed.load(Ordering::SeqCst) {
                let err = Self::injected_error();
                return Box::pin(futures::stream::once(async move { Err(err) }));
            }
            self.inner.scan_prefix_stream(prefix, batch_size)
        }
    }
}

/// Build a `ShamirDb` whose SYSTEM_REPO "databases" table is backed by a
/// `FailingStore`, returning the `ShamirDb` plus a handle to arm/disarm the
/// injected fault. The table starts unarmed so normal catalogue writes
/// (`create_db`) succeed; the test arms the fault right before the
/// `resource_meta`/`authorize_access` call under test.
async fn shamir_with_failing_databases_table(
) -> (ShamirDb, std::sync::Arc<failing_store::FailingStore>) {
    use crate::engine::table::TableManager;

    let shamir = ShamirDb::init_memory().await.unwrap();

    let data_store = std::sync::Arc::new(failing_store::FailingStore::new());
    let info_store: std::sync::Arc<dyn shamir_storage::types::Store> =
        std::sync::Arc::new(shamir_storage::storage_in_memory::InMemoryStore::new());
    let tbl = TableManager::create(
        "databases".to_string(),
        data_store.clone() as std::sync::Arc<dyn shamir_storage::types::Store>,
        info_store,
    )
    .await
    .unwrap();

    let system_repo = shamir.system_store().system_repo().unwrap();
    system_repo.install_table_for_test("databases", tbl);

    (shamir, data_store)
}

#[tokio::test]
async fn resource_meta_fails_closed_on_storage_error() {
    let (shamir, fault) = shamir_with_failing_databases_table().await;
    shamir.create_db("testdb").await;

    // Sanity: unarmed, the normal round-trip still works (Ok(Some(..))).
    let meta = shamir
        .resource_meta(&ResourcePath::database("testdb"))
        .await
        .unwrap();
    assert_eq!(meta.owner, Actor::System);

    // Arm the fault: the next catalogue read returns a REAL Err, not
    // Ok(None). resource_meta must propagate it, NOT collapse it into
    // ResourceMeta::default() (owner=System, mode 0o777).
    fault.armed.store(true, std::sync::atomic::Ordering::SeqCst);

    let result = shamir
        .resource_meta(&ResourcePath::database("testdb"))
        .await;
    assert!(
        result.is_err(),
        "resource_meta must propagate a real storage error as Err, \
         not fail-open into a default-open ResourceMeta"
    );
}

#[tokio::test]
async fn authorize_access_denies_when_resource_meta_errors() {
    let (shamir, fault) = shamir_with_failing_databases_table().await;
    shamir.create_db("testdb").await;

    // Under the OLD fail-open code, a database whose meta read errors would
    // collapse to ResourceMeta::default() == open() (owner=System,
    // mode 0o777) — any Actor::User would then be PERMITTED Read via the
    // Other-rwx bits. Confirm the actor is a non-owner (User(999) is not
    // System, and default owner is System) so a fail-open bug would show
    // up as `Ok(())` here.
    let actor = Actor::User(999);

    fault.armed.store(true, std::sync::atomic::Ordering::SeqCst);

    let result = shamir
        .authorize_access(
            &actor,
            &ResourcePath::database("testdb"),
            crate::access::Action::Read,
        )
        .await;
    assert!(
        result.is_err(),
        "authorize_access must deny (Err) when resource_meta fails with a \
         real storage error — a fail-open bug would return Ok(()) here \
         because the old default-open ResourceMeta permits every actor"
    );
}

// ============================================================================
// Task #602 — resource_meta's ResourcePath::Group branch fails CLOSED on a
// real catalogue-read error from `resolve_group_id`, mirroring the Database
// branch above (audit #540). `resolve_group_id` returns `DbResult<u64>`
// (not `DbResult<Option<u64>>`), so the fix matches on the `Err` variant
// itself: `DbError::NotFound(_)` is the confirmed-absent case (legitimate
// fallback to `ResourceMeta::open()`), any other `Err` (a real storage
// fault) must propagate.
// ============================================================================

/// Build a `ShamirDb` whose SYSTEM_REPO "groups" table is backed by a
/// `FailingStore`, returning the `ShamirDb` plus a handle to arm/disarm the
/// injected fault. The table starts unarmed so normal catalogue writes
/// (`create_group`) succeed; the test arms the fault right before the
/// `resource_meta`/`authorize_access` call under test.
async fn shamir_with_failing_groups_table(
) -> (ShamirDb, std::sync::Arc<failing_store::FailingStore>) {
    use crate::engine::table::TableManager;

    let shamir = ShamirDb::init_memory().await.unwrap();

    let data_store = std::sync::Arc::new(failing_store::FailingStore::new());
    let info_store: std::sync::Arc<dyn shamir_storage::types::Store> =
        std::sync::Arc::new(shamir_storage::storage_in_memory::InMemoryStore::new());
    let tbl = TableManager::create(
        "groups".to_string(),
        data_store.clone() as std::sync::Arc<dyn shamir_storage::types::Store>,
        info_store,
    )
    .await
    .unwrap();

    let system_repo = shamir.system_store().system_repo().unwrap();
    system_repo.install_table_for_test("groups", tbl);

    (shamir, data_store)
}

#[tokio::test]
async fn resource_meta_group_fails_closed_on_storage_error() {
    let (shamir, fault) = shamir_with_failing_groups_table().await;
    shamir.create_group("testgroup").await.unwrap();

    // Sanity: unarmed, the call succeeds (mirrors the Database fault-
    // injection sanity check above: it confirms the unarmed path doesn't
    // already error, not that the catalogue round-trip is deep-verified —
    // that's covered by `group_resource_meta_reports_persisted_owner_and_group_class`
    // in `root_user_group_meta_tests.rs`, which runs against the real
    // (non-swapped) SYSTEM_REPO tables).
    let meta = shamir
        .resource_meta(&ResourcePath::Group {
            name: "testgroup".into(),
        })
        .await
        .unwrap();
    assert_eq!(meta.owner, Actor::System);

    // Arm the fault: the next catalogue read (via resolve_group_id ->
    // load_groups) returns a REAL Err, not a confirmed not-found. resource_meta
    // must propagate it, NOT collapse it into ResourceMeta::open() (owner=
    // System, mode 0o777 — accessible to everyone).
    fault.armed.store(true, std::sync::atomic::Ordering::SeqCst);

    let result = shamir
        .resource_meta(&ResourcePath::Group {
            name: "testgroup".into(),
        })
        .await;
    assert!(
        result.is_err(),
        "resource_meta must propagate a real storage error from \
         resolve_group_id as Err, not fail-open into a default-open \
         ResourceMeta"
    );
}

#[tokio::test]
async fn authorize_access_denies_when_group_resource_meta_errors() {
    let (shamir, fault) = shamir_with_failing_groups_table().await;
    shamir.create_group("testgroup").await.unwrap();

    // Under the OLD fail-open code, a group whose resolve_group_id call
    // errors would collapse to ResourceMeta::open() (owner=System, mode
    // 0o777) — any Actor::User would then be PERMITTED Read via the
    // Other-rwx bits. Confirm the actor is a non-owner (User(999) is not
    // System, and default owner is System) so a fail-open bug would show
    // up as `Ok(())` here.
    let actor = Actor::User(999);

    fault.armed.store(true, std::sync::atomic::Ordering::SeqCst);

    let result = shamir
        .authorize_access(
            &actor,
            &ResourcePath::Group {
                name: "testgroup".into(),
            },
            crate::access::Action::Read,
        )
        .await;
    assert!(
        result.is_err(),
        "authorize_access must deny (Err) when resource_meta for a Group \
         fails with a real storage error — a fail-open bug would return \
         Ok(()) here because the old default-open ResourceMeta permits \
         every actor"
    );
}
