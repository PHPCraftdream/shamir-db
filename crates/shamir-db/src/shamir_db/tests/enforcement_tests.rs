use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::shamir_db::ShamirDb;
use shamir_types::access::{Action, Actor, Mode, ResourceMeta, ResourcePath};

// ============================================================================
// System actor always bypasses (behavior preservation)
// ============================================================================

#[tokio::test]
async fn system_actor_bypasses_all() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", config).await.unwrap();

    // Set restrictive mode — even mode 0o000 should not stop System.
    let meta = ResourceMeta {
        owner: Actor::User(1),
        group: None,
        mode: 0o000,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "data", "users"), &meta)
        .await
        .unwrap();

    for action in [
        Action::Read,
        Action::Write,
        Action::Create,
        Action::Delete,
        Action::Execute,
        Action::List,
        Action::Manage,
    ] {
        assert!(
            shamir
                .authorize_access(
                    &Actor::System,
                    &ResourcePath::table("testdb", "data", "users"),
                    action,
                )
                .await
                .is_ok(),
            "System should bypass for {action}"
        );
    }
}

// ============================================================================
// Owner can / other cannot per mode
// ============================================================================

#[tokio::test]
async fn owner_can_read_write_mode_700() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", config).await.unwrap();

    // G.4c: create defaults are now enforced (0o700, owner=System). For this
    // test the SUBJECT is the table's owner-only mode; the db/store ancestors
    // must be open so traversal-Execute passes and the target mode is the
    // sole gate. Open the ancestors explicitly.
    let open = ResourceMeta::open();
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(&ResourcePath::store("testdb", "data"), &open)
        .await
        .unwrap();

    let meta = ResourceMeta {
        owner: Actor::User(10),
        group: None,
        mode: 0o700,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "data", "users"), &meta)
        .await
        .unwrap();

    // Owner can.
    assert!(shamir
        .authorize_access(
            &Actor::User(10),
            &ResourcePath::table("testdb", "data", "users"),
            Action::Read,
        )
        .await
        .is_ok());
    assert!(shamir
        .authorize_access(
            &Actor::User(10),
            &ResourcePath::table("testdb", "data", "users"),
            Action::Write,
        )
        .await
        .is_ok());

    // Other cannot.
    assert!(shamir
        .authorize_access(
            &Actor::User(20),
            &ResourcePath::table("testdb", "data", "users"),
            Action::Read,
        )
        .await
        .is_err());
}

// ============================================================================
// Group member can via group bits
// ============================================================================

#[tokio::test]
async fn group_member_authorized_via_group_bits() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", config).await.unwrap();

    let gid = shamir.create_group("devs").await.unwrap();
    shamir.add_group_member(gid, 20).await.unwrap();

    // G.4c: open the db/store ancestors so traversal-Execute passes; the
    // table's group bits are the SUBJECT of this test.
    let open = ResourceMeta::open();
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(&ResourcePath::store("testdb", "data"), &open)
        .await
        .unwrap();

    let meta = ResourceMeta {
        owner: Actor::User(10),
        group: Some(gid),
        mode: 0o070,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "data", "users"), &meta)
        .await
        .unwrap();

    // Group member can read (group bits are rwx).
    assert!(shamir
        .authorize_access(
            &Actor::User(20),
            &ResourcePath::table("testdb", "data", "users"),
            Action::Read,
        )
        .await
        .is_ok());

    // Non-member, non-owner cannot.
    assert!(shamir
        .authorize_access(
            &Actor::User(30),
            &ResourcePath::table("testdb", "data", "users"),
            Action::Read,
        )
        .await
        .is_err());
}

// ============================================================================
// Traversal denied when ancestor lacks Execute
// ============================================================================

#[tokio::test]
async fn traversal_denied_without_execute_on_ancestor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", config).await.unwrap();

    // G.4c: the store ancestor now defaults to enforced (0o700, System-owned),
    // which would ALSO deny traversal. To isolate the DATABASE as the denied
    // ancestor (the test's SUBJECT), open the store so the db is the first
    // ancestor that denies Execute to User(99).
    shamir
        .set_resource_meta(
            &ResourcePath::store("testdb", "data"),
            &ResourceMeta::open(),
        )
        .await
        .unwrap();

    // Database: owner=User(10), mode=0o700 (no execute for others).
    let db_meta = ResourceMeta {
        owner: Actor::User(10),
        group: None,
        mode: 0o700,
    };
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &db_meta)
        .await
        .unwrap();

    // Table: open — but the traversal of the database ancestor fails first.
    let err = shamir
        .authorize_access(
            &Actor::User(99),
            &ResourcePath::table("testdb", "data", "users"),
            Action::Read,
        )
        .await
        .unwrap_err();
    // The denied path should be the database (ancestor), not the table.
    assert_eq!(err.path, "db://testdb");
    assert_eq!(err.action, Action::Execute);
}

#[tokio::test]
async fn traversal_allows_when_ancestors_grant_execute() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", config).await.unwrap();

    // Database + store: open (others have execute) so traversal passes.
    // G.4c: create defaults are now enforced (0o700), so we must open the
    // ancestors explicitly to exercise the "traversal passes, target denies"
    // path that this test verifies.
    let open = ResourceMeta::open();
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(&ResourcePath::store("testdb", "data"), &open)
        .await
        .unwrap();

    // Table: mode=0o700 (owner-only).
    let table_meta = ResourceMeta {
        owner: Actor::User(10),
        group: None,
        mode: 0o700,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "data", "users"), &table_meta)
        .await
        .unwrap();

    // Traversal of ancestors passes (opened above), but target is denied.
    let err = shamir
        .authorize_access(
            &Actor::User(99),
            &ResourcePath::table("testdb", "data", "users"),
            Action::Read,
        )
        .await
        .unwrap_err();
    assert_eq!(err.path, "db://testdb/data/users");
    assert_eq!(err.action, Action::Read);
}

// ============================================================================
// OPEN-default resource allows everyone
// ============================================================================

#[tokio::test]
async fn open_default_allows_any_user() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", config).await.unwrap();

    // G.4c: create defaults are now enforced (owner-rwx 0o700), so a stranger
    // is DENIED by default. Verify both paths:
    //   (1) enforced default denies a non-owner;
    //   (2) after an explicit chmod to OPEN (0o777), everyone is allowed.
    let stranger = Actor::User(99);
    let table_path = ResourcePath::table("testdb", "data", "users");

    // (1) Enforced default: stranger denied (traversal fails on System-owned
    //     0o700 ancestors before even reaching the table).
    assert!(shamir
        .authorize_access(&stranger, &table_path, Action::Read)
        .await
        .is_err());

    // (2) Explicit chmod to OPEN on db, store, and table: now everyone can.
    let open = ResourceMeta::open();
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(&ResourcePath::store("testdb", "data"), &open)
        .await
        .unwrap();
    shamir.set_resource_meta(&table_path, &open).await.unwrap();

    assert!(shamir
        .authorize_access(&stranger, &table_path, Action::Read)
        .await
        .is_ok());
    assert!(shamir
        .authorize_access(&stranger, &table_path, Action::Write)
        .await
        .is_ok());
}

// ============================================================================
// Manage is owner-only for non-System actors
// ============================================================================

#[tokio::test]
async fn manage_denied_for_non_owner() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;

    let db_meta = ResourceMeta {
        owner: Actor::User(10),
        group: None,
        mode: 0o777,
    };
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &db_meta)
        .await
        .unwrap();

    // Even with mode 0o777, non-owner cannot Manage.
    assert!(shamir
        .authorize_access(
            &Actor::User(20),
            &ResourcePath::database("testdb"),
            Action::Manage,
        )
        .await
        .is_err());

    // Owner can Manage.
    assert!(shamir
        .authorize_access(
            &Actor::User(10),
            &ResourcePath::database("testdb"),
            Action::Manage,
        )
        .await
        .is_ok());
}

// ============================================================================
// Record inherits table meta — enforcement respects inheritance
// ============================================================================

#[tokio::test]
async fn record_enforcement_inherits_table_meta() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("testdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    shamir.add_repo("testdb", config).await.unwrap();

    // G.4c: open the db/store ancestors so traversal-Execute passes; the
    // table's owner-only mode (inherited by records) is the SUBJECT.
    let open = ResourceMeta::open();
    shamir
        .set_resource_meta(&ResourcePath::database("testdb"), &open)
        .await
        .unwrap();
    shamir
        .set_resource_meta(&ResourcePath::store("testdb", "data"), &open)
        .await
        .unwrap();

    let table_meta = ResourceMeta {
        owner: Actor::User(10),
        group: None,
        mode: 0o700,
    };
    shamir
        .set_resource_meta(&ResourcePath::table("testdb", "data", "users"), &table_meta)
        .await
        .unwrap();

    // Record inherits the table's restrictive meta.
    assert!(shamir
        .authorize_access(
            &Actor::User(20),
            &ResourcePath::record("testdb", "data", "users", "key1"),
            Action::Read,
        )
        .await
        .is_err());

    // Owner of the table can read the record.
    assert!(shamir
        .authorize_access(
            &Actor::User(10),
            &ResourcePath::record("testdb", "data", "users", "key1"),
            Action::Read,
        )
        .await
        .is_ok());
}

// ============================================================================
// setuid: effective actor switches to function owner
// ============================================================================

#[tokio::test]
async fn effective_fn_actor_switches_on_setuid() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // Without setuid, effective actor is the caller (open defaults → no setuid).
    let caller = Actor::User(42);
    let effective = shamir.effective_fn_actor("nonexistent", &caller).await;
    assert_eq!(effective, Actor::User(42));

    // Create a real catalogue entry for a function, then set its meta to setuid.
    use base64::Engine;
    use shamir_types::types::common::new_map;
    use shamir_types::types::value::QueryValue;

    let wasm_b64 = base64::engine::general_purpose::STANDARD.encode(b"\x00asm\x01\x00\x00\x00");
    let mut fn_rec_map = new_map();
    fn_rec_map.insert("name".to_string(), QueryValue::Str("suid_fn".to_string()));
    fn_rec_map.insert("wasm_b64".to_string(), QueryValue::Str(wasm_b64));
    fn_rec_map.insert(
        "owner".to_string(),
        QueryValue::Int(Actor::User(10).to_owner_id() as i64),
    );
    fn_rec_map.insert("group".to_string(), QueryValue::Null);
    fn_rec_map.insert(
        "mode".to_string(),
        QueryValue::Int(Mode::with_setuid(0o755, true) as i64),
    );
    let fn_rec = QueryValue::Map(fn_rec_map);
    shamir
        .system_store()
        .save_function(
            "suid_fn",
            &fn_rec,
            &ResourceMeta {
                owner: Actor::User(10),
                group: None,
                mode: Mode::with_setuid(0o755, true),
            },
        )
        .await
        .unwrap();

    let effective = shamir.effective_fn_actor("suid_fn", &caller).await;
    assert_eq!(effective, Actor::User(10));
}

// Verify fail-closed: a missing (or unreadable) function record must
// never escalate the caller to Actor::System via an open()-default.
#[tokio::test]
async fn effective_fn_actor_missing_meta_returns_caller_not_system() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let caller = Actor::User(99);

    // "ghost_fn" was never registered — load_function returns Ok(None).
    let effective = shamir.effective_fn_actor("ghost_fn", &caller).await;
    assert_eq!(
        effective,
        Actor::User(99),
        "missing meta must return caller, never Actor::System"
    );
    assert_ne!(
        effective,
        Actor::System,
        "escalation to System via open()-default must be impossible"
    );
}
