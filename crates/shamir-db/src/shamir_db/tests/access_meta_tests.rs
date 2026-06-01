use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::shamir_db::ShamirDb;
use crate::shamir_db::SystemStoreConfig;
use shamir_types::access::{Actor, Mode, PermClass, ResourceMeta, ResourcePath};

// ============================================================================
// Catalogue round-trip: resource_meta returns open defaults on create
// ============================================================================

#[tokio::test]
async fn database_resource_meta_defaults_to_open() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    let meta = shamir
        .resource_meta(&ResourcePath::database("testdb"))
        .await;
    let open = ResourceMeta::open();
    assert_eq!(meta.owner, open.owner);
    assert_eq!(meta.group, open.group);
    assert_eq!(meta.mode, open.mode);
}

#[tokio::test]
async fn store_resource_meta_defaults_to_open() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config = RepoConfig::new("data", BoxRepoFactory::in_memory());
    shamir.add_repo("testdb", config).await.unwrap();

    let meta = shamir
        .resource_meta(&ResourcePath::store("testdb", "data"))
        .await;
    let open = ResourceMeta::open();
    assert_eq!(meta.owner, open.owner);
    assert_eq!(meta.group, open.group);
    assert_eq!(meta.mode, open.mode);
}

#[tokio::test]
async fn table_resource_meta_defaults_to_open() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", config).await.unwrap();

    let meta = shamir
        .resource_meta(&ResourcePath::table("testdb", "data", "users"))
        .await;
    let open = ResourceMeta::open();
    assert_eq!(meta.owner, open.owner);
    assert_eq!(meta.group, open.group);
    assert_eq!(meta.mode, open.mode);
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
        .await;
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
        .await;
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
        .await;
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

    let loaded = shamir.resource_meta(&ResourcePath::FunctionNamespace).await;
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
        .await;
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
        .await;
    assert_eq!(idx_meta.owner, Actor::User(77));
    assert_eq!(idx_meta.group, Some(3));
    assert_eq!(idx_meta.mode, 0o740);
}

// ============================================================================
// Root and unknown paths default to open
// ============================================================================

#[tokio::test]
async fn root_meta_defaults_to_open() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let meta = shamir.resource_meta(&ResourcePath::Root).await;
    assert_eq!(meta, ResourceMeta::open());
}

#[tokio::test]
async fn unknown_database_defaults_to_open() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let meta = shamir
        .resource_meta(&ResourcePath::database("nonexistent"))
        .await;
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
        let shamir = ShamirDb::init(SystemStoreConfig::Redb(sys_path.clone()))
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
        .await;
    assert_eq!(db_meta.owner, Actor::User(42));
    assert_eq!(db_meta.group, Some(7));
    assert_eq!(db_meta.mode, 0o750);

    let tbl_meta = shamir
        .resource_meta(&ResourcePath::table("testdb", "data", "users"))
        .await;
    assert_eq!(tbl_meta.owner, Actor::User(42));
    assert_eq!(tbl_meta.group, Some(7));
    assert_eq!(tbl_meta.mode, 0o750);
}

#[tokio::test]
async fn groups_survive_reopen() {
    let sys_dir = tempfile::tempdir().unwrap();
    let sys_path = sys_dir.path().join("system.redb");

    let gid = {
        let shamir = ShamirDb::init(SystemStoreConfig::Redb(sys_path.clone()))
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
        .await;
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
        match ShamirDb::init(SystemStoreConfig::Redb(sys_path.clone())).await {
            Ok(shamir) => return shamir,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    ShamirDb::init(SystemStoreConfig::Redb(sys_path))
        .await
        .expect("system store still locked after retries")
}
