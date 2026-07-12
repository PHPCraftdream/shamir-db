//! Group-id allocation correctness (audit 3d-D2).
//!
//! `create_group` allocates ids from a `next_group_id` counter. These tests
//! guard the three bugs the audit found: a concurrent read-modify-write race,
//! crash-resurrection (group persisted before the counter bumped), and a
//! `default 1` collision when the counter setting is absent but groups exist.
//!
//! (The companion D1 fix — `function_meta: Arc<DashMap<…>>` so `ShamirDb::clone`
//! is O(1) instead of deep-copying the map — is a compile-time type guarantee,
//! not a runtime behaviour, so it carries no separate test.)

use crate::shamir_db::ShamirDb;
use shamir_types::access::Actor;

/// Sequential `create_group` calls return distinct, monotonically increasing
/// ids.
#[tokio::test]
async fn test_create_group_sequential_distinct_ids() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let id1 = shamir.create_group("alpha").await.unwrap();
    let id2 = shamir.create_group("beta").await.unwrap();
    let id3 = shamir.create_group("gamma").await.unwrap();

    assert!(id1 < id2, "expected id1 ({id1}) < id2 ({id2})");
    assert!(id2 < id3, "expected id2 ({id2}) < id3 ({id3})");
}

/// When the `next_group_id` setting is absent but groups already exist at
/// higher ids, `create_group` seeds past the max existing id — it must NOT
/// return 1 and overwrite an existing group.
#[tokio::test]
async fn test_create_group_seeds_from_max_when_counter_absent() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // Persist a group at id 5 directly, bypassing create_group's counter so
    // the `next_group_id` setting is never written.
    shamir
        .system_store()
        .save_group(5, "preexisting", &[])
        .await
        .unwrap();

    let groups = shamir.system_store().load_groups().await.unwrap();
    assert!(
        groups.iter().any(|g| g["group_id"].as_u64() == Some(5)),
        "precondition: group 5 must exist"
    );

    // Must seed from max(existing) + 1 == 6, not collide at 1.
    let id = shamir.create_group("new_group").await.unwrap();
    assert_eq!(id, 6, "expected id 6 (max_existing + 1), got {id}");

    // The pre-existing group must survive untouched.
    let groups = shamir.system_store().load_groups().await.unwrap();
    let g5 = groups.iter().find(|g| g["group_id"].as_u64() == Some(5));
    assert!(g5.is_some(), "group 5 must not be overwritten");
    assert_eq!(g5.unwrap()["name"].as_str(), Some("preexisting"));
}

/// Two concurrent `create_group` calls on the SAME `ShamirDb` must return
/// distinct ids — the `group_id_lock` serialises the read-modify-write.
#[tokio::test]
async fn test_create_group_concurrent_distinct_ids() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let (r1, r2) = tokio::join!(
        shamir.create_group("concurrent_a"),
        shamir.create_group("concurrent_b"),
    );

    let id1 = r1.expect("first create_group should succeed");
    let id2 = r2.expect("second create_group should succeed");

    assert_ne!(
        id1, id2,
        "concurrent create_group must not return the same id"
    );
}

// ============================================================================
// #546 — Manage(Root) gate self-defense on the `*_as` group-CRUD methods
// ============================================================================
//
// Before this fix, `create_group`/`drop_group`/`rename_group`/
// `add_group_member`/`remove_group_member` did NO authorization check
// themselves — safety relied entirely on every dispatcher handler
// (`admin_access.rs`) pre-calling `authorize_access(Root, Manage)` before
// reaching these methods. The `*_as` variants introduced by this task now
// perform that check inline, so calling them DIRECTLY (bypassing the
// dispatcher) with a non-System, non-Manage actor is still denied — the
// gate is structurally enforced on the method itself, not just by
// convention at the caller.

/// A non-System actor with no Manage rights on Root must be denied by
/// `create_group_as` even when called directly, bypassing the dispatcher.
#[tokio::test]
async fn create_group_as_denies_non_manage_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let stranger = Actor::User(999);

    let err = shamir
        .create_group_as("devs", &stranger)
        .await
        .expect_err("a non-Manage actor must be denied by create_group_as directly");
    let msg = format!("{err}");
    assert!(
        msg.contains("denied") || msg.contains("Denied"),
        "expected an access-denied error, got: {msg}"
    );

    // No group must have been created as a side effect of the denied call.
    let groups = shamir.system_store().load_groups().await.unwrap();
    assert!(
        groups.iter().all(|g| g["name"].as_str() != Some("devs")),
        "denied create_group_as must not create the group"
    );
}

/// `drop_group_as` denies a non-Manage actor directly.
#[tokio::test]
async fn drop_group_as_denies_non_manage_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let gid = shamir.create_group("to_drop").await.unwrap();
    let stranger = Actor::User(999);

    let err = shamir
        .drop_group_as(gid, &stranger)
        .await
        .expect_err("a non-Manage actor must be denied by drop_group_as directly");
    assert!(format!("{err}").to_lowercase().contains("denied"));

    // The group must still exist — the denied call must be a no-op.
    let groups = shamir.system_store().load_groups().await.unwrap();
    assert!(groups.iter().any(|g| g["group_id"].as_u64() == Some(gid)));
}

/// `rename_group_as` denies a non-Manage actor directly.
#[tokio::test]
async fn rename_group_as_denies_non_manage_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let gid = shamir.create_group("original").await.unwrap();
    let stranger = Actor::User(999);

    let err = shamir
        .rename_group_as(
            &crate::query::admin::GroupRef::Id { id: gid },
            "renamed",
            &stranger,
        )
        .await
        .expect_err("a non-Manage actor must be denied by rename_group_as directly");
    assert!(format!("{err}").to_lowercase().contains("denied"));

    // The name must be unchanged.
    let groups = shamir.system_store().load_groups().await.unwrap();
    let g = groups
        .iter()
        .find(|g| g["group_id"].as_u64() == Some(gid))
        .unwrap();
    assert_eq!(g["name"].as_str(), Some("original"));
}

/// `add_group_member_as` denies a non-Manage actor directly.
#[tokio::test]
async fn add_group_member_as_denies_non_manage_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let gid = shamir.create_group("devs").await.unwrap();
    let stranger = Actor::User(999);

    let err = shamir
        .add_group_member_as(gid, 42, &stranger)
        .await
        .expect_err("a non-Manage actor must be denied by add_group_member_as directly");
    assert!(format!("{err}").to_lowercase().contains("denied"));

    // The membership must not have been added.
    assert!(!shamir.user_in_group(42, gid).await.unwrap());
}

/// `remove_group_member_as` denies a non-Manage actor directly.
#[tokio::test]
async fn remove_group_member_as_denies_non_manage_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let gid = shamir.create_group("devs").await.unwrap();
    shamir.add_group_member(gid, 42).await.unwrap();
    let stranger = Actor::User(999);

    let err = shamir
        .remove_group_member_as(gid, 42, &stranger)
        .await
        .expect_err("a non-Manage actor must be denied by remove_group_member_as directly");
    assert!(format!("{err}").to_lowercase().contains("denied"));

    // The membership must survive the denied call.
    assert!(shamir.user_in_group(42, gid).await.unwrap());
}

/// System bypasses the new inline gate on every `*_as` group-CRUD method —
/// composition check: the dispatcher's own pre-check and this method-level
/// check must not double-deny a legitimate System actor.
#[tokio::test]
async fn group_as_methods_still_allow_system_actor() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let gid = shamir
        .create_group_as("sys_devs", &Actor::System)
        .await
        .expect("System must be allowed by create_group_as");
    shamir
        .add_group_member_as(gid, 7, &Actor::System)
        .await
        .expect("System must be allowed by add_group_member_as");
    shamir
        .rename_group_as(
            &crate::query::admin::GroupRef::Id { id: gid },
            "sys_devs_renamed",
            &Actor::System,
        )
        .await
        .expect("System must be allowed by rename_group_as");
    shamir
        .remove_group_member_as(gid, 7, &Actor::System)
        .await
        .expect("System must be allowed by remove_group_member_as");
    shamir
        .drop_group_as(gid, &Actor::System)
        .await
        .expect("System must be allowed by drop_group_as");
}
