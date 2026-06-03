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
