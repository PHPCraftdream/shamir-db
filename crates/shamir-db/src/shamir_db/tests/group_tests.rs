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

use crate::query::admin::GroupRef;
use crate::shamir_db::ShamirDb;
use crate::DbError;
use shamir_types::access::{Actor, ResourceMeta, ResourcePath};
use std::sync::Arc;

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
        .save_group(5, "preexisting", &[], shamir_types::access::OWNER_SYSTEM)
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

// ============================================================================
// #563 — per-group_id RMW lock closes the unlocked read-modify-write
// TOCTOU on group records (add/remove-member, set-owner, rename).
// ============================================================================
//
// Before this fix, `add_group_member_as` / `remove_group_member_as` /
// `set_resource_meta(Group)` / `rename_group_as` each performed an UNLOCKED
// load_group → mutate → save_group sequence. Two concurrent mutations on the
// SAME group_id each independently read the pre-mutation record, applied
// their own single-field change, and wrote back the WHOLE record — so
// whichever `save_group` landed second won outright, silently discarding the
// other call's change (last-writer-wins on the full record, not per-field).
//
// This test forces that interleaving: it releases N distinct-member
// `add_group_member_as` calls PLUS one concurrent chown
// (`set_resource_meta(Group)`) simultaneously via a barrier, then reloads the
// record and asserts EVERY member add is reflected AND the owner change
// survived. On the unlocked code the membership list is silently truncated
// (only the last writer's base + its own add survives, never all N); with the
// per-group_id lock the whole sequence serialises and no update is lost.
//
// The `multi_thread` flavour is deliberate: real OS-thread parallelism across
// the contending tasks maximises the load-before-any-save window so the race
// trips reliably on the unlocked code (a flaky-red reproduction would be
// unacceptable proof). On the fixed code the per-group_id mutex makes the
// outcome deterministic regardless of scheduling.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn group_member_toctou_concurrent_mutations_lose_no_update() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let group_name = "race_group";
    let gid = shamir.create_group(group_name).await.unwrap();

    const N: usize = 48;
    let new_owner = Actor::User(4242);

    // Barrier of size N+1 (N member-add tasks + the chown task). The spawned
    // tasks self-release: once all N+1 have arrived the barrier fires and they
    // all proceed at once, maximising initial contention on the shared group
    // record. The main task does NOT participate in the barrier — it just
    // awaits the join handles below.
    let barrier = Arc::new(tokio::sync::Barrier::new(N + 1));

    let mut handles = Vec::with_capacity(N + 1);
    for i in 0..N as u64 {
        let shamir = shamir.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            // `Actor::System` bypasses the Manage gate, so this exercises the
            // bare (previously unlocked) read-modify-write path directly.
            shamir
                .add_group_member_as(gid, 1000 + i, &Actor::System)
                .await
        }));
    }
    // One concurrent chown on the SAME group via the Group arm of
    // `set_resource_meta` (the `set_group_owner` read-modify-write path).
    let new_owner_for_task = new_owner.clone();
    {
        let shamir = shamir.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            shamir
                .set_resource_meta(
                    &ResourcePath::group(group_name),
                    &ResourceMeta {
                        owner: new_owner_for_task,
                        group: None,
                        mode: 0o750,
                    },
                )
                .await
        }));
    }

    // The spawned tasks self-release the barrier (size N+1 == N member-adds
    // + 1 chown). Just await their completion.
    for h in handles {
        h.await.unwrap().unwrap();
    }

    // Reload the record and verify NO membership update was lost.
    let mut members = shamir.group_members(gid).await.unwrap();
    members.sort();
    let expected: Vec<u64> = (0..N as u64).map(|i| 1000 + i).collect();
    assert_eq!(
        members,
        expected,
        "all {N} concurrent member adds must survive — a truncated list means \
         the unlocked read-modify-write lost an update (TOCTOU #563); \
         got {} members, expected {N}",
        members.len(),
    );

    // The chown must survive too. On the unlocked code, a racing member add
    // that read owner=System BEFORE the chown could overwrite owner back to
    // System AFTER the chown landed. With the per-group_id lock, once the
    // chown runs every subsequent op reads owner=new_owner and preserves it
    // (member-add threads the existing owner through unchanged), so the final
    // owner is deterministically `new_owner`.
    let meta = shamir
        .resource_meta(&ResourcePath::group(group_name))
        .await
        .unwrap();
    assert_eq!(
        meta.owner, new_owner,
        "concurrent chown must survive — owner reverted to {} means a stale \
         member-mutation read (pre-chown) overwrote the chown (TOCTOU #563)",
        meta.owner,
    );
}

// ============================================================================
// #570 — group-NAME uniqueness across both allocation paths (create + rename).
// ============================================================================
//
// Before this fix `create_group_as` had NO name-uniqueness check at all (a
// complete absence — not a race), and `rename_group_as`'s uniqueness scan was
// protected only by #563's per-`group_id` lock (which serialises mutations on
// the SAME id, not across DIFFERENT ids). The invariant that a group name maps
// to exactly one group — relied on by `resolve_group_id` / `GroupRef::Name` —
// was therefore broken both sequentially (two `create_group("ops")` calls both
// succeeding) and concurrently (a create racing a rename, or two concurrent
// creates, all landing the same name). The fix reuses the existing global
// `group_id_lock` as the single point of serialization across the whole group
// NAME namespace for both create and rename, held from before the uniqueness
// scan through the write.

/// Task #570 — `create_group_as` previously had NO name-uniqueness check at
/// all: two SEQUENTIAL (non-concurrent) `create_group("ops")` calls both
/// succeeded, producing two groups sharing the name "ops". This breaks
/// name-based resolution (`resolve_group_id` / `GroupRef::Name`), which
/// assumes a name maps to exactly one group. No concurrency is needed to
/// reproduce the gap.
#[tokio::test]
async fn create_group_as_rejects_duplicate_name_sequential() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    let _first = shamir.create_group("ops").await.unwrap();

    // A second create with the SAME name must be rejected with KeyExists —
    // a silent second success means two groups share the name and name-based
    // resolution becomes nondeterministic (whichever a linear scan finds
    // first). On the unfixed code this `unwrap_err` panics because the call
    // succeeds outright.
    let err = shamir
        .create_group("ops")
        .await
        .expect_err("duplicate create_group('ops') must be rejected");
    assert!(
        matches!(err, DbError::KeyExists(_)),
        "expected DbError::KeyExists, got {err:?}"
    );

    // Persistence invariant: exactly one group named "ops" survives.
    let groups = shamir.system_store().load_groups().await.unwrap();
    let count = groups
        .iter()
        .filter(|g| g["name"].as_str() == Some("ops"))
        .count();
    assert_eq!(
        count, 1,
        "exactly one 'ops' group must persist after the duplicate create, \
         got {count}"
    );
}

/// Task #570 — N concurrent `create_group_as` calls all targeting the SAME
/// name must collapse to exactly one success and N-1 `KeyExists` rejections.
/// Before the fix the global `group_id_lock` was held only across the
/// id-counter bump, NOT across any uniqueness check, so N concurrent creates
/// of the same name each passed (there was no check to race) and all N
/// landed — producing N groups sharing a name. The `multi_thread` flavour
/// maximises real OS-level contention so the race trips reliably on the
/// unfixed code; on the fixed code the global lock makes the outcome
/// deterministic regardless of scheduling.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_group_as_concurrent_same_name_only_one_wins() {
    let shamir = ShamirDb::init_memory().await.unwrap();
    const N: usize = 16;
    let name = "dupe";

    // Barrier of size N: all create tasks self-release simultaneously to
    // maximise the contention window. The main task does NOT participate.
    let barrier = Arc::new(tokio::sync::Barrier::new(N));

    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let shamir = shamir.clone();
        let barrier = barrier.clone();
        handles.push(tokio::spawn(async move {
            barrier.wait().await;
            // `Actor::System` bypasses the Manage gate, exercising the bare
            // (previously check-free) allocation path directly.
            shamir.create_group_as(name, &Actor::System).await
        }));
    }

    let mut oks = 0usize;
    let mut keyexists = 0usize;
    for h in handles {
        match h.await.unwrap() {
            Ok(_) => oks += 1,
            Err(DbError::KeyExists(_)) => keyexists += 1,
            Err(e) => panic!("unexpected error from create_group_as: {e:?}"),
        }
    }
    assert_eq!(
        oks, 1,
        "exactly one concurrent create of '{name}' must succeed, got {oks}"
    );
    assert_eq!(
        keyexists,
        N - 1,
        "the remaining {} creates must return KeyExists, got {keyexists}",
        N - 1
    );

    // Persistence invariant: exactly one group named `name` survives.
    let groups = shamir.system_store().load_groups().await.unwrap();
    let count = groups
        .iter()
        .filter(|g| g["name"].as_str() == Some(name))
        .count();
    assert_eq!(
        count, 1,
        "exactly one '{name}' group must persist, got {count}"
    );
}

/// Task #570 — the cross-path race the original review finding specifically
/// called out: a `create_group_as("beta")` racing a
/// `rename_group_as(<other existing group>, "beta")` on the SAME target
/// name. Before the fix the rename's uniqueness scan was protected only by
/// #563's per-`group_id` lock (serialising mutations on the SAME id, not
/// across ids) and the create had no uniqueness check at all — so both could
/// land a group named "beta". The shared global `group_id_lock` now covers
/// the entire group-NAME namespace across both allocation paths, so exactly
/// one of the two wins and the loser gets a clear `KeyExists` conflict rather
/// than a silent double-assignment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_create_vs_rename_same_name_only_one_wins() {
    let shamir = ShamirDb::init_memory().await.unwrap();

    // Two pre-existing groups: "alpha" (to be renamed) and a sentinel so the
    // create doesn't run against an empty namespace.
    let alpha = shamir.create_group("alpha").await.unwrap();
    let _sentinel = shamir.create_group("sentinel").await.unwrap();

    let target = "beta";
    let barrier = Arc::new(tokio::sync::Barrier::new(2));

    // Create task.
    let shamir_create = shamir.clone();
    let barrier_create = barrier.clone();
    let create_handle = tokio::spawn(async move {
        barrier_create.wait().await;
        shamir_create.create_group_as(target, &Actor::System).await
    });

    // Rename task — renames "alpha" → "beta".
    let shamir_rename = shamir.clone();
    let barrier_rename = barrier.clone();
    let rename_handle = tokio::spawn(async move {
        barrier_rename.wait().await;
        shamir_rename
            .rename_group_as(&GroupRef::Id { id: alpha }, target, &Actor::System)
            .await
    });

    let create_res = create_handle.await.unwrap();
    let rename_res = rename_handle.await.unwrap();

    // Exactly one of the two must land the name "beta"; never both, never
    // neither. (A double-success is the #570 cross-path race; a double-failure
    // would be a different regression — one side must win.)
    let create_ok = create_res.is_ok();
    let rename_ok = rename_res.is_ok();
    assert!(
        create_ok ^ rename_ok,
        "exactly one of create/rename must succeed — \
         create_ok={create_ok} rename_ok={rename_ok} \
         (double-assignment or double-failure is the #570 cross-path race)",
    );
    // The loser must surface `KeyExists` (not some other error or a panic).
    if !create_ok {
        assert!(
            matches!(create_res, Err(DbError::KeyExists(_))),
            "losing create must surface KeyExists, got {create_res:?}"
        );
    }
    if !rename_ok {
        assert!(
            matches!(rename_res, Err(DbError::KeyExists(_))),
            "losing rename must surface KeyExists, got {rename_res:?}"
        );
    }

    // Persistence invariant: exactly one group named "beta" survives.
    let groups = shamir.system_store().load_groups().await.unwrap();
    let count = groups
        .iter()
        .filter(|g| g["name"].as_str() == Some(target))
        .count();
    assert_eq!(
        count, 1,
        "exactly one '{target}' group must persist, got {count}"
    );
}
