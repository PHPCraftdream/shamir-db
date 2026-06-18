use crate::access::{
    action_perm, authorize, class_of, permits, principal_id, Action, Actor, Mode, Perm, PermClass,
    ResourceMeta, ResourcePath, OWNER_SYSTEM,
};
use crate::mpack;

#[test]
fn system_is_default() {
    assert_eq!(Actor::default(), Actor::System);
}

#[test]
fn authorize_transparent_for_all_variants() {
    for path in [
        ResourcePath::Root,
        ResourcePath::database("d"),
        ResourcePath::store("d", "s"),
        ResourcePath::table("d", "s", "t"),
        ResourcePath::record("d", "s", "t", "k"),
        ResourcePath::index("d", "s", "t", "i"),
        ResourcePath::FunctionNamespace,
        ResourcePath::function_folder(vec!["reports".to_string()]),
        ResourcePath::function("f"),
        ResourcePath::user("u"),
        ResourcePath::group("g"),
    ] {
        for action in [
            Action::Read,
            Action::Write,
            Action::Create,
            Action::Delete,
            Action::Execute,
            Action::List,
            Action::Manage,
        ] {
            assert!(authorize(&Actor::System, &path, action).is_ok());
        }
    }
}

#[test]
fn parent_walks_to_root() {
    // record → table → store → database → root → None
    let rec = ResourcePath::record("d", "s", "t", "k");
    let table = rec.parent().unwrap();
    assert_eq!(table, ResourcePath::table("d", "s", "t"));
    let store = table.parent().unwrap();
    assert_eq!(store, ResourcePath::store("d", "s"));
    let db = store.parent().unwrap();
    assert_eq!(db, ResourcePath::database("d"));
    let root = db.parent().unwrap();
    assert_eq!(root, ResourcePath::Root);
    assert_eq!(root.parent(), None);
}

#[test]
fn index_inherits_table_as_parent() {
    let idx = ResourcePath::index("d", "s", "t", "i");
    assert_eq!(idx.parent().unwrap(), ResourcePath::table("d", "s", "t"));
}

#[test]
fn function_parent_is_namespace() {
    assert_eq!(
        ResourcePath::function("f").parent().unwrap(),
        ResourcePath::FunctionNamespace
    );
    assert_eq!(
        ResourcePath::FunctionNamespace.parent().unwrap(),
        ResourcePath::Root
    );
}

#[test]
fn ancestors_nearest_first_to_root() {
    let rec = ResourcePath::record("d", "s", "t", "k");
    assert_eq!(
        rec.ancestors(),
        vec![
            ResourcePath::table("d", "s", "t"),
            ResourcePath::store("d", "s"),
            ResourcePath::database("d"),
            ResourcePath::Root,
        ]
    );
}

#[test]
fn user_and_group_under_root() {
    assert_eq!(
        ResourcePath::user("u").parent().unwrap(),
        ResourcePath::Root
    );
    assert_eq!(
        ResourcePath::group("g").parent().unwrap(),
        ResourcePath::Root
    );
}

// ========================================================================
// ResourceMeta + Mode tests
// ========================================================================

#[test]
fn open_default_is_system_owned_mode_777() {
    let open = ResourceMeta::open();
    assert_eq!(open.owner, Actor::System);
    assert!(open.group.is_none());
    assert_eq!(open.mode, 0o777);
    assert_eq!(ResourceMeta::default(), open);
}

#[test]
fn actor_owner_round_trip() {
    assert_eq!(Actor::System.to_owner_id(), OWNER_SYSTEM);
    assert_eq!(Actor::from_owner_id(OWNER_SYSTEM), Actor::System);
    assert_eq!(Actor::User(42).to_owner_id(), 42);
    assert_eq!(Actor::from_owner_id(42), Actor::User(42));
}

#[test]
fn principal_id_is_deterministic_and_distinct() {
    // Stable across calls for the same name.
    assert_eq!(principal_id("alice"), principal_id("alice"));
    // Distinct names hash apart (no trivial collision for these).
    assert_ne!(principal_id("alice"), principal_id("bob"));
}

#[test]
fn principal_id_always_fits_i64() {
    // The catalogue stores ids as i64; every principal id must be
    // <= i64::MAX so it survives the wire encoding→InnerValue→msgpack round-trip
    // (the root cause of the empty group-member bug in HIGH-7).
    for name in [
        "",
        "a",
        "admin",
        "alice",
        "bob",
        "Σίσυφος",
        "очень-длинное-имя-пользователя-1234567890",
    ] {
        assert!(
            principal_id(name) <= i64::MAX as u64,
            "principal_id({name:?}) overflowed i64"
        );
    }
}

#[test]
fn principal_id_round_trips_through_actor_owner_id() {
    // A user id derived from a name must decode back to the same
    // `Actor::User`, never aliasing the reserved System id.
    let id = principal_id("alice");
    assert_ne!(id, OWNER_SYSTEM);
    assert_eq!(Actor::from_owner_id(id), Actor::User(id));
}

#[test]
fn mode_from_rwx_combinations() {
    assert_eq!(Mode::from_rwx(true, true, true), 0o777);
    assert_eq!(Mode::from_rwx(true, false, false), 0o700);
    assert_eq!(Mode::from_rwx(false, true, false), 0o070);
    assert_eq!(Mode::from_rwx(false, false, true), 0o007);
    assert_eq!(Mode::from_rwx(false, false, false), 0o000);
}

#[test]
fn mode_is_set_checks() {
    // 0o770 = rwxrwx---
    let mode = 0o770;
    assert!(Mode::is_set(mode, PermClass::Owner, Perm::Read));
    assert!(Mode::is_set(mode, PermClass::Owner, Perm::Write));
    assert!(Mode::is_set(mode, PermClass::Owner, Perm::Execute));
    assert!(Mode::is_set(mode, PermClass::Group, Perm::Read));
    assert!(Mode::is_set(mode, PermClass::Group, Perm::Write));
    assert!(Mode::is_set(mode, PermClass::Group, Perm::Execute));
    assert!(!Mode::is_set(mode, PermClass::Other, Perm::Read));
    assert!(!Mode::is_set(mode, PermClass::Other, Perm::Write));
    assert!(!Mode::is_set(mode, PermClass::Other, Perm::Execute));
    // 0o750 = rwxr-x---
    let mode2 = 0o750;
    assert!(!Mode::is_set(mode2, PermClass::Group, Perm::Write));
    assert!(Mode::is_set(mode2, PermClass::Group, Perm::Execute));
}

#[test]
fn setuid_flag_accessor() {
    let mode = 0o777;
    assert!(!Mode::is_setuid(mode));
    let mode_suid = Mode::with_setuid(mode, true);
    assert!(Mode::is_setuid(mode_suid));
    assert_eq!(mode_suid & 0o777, 0o777);
    let cleared = Mode::with_setuid(mode_suid, false);
    assert!(!Mode::is_setuid(cleared));
    assert_eq!(cleared, 0o777);
}

#[test]
fn inject_into_and_from_record_round_trip() {
    let meta = ResourceMeta {
        owner: Actor::User(10),
        group: Some(5),
        mode: 0o750,
    };
    let mut rec = mpack!({"name": "test"});
    meta.inject_into(&mut rec);
    assert_eq!(rec.get("owner").and_then(|v| v.as_u64()), Some(10));
    assert_eq!(rec.get("group").and_then(|v| v.as_u64()), Some(5));
    assert_eq!(rec.get("mode").and_then(|v| v.as_u64()), Some(0o750));

    let loaded = ResourceMeta::from_record(&rec);
    assert_eq!(loaded.owner, Actor::User(10));
    assert_eq!(loaded.group, Some(5));
    assert_eq!(loaded.mode, 0o750);
}

#[test]
fn from_record_backward_compat_returns_open() {
    let rec = mpack!({"name": "legacy"});
    let loaded = ResourceMeta::from_record(&rec);
    assert_eq!(loaded, ResourceMeta::open());
}

#[test]
fn from_record_null_group_is_none() {
    use crate::types::common::new_map;
    use crate::types::value::QueryValue;
    let mut m = new_map();
    m.insert("name".to_string(), QueryValue::Str("test".to_string()));
    m.insert("owner".to_string(), QueryValue::Int(42));
    m.insert("group".to_string(), QueryValue::Null);
    m.insert("mode".to_string(), QueryValue::Int(0o644));
    let rec = QueryValue::Map(m);
    let loaded = ResourceMeta::from_record(&rec);
    assert_eq!(loaded.owner, Actor::User(42));
    assert!(loaded.group.is_none());
    assert_eq!(loaded.mode, 0o644);
}

// ========================================================================
// permits / class_of tests (P4)
// ========================================================================

#[test]
fn action_perm_mapping() {
    assert_eq!(action_perm(Action::Read), Some(Perm::Read));
    assert_eq!(action_perm(Action::List), Some(Perm::Read));
    assert_eq!(action_perm(Action::Write), Some(Perm::Write));
    assert_eq!(action_perm(Action::Create), Some(Perm::Write));
    assert_eq!(action_perm(Action::Delete), Some(Perm::Write));
    assert_eq!(action_perm(Action::Execute), Some(Perm::Execute));
    assert_eq!(action_perm(Action::Manage), None);
}

#[test]
fn class_of_owner_first_match() {
    let meta = ResourceMeta {
        owner: Actor::User(10),
        group: Some(5),
        mode: 0o777,
    };
    // Owner matches even though group is present and in_group is true.
    assert_eq!(class_of(&Actor::User(10), &meta, true), PermClass::Owner);
    // Not the owner but in group.
    assert_eq!(class_of(&Actor::User(20), &meta, true), PermClass::Group);
    // Not the owner, not in group.
    assert_eq!(class_of(&Actor::User(20), &meta, false), PermClass::Other);
    // Group is None — no group class even if in_group is true.
    let no_group = ResourceMeta {
        owner: Actor::User(10),
        group: None,
        mode: 0o777,
    };
    assert_eq!(
        class_of(&Actor::User(20), &no_group, true),
        PermClass::Other
    );
}

#[test]
fn permits_system_bypass() {
    let meta = ResourceMeta {
        owner: Actor::User(99),
        group: None,
        mode: 0o000,
    };
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
            permits(&Actor::System, &meta, action, false),
            "System should bypass for {action}"
        );
    }
}

#[test]
fn permits_owner_class_rwx() {
    let meta = ResourceMeta {
        owner: Actor::User(10),
        group: None,
        mode: 0o700,
    };
    assert!(permits(&Actor::User(10), &meta, Action::Read, false));
    assert!(permits(&Actor::User(10), &meta, Action::Write, false));
    assert!(permits(&Actor::User(10), &meta, Action::Execute, false));
    assert!(permits(&Actor::User(10), &meta, Action::Create, false));
    assert!(permits(&Actor::User(10), &meta, Action::Delete, false));
    assert!(permits(&Actor::User(10), &meta, Action::List, false));
    assert!(permits(&Actor::User(10), &meta, Action::Manage, false));
}

#[test]
fn permits_group_class() {
    let meta = ResourceMeta {
        owner: Actor::User(10),
        group: Some(5),
        mode: 0o070,
    };
    // In group → allowed (group bits are rwx).
    assert!(permits(&Actor::User(20), &meta, Action::Read, true));
    assert!(permits(&Actor::User(20), &meta, Action::Write, true));
    assert!(permits(&Actor::User(20), &meta, Action::Execute, true));
    // Not in group → denied (owner has rwx but actor is not owner;
    // other bits are 0).
    assert!(!permits(&Actor::User(20), &meta, Action::Read, false));
    assert!(!permits(&Actor::User(20), &meta, Action::Write, false));
}

#[test]
fn permits_other_class() {
    let meta = ResourceMeta {
        owner: Actor::User(10),
        group: None,
        mode: 0o007,
    };
    assert!(permits(&Actor::User(20), &meta, Action::Read, false));
    assert!(permits(&Actor::User(20), &meta, Action::Write, false));
    assert!(permits(&Actor::User(20), &meta, Action::Execute, false));
}

#[test]
fn permits_manage_owner_only() {
    let meta = ResourceMeta {
        owner: Actor::User(10),
        group: Some(5),
        mode: 0o777,
    };
    // Owner can manage.
    assert!(permits(&Actor::User(10), &meta, Action::Manage, false));
    // Non-owner cannot manage, even with full mode bits.
    assert!(!permits(&Actor::User(20), &meta, Action::Manage, true));
    assert!(!permits(&Actor::User(20), &meta, Action::Manage, false));
}

#[test]
fn permits_first_match_wins_owner_denied() {
    // Owner bits are 0, but other bits are rwx. POSIX first-match:
    // actor IS the owner → Owner class → owner bits (0) → denied.
    let meta = ResourceMeta {
        owner: Actor::User(10),
        group: None,
        mode: 0o007,
    };
    assert!(
        !permits(&Actor::User(10), &meta, Action::Read, false),
        "owner should be denied when owner bits are 0 despite other=rwx"
    );
}

#[test]
fn permits_open_mode_allows_everyone() {
    let meta = ResourceMeta::open(); // mode 0o777
    assert!(permits(&Actor::User(99), &meta, Action::Read, false));
    assert!(permits(&Actor::User(99), &meta, Action::Write, false));
    assert!(permits(&Actor::User(99), &meta, Action::Execute, false));
}

// ========================================================================
// FunctionFolder tests (#118)
// ========================================================================

#[test]
fn function_folder_parent_chain_for_folder_qualified_function() {
    // reports/daily/orders → folder ["reports","daily"] → ["reports"] → FunctionNamespace → Root
    let func = ResourcePath::function("reports/daily/orders");
    let folder2 = func.parent().unwrap();
    assert_eq!(
        folder2,
        ResourcePath::function_folder(vec!["reports".to_string(), "daily".to_string(),])
    );
    let folder1 = folder2.parent().unwrap();
    assert_eq!(
        folder1,
        ResourcePath::function_folder(vec!["reports".to_string()])
    );
    let ns = folder1.parent().unwrap();
    assert_eq!(ns, ResourcePath::FunctionNamespace);
    let root = ns.parent().unwrap();
    assert_eq!(root, ResourcePath::Root);
    assert_eq!(root.parent(), None);
}

#[test]
fn single_segment_function_parent_is_namespace() {
    // A function without `/` has FunctionNamespace as parent (unchanged).
    let func = ResourcePath::function("my_fn");
    assert_eq!(func.parent().unwrap(), ResourcePath::FunctionNamespace);
}

#[test]
fn function_folder_ancestors_order() {
    let func = ResourcePath::function("reports/daily/orders");
    let ancestors = func.ancestors();
    assert_eq!(
        ancestors,
        vec![
            ResourcePath::function_folder(vec!["reports".to_string(), "daily".to_string(),]),
            ResourcePath::function_folder(vec!["reports".to_string()]),
            ResourcePath::FunctionNamespace,
            ResourcePath::Root,
        ]
    );
}

#[test]
fn function_folder_display() {
    let folder = ResourcePath::function_folder(vec!["reports".to_string(), "daily".to_string()]);
    assert_eq!(folder.to_string(), "fn://reports/daily/");

    let single = ResourcePath::function_folder(vec!["utils".to_string()]);
    assert_eq!(single.to_string(), "fn://utils/");
}

#[test]
fn function_folder_constructor() {
    let folder = ResourcePath::function_folder(vec!["a".to_string(), "b".to_string()]);
    assert_eq!(
        folder,
        ResourcePath::FunctionFolder {
            path: vec!["a".to_string(), "b".to_string()],
        }
    );
}

#[test]
fn empty_function_folder_parent_is_namespace() {
    let folder = ResourcePath::function_folder(vec![]);
    assert_eq!(folder.parent().unwrap(), ResourcePath::FunctionNamespace);
}
