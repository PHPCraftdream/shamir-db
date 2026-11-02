use crate::server::token::{
    TokenChannels, TokenId, TokenMirror, TokenMode, TokenRecord, TokenState,
};
use rustc_hash::FxHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier};
fn record(id: u8, owner: u8) -> TokenRecord {
    TokenRecord {
        id: [id; 16],
        owner_user_id: [owner; 16],
        label: "label".into(),
        mac: [3; 32],
        key_id: 1,
        format_version: 1,
        created_at_ns: 1,
        expires_at_ns: 100,
        mode: TokenMode::ReadOnly,
        channels: TokenChannels::Both,
        databases: None,
        roles: vec![],
        revoked_at_ns: None,
    }
}
#[test]
fn insert_get_duplicate_remove_and_owner_cleanup() {
    let mirror = TokenMirror::new();
    assert!(mirror.insert(record(1, 2)));
    assert!(mirror.insert(record(2, 2)));
    assert!(!mirror.insert(record(1, 3)));
    let id = TokenId([1; 16]);
    assert_eq!(mirror.get(&id).unwrap().record().owner_user_id, [2; 16]);
    assert!(mirror.ids_of_owner(&[3; 16]).is_empty());
    assert_eq!(mirror.len_hint(), 2);
    assert!(mirror.remove(&id));
    assert_eq!(mirror.ids_of_owner(&[2; 16]), vec![TokenId([2; 16])]);
    assert!(mirror.remove(&TokenId([2; 16])));
    assert!(mirror.ids_of_owner(&[2; 16]).is_empty());
    assert_eq!(mirror.len_hint(), 0);
    assert!(!mirror.remove(&id));
}
#[test]
fn revocation_is_idempotent_and_post_return_reader_observes_it() {
    let mirror = Arc::new(TokenMirror::new());
    mirror.insert(record(1, 2));
    let id = TokenId([1; 16]);
    let held_state = mirror.get(&id).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let before_publish = Arc::new(Barrier::new(2));
    let after_publish = Arc::new(Barrier::new(2));
    let reader = {
        let held_state = Arc::clone(&held_state);
        let barrier = Arc::clone(&barrier);
        let before_publish = Arc::clone(&before_publish);
        let after_publish = Arc::clone(&after_publish);
        std::thread::spawn(move || {
            barrier.wait();
            let _may_have_seen_live_before_publish = !held_state.is_revoked();
            before_publish.wait();
            after_publish.wait();
            assert!(held_state.is_revoked());
        })
    };
    barrier.wait();
    before_publish.wait();
    assert!(mirror.revoke(&id, 10));
    assert!(!mirror.revoke(&id, 11));
    after_publish.wait();
    reader.join().unwrap();
    assert_eq!(held_state.revoked_at_ns(), Some(10));
}
#[test]
fn concurrent_revocation_readers_never_observe_live_after_first_true() {
    let mirror = Arc::new(TokenMirror::new());
    assert!(mirror.insert(record(1, 2)));
    let held_state = mirror.get(&TokenId([1; 16])).expect("inserted token");
    let start = Arc::new(AtomicBool::new(false));
    let observation_barrier = Arc::new(Barrier::new(5));
    let post_publish = Arc::new(Barrier::new(5));

    std::thread::scope(|scope| {
        let mut readers = Vec::new();
        for _ in 0..4 {
            let held_state = Arc::clone(&held_state);
            let start = Arc::clone(&start);
            let observation_barrier = Arc::clone(&observation_barrier);
            let post_publish = Arc::clone(&post_publish);
            let mirror = Arc::clone(&mirror);
            readers.push(scope.spawn(move || {
                observation_barrier.wait();
                while !start.load(Ordering::Acquire) {
                    std::hint::spin_loop();
                }
                let mut first_revoked_read = None;
                let mut last_state_revoked = false;
                let mut last_indexed_revoked = false;
                let mut monotonic = true;
                for iteration in 0..10_000 {
                    let state_revoked = held_state.is_revoked();
                    let indexed_revoked = mirror
                        .get(&TokenId([1; 16]))
                        .expect("token remains indexed")
                        .is_revoked();
                    monotonic &= !last_state_revoked || state_revoked;
                    monotonic &= !last_indexed_revoked || indexed_revoked;
                    if (state_revoked || indexed_revoked) && first_revoked_read.is_none() {
                        first_revoked_read = Some(iteration);
                    }
                    last_state_revoked = state_revoked;
                    last_indexed_revoked = indexed_revoked;
                }
                post_publish.wait();
                assert!(monotonic);
                assert!(held_state.is_revoked());
                assert!(mirror
                    .get(&TokenId([1; 16]))
                    .expect("token remains indexed")
                    .is_revoked());
                *first_revoked_read.get_or_insert(10_000)
            }));
        }
        observation_barrier.wait();
        start.store(true, Ordering::Release);
        assert!(mirror.revoke(&TokenId([1; 16]), 42));
        post_publish.wait();
        for reader in readers {
            assert!(reader.join().unwrap() <= 10_000);
        }

        // These readers are created only after revoke returns.
        for _ in 0..2 {
            let mirror = Arc::clone(&mirror);
            scope.spawn(move || {
                let state = mirror
                    .get(&TokenId([1; 16]))
                    .expect("token remains indexed");
                assert!(state.is_revoked());
            });
        }
    });
}

#[test]
fn preexisting_zero_revocation_timestamp_is_still_revoked() {
    let mut r = record(1, 2);
    r.revoked_at_ns = Some(0);
    let state = TokenState::new(r);
    assert!(state.is_revoked());
    assert_ne!(state.revoked_at_ns(), Some(0));
}
#[test]
fn revoke_owner_removes_and_counts_only_first_transition() {
    let mirror = TokenMirror::new();
    mirror.insert(record(1, 7));
    mirror.insert(record(2, 7));
    assert_eq!(mirror.revoke_owner(&[7; 16], 20), 2);
    assert_eq!(mirror.revoke_owner(&[7; 16], 21), 0);
}
#[test]
fn simultaneous_different_id_cleanup_preserves_owner_membership() {
    let mirror = Arc::new(TokenMirror::new());
    mirror.insert(record(1, 9));
    let barrier = Arc::new(Barrier::new(3));
    let remove = {
        let m = Arc::clone(&mirror);
        let b = Arc::clone(&barrier);
        std::thread::spawn(move || {
            b.wait();
            m.remove(&TokenId([1; 16]));
        })
    };
    let insert = {
        let m = Arc::clone(&mirror);
        let b = Arc::clone(&barrier);
        std::thread::spawn(move || {
            b.wait();
            m.insert(record(2, 9));
        })
    };
    barrier.wait();
    remove.join().unwrap();
    insert.join().unwrap();
    assert_eq!(mirror.ids_of_owner(&[9; 16]), vec![TokenId([2; 16])]);
}
#[test]
fn removal_reclaims_owner_bucket_and_state_arc_remains_revoked() {
    let mirror = TokenMirror::new();
    let id = TokenId([41; 16]);
    assert!(mirror.insert(record(41, 42)));
    let retained = mirror.get(&id).expect("inserted state");
    assert!(mirror.revoke(&id, 70));
    assert!(mirror.remove(&id));
    assert!(mirror.ids_of_owner(&[42; 16]).is_empty());
    assert!(retained.is_revoked());
    assert_eq!(retained.revoked_at_ns(), Some(70));
    assert!(mirror.insert(record(41, 43)));
    assert_eq!(
        mirror
            .get(&id)
            .expect("re-admitted id")
            .record()
            .owner_user_id,
        [43; 16]
    );
    assert_eq!(mirror.ids_of_owner(&[42; 16]), Vec::<TokenId>::new());
}

#[test]
fn load_all_ignores_duplicates_and_unknown_operations_are_noops() {
    let mirror = TokenMirror::new();
    mirror.load_all([record(1, 2), record(1, 3), record(2, 2)]);
    assert_eq!(mirror.len_hint(), 2);
    let unknown = TokenId([99; 16]);
    assert!(mirror.get(&unknown).is_none());
    assert!(!mirror.revoke(&unknown, 10));
    assert!(!mirror.remove(&unknown));
    let ids = mirror.ids_of_owner(&[2; 16]);
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&TokenId([1; 16])));
    assert!(ids.contains(&TokenId([2; 16])));
}
#[test]
fn revoke_zero_normalizes_timestamp() {
    let mirror = TokenMirror::new();
    let id = TokenId([8; 16]);
    assert!(mirror.insert(record(8, 2)));
    assert!(mirror.revoke(&id, 0));
    assert_eq!(mirror.get(&id).unwrap().revoked_at_ns(), Some(1));
}
#[test]
fn deterministic_colliding_stripes_serialize_remove_and_insert() {
    let mut stripes: [Option<TokenId>; 64] = [None; 64];
    let (first, second) = (0u8..=64)
        .find_map(|value| {
            let id = TokenId([value; 16]);
            let mut hasher = FxHasher::default();
            id.hash(&mut hasher);
            let stripe = hasher.finish() as usize % 64;
            stripes[stripe].replace(id).map(|previous| (previous, id))
        })
        .expect("65 IDs must collide across 64 stripes");
    assert_ne!(first, second);
    let mirror = Arc::new(TokenMirror::new());
    let owner = [9; 16];
    assert!(mirror.insert(TokenRecord {
        id: first.0,
        ..record(first.0[0], owner[0])
    }));
    let barrier = Arc::new(Barrier::new(3));
    let remove = {
        let mirror = Arc::clone(&mirror);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            mirror.remove(&first)
        })
    };
    let insert = {
        let mirror = Arc::clone(&mirror);
        let barrier = Arc::clone(&barrier);
        std::thread::spawn(move || {
            barrier.wait();
            mirror.insert(TokenRecord {
                id: second.0,
                ..record(second.0[0], owner[0])
            })
        })
    };
    barrier.wait();
    assert!(remove.join().unwrap());
    assert!(insert.join().unwrap());
    assert!(mirror.get(&second).is_some());
    assert!(mirror.get(&first).is_none());
    assert_eq!(mirror.ids_of_owner(&owner), vec![second]);
    assert_eq!(mirror.len_hint(), 1);
    assert_eq!(mirror.retained_gate_cells_hint(), 64);
    assert!(mirror.remove(&second));
    for iteration in 0u16..512 {
        let mut raw_id = [0u8; 16];
        raw_id[..2].copy_from_slice(&iteration.to_le_bytes());
        let id = TokenId(raw_id);
        let churn_record = TokenRecord {
            id: raw_id,
            ..record(raw_id[0], owner[0])
        };
        assert!(mirror.insert(churn_record));
        assert!(mirror.remove(&id));
    }
    assert_eq!(mirror.retained_gate_cells_hint(), 64);
}
#[test]
fn same_id_insert_remove_race_has_no_stale_index() {
    let mirror = Arc::new(TokenMirror::new());
    let id = TokenId([1; 16]);
    for _ in 0..100 {
        let barrier = Arc::new(Barrier::new(3));
        let insert = {
            let m = Arc::clone(&mirror);
            let b = Arc::clone(&barrier);
            std::thread::spawn(move || {
                b.wait();
                m.insert(record(1, 4));
            })
        };
        let remove = {
            let m = Arc::clone(&mirror);
            let b = Arc::clone(&barrier);
            std::thread::spawn(move || {
                b.wait();
                m.remove(&id);
            })
        };
        barrier.wait();
        insert.join().unwrap();
        remove.join().unwrap();
        assert_eq!(
            mirror.ids_of_owner(&[4; 16]).contains(&id),
            mirror.get(&id).is_some()
        );
        if mirror.get(&id).is_some() {
            mirror.remove(&id);
        }
    }
}
