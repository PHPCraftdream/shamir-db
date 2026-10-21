use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::engine::table::TableConfig;
use crate::shamir_db::ShamirDb;
// CRIT-6 part B — `Security` declares INVOKER/DEFINER semantics now
// honoured by `effective_fn_actor`.
use shamir_engine::function::Security;
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

// ============================================================================
// CRIT-6 part B / audit #440 — Security::Definer / Invoker enforcement
// ============================================================================
//
// `effective_fn_actor` now consults `FunctionMeta::security` in addition to
// the legacy POSIX setuid mode bit. Decision table implemented:
//
//   security=Definer, any mode      → owner
//   security=Invoker, setuid set    → owner   (legacy-compat)
//   security=Invoker, setuid clear  → caller

/// Test helper: persist a function catalogue entry with the given owner,
/// setuid mode bit, and declared `Security`, then return the `ShamirDb`.
///
/// Mirrors the record shape used by `effective_fn_actor_switches_on_setuid`
/// above, with the addition of an explicit `security` field injected via
/// `FunctionMeta::inject_into`.
async fn make_fn_with_security(
    shamir: &ShamirDb,
    name: &str,
    owner: Actor,
    setuid: bool,
    security: shamir_engine::function::Security,
) {
    use base64::Engine;
    use shamir_types::types::common::new_map;
    use shamir_types::types::value::QueryValue;

    let wasm_b64 = base64::engine::general_purpose::STANDARD.encode(b"\x00asm\x01\x00\x00\x00");
    let mut fn_rec_map = new_map();
    fn_rec_map.insert("name".to_string(), QueryValue::Str(name.to_string()));
    fn_rec_map.insert("wasm_b64".to_string(), QueryValue::Str(wasm_b64));
    fn_rec_map.insert(
        "owner".to_string(),
        QueryValue::Int(owner.to_owner_id() as i64),
    );
    fn_rec_map.insert("group".to_string(), QueryValue::Null);
    let mode = if setuid {
        Mode::with_setuid(0o755, true)
    } else {
        0o755
    };
    fn_rec_map.insert("mode".to_string(), QueryValue::Int(mode as i64));
    let mut fn_rec = QueryValue::Map(fn_rec_map);
    // Inject the declared `security` (and visibility / empty grants) into
    // the persisted record so `FunctionMeta::from_record` reads it back.
    shamir_engine::function::FunctionMeta::new(
        shamir_engine::function::Visibility::Private,
        security,
        Vec::new(),
        Vec::new(),
    )
    .inject_into(&mut fn_rec);

    shamir
        .system_store()
        .save_function(
            name,
            &fn_rec,
            &ResourceMeta {
                owner,
                group: None,
                mode,
            },
        )
        .await
        .unwrap();
}

/// `Security::Definer` ALWAYS escalates to the function owner, even when
/// the setuid mode bit is NOT set — the explicit declaration is what
/// drives escalation, not the legacy bit.
#[tokio::test]
async fn effective_fn_actor_definer_escalates_without_setuid() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let owner = Actor::User(10);
    let caller = Actor::User(42);

    make_fn_with_security(
        &shamir,
        "definer_fn",
        owner.clone(),
        false,
        Security::Definer,
    )
    .await;

    let effective = shamir.effective_fn_actor("definer_fn", &caller).await;
    assert_eq!(
        effective, owner,
        "security=definer must escalate to owner even without setuid"
    );
    assert_ne!(effective, caller);
}

/// `Security::Definer` with the setuid bit ALSO set still escalates (the
/// bit is redundant under Definer but must not break the path).
#[tokio::test]
async fn effective_fn_actor_definer_with_setuid_still_escalates() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let owner = Actor::User(11);
    let caller = Actor::User(42);

    make_fn_with_security(
        &shamir,
        "definer_suid_fn",
        owner.clone(),
        true,
        Security::Definer,
    )
    .await;

    let effective = shamir.effective_fn_actor("definer_suid_fn", &caller).await;
    assert_eq!(effective, owner);
}

/// `Security::Invoker` WITHOUT the setuid bit must NOT escalate — the
/// caller is returned unchanged. This is the base case and must not
/// regress pre-CRIT-6 behaviour for the common `invoker` + plain-mode
/// function.
#[tokio::test]
async fn effective_fn_actor_invoker_without_setuid_returns_caller() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let owner = Actor::User(10);
    let caller = Actor::User(42);

    make_fn_with_security(
        &shamir,
        "invoker_fn",
        owner.clone(),
        false,
        Security::Invoker,
    )
    .await;

    let effective = shamir.effective_fn_actor("invoker_fn", &caller).await;
    assert_eq!(effective, caller);
    assert_ne!(effective, owner);
}

/// `Security::Invoker` WITH the legacy setuid bit still escalates, per
/// the chosen legacy-compatible semantics (see the doc note on
/// `effective_fn_actor`). This preserves backward compatibility for
/// callers that relied on the setuid bit alone — the primary CRIT-6
/// defect (`Definer` was ignored) is closed by the
/// `definer_*` tests above; this case documents the deliberate
/// legacy-compat carve-out for `Invoker` + setuid.
#[tokio::test]
async fn effective_fn_actor_invoker_with_setuid_legacy_escalates() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let owner = Actor::User(12);
    let caller = Actor::User(42);

    make_fn_with_security(
        &shamir,
        "invoker_suid_fn",
        owner.clone(),
        true,
        Security::Invoker,
    )
    .await;

    let effective = shamir.effective_fn_actor("invoker_suid_fn", &caller).await;
    assert_eq!(
        effective, owner,
        "invoker + legacy setuid bit must still escalate (backward compat)"
    );
}

// ============================================================================
// #541 — missing `owner` field on a present record must not escalate
// ============================================================================
//
// `ResourceMeta::from_record` defaults a MISSING `owner` field to
// `Actor::System` — correct for every other caller of `from_record`, but
// wrong for `effective_fn_actor`'s escalation decision: a Definer (or
// Invoker+setuid) function whose record loads but lacks an `owner` field
// must NOT silently escalate the caller to System.

/// Test helper: persist a function catalogue entry via
/// `save_function_meta_record` (the raw-record seam, bypassing
/// `ResourceMeta::inject_into`) so the resulting record can omit the
/// `owner` field entirely — modeling a legacy or partially-written
/// catalogue row. `mode`/`group` are still injected explicitly since the
/// fix under test only concerns the `owner` field.
async fn make_fn_without_owner_field(
    shamir: &ShamirDb,
    name: &str,
    setuid: bool,
    security: shamir_engine::function::Security,
) {
    use base64::Engine;
    use shamir_types::types::common::new_map;
    use shamir_types::types::value::QueryValue;

    let wasm_b64 = base64::engine::general_purpose::STANDARD.encode(b"\x00asm\x01\x00\x00\x00");
    let mut fn_rec_map = new_map();
    fn_rec_map.insert("name".to_string(), QueryValue::Str(name.to_string()));
    fn_rec_map.insert("wasm_b64".to_string(), QueryValue::Str(wasm_b64));
    // Deliberately NO "owner" key — models a legacy/corrupted record.
    fn_rec_map.insert("group".to_string(), QueryValue::Null);
    let mode = if setuid {
        Mode::with_setuid(0o755, true)
    } else {
        0o755
    };
    fn_rec_map.insert("mode".to_string(), QueryValue::Int(mode as i64));
    let mut fn_rec = QueryValue::Map(fn_rec_map);
    shamir_engine::function::FunctionMeta::new(
        shamir_engine::function::Visibility::Private,
        security,
        Vec::new(),
        Vec::new(),
    )
    .inject_into(&mut fn_rec);

    shamir
        .system_store()
        .save_function_meta_record(name, &fn_rec)
        .await
        .unwrap();
}

/// RED before the fix / GREEN after: a `Security::Definer` function whose
/// record loads but has NO `owner` field must return the ORIGINAL CALLER,
/// never `Actor::System` via `ResourceMeta::from_record`'s default.
#[tokio::test]
async fn effective_fn_actor_definer_missing_owner_returns_caller_not_system() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let caller = Actor::User(42);

    make_fn_without_owner_field(&shamir, "definer_no_owner_fn", false, Security::Definer).await;

    let effective = shamir
        .effective_fn_actor("definer_no_owner_fn", &caller)
        .await;
    assert_eq!(
        effective, caller,
        "missing owner field on a Definer function must fail closed to the caller"
    );
    assert_ne!(
        effective,
        Actor::System,
        "must never escalate to System via from_record's default-open owner"
    );
}

/// Same gap, `Invoker` + legacy setuid bit arm: a record with no `owner`
/// field must not escalate even though the setuid bit is set.
#[tokio::test]
async fn effective_fn_actor_invoker_setuid_missing_owner_returns_caller_not_system() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let caller = Actor::User(43);

    make_fn_without_owner_field(&shamir, "invoker_suid_no_owner_fn", true, Security::Invoker).await;

    let effective = shamir
        .effective_fn_actor("invoker_suid_no_owner_fn", &caller)
        .await;
    assert_eq!(
        effective, caller,
        "missing owner field on an Invoker+setuid function must fail closed to the caller"
    );
    assert_ne!(
        effective,
        Actor::System,
        "must never escalate to System via from_record's default-open owner"
    );
}

/// Sibling test: a function record that DOES carry an explicit
/// `owner: 0` (System) field must still legitimately escalate Definer
/// callers to System — proving the fix distinguishes "explicitly System"
/// from "absent", rather than failing closed on System ownership too.
#[tokio::test]
async fn effective_fn_actor_definer_explicit_system_owner_still_escalates() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let caller = Actor::User(44);

    make_fn_with_security(
        &shamir,
        "definer_explicit_system_fn",
        Actor::System,
        false,
        Security::Definer,
    )
    .await;

    let effective = shamir
        .effective_fn_actor("definer_explicit_system_fn", &caller)
        .await;
    assert_eq!(
        effective,
        Actor::System,
        "an explicit owner=System field on a Definer function must still escalate"
    );
}
