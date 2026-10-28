//! Regression oracles for interner publication and allocation cards.

use crate::core::interner::{Interner, InternerError, InternerKey, InternerQuota};
use std::sync::Arc;
use std::thread;

#[test]
fn rv_types_056200a9_generation_tracks_explicit_hole_publication() {
    let interner = Interner::new();
    interner.touch_with_id("upper", 2).unwrap();
    let sampled = interner.generation();
    let (first_batch, high_water) = interner.entries_after(0);
    assert_eq!(first_batch.len(), 1);
    assert_eq!(high_water, 0);
    interner.touch_with_id("lower-hole", 1).unwrap();
    assert!(interner.generation() > sampled);
    let (second_batch, second_high_water) = interner.entries_after(high_water);
    assert_eq!(second_batch.len(), 2);
    assert_eq!(second_high_water, 2);
    assert_eq!(
        interner.get_str(&InternerKey::new(1)).as_deref(),
        Some("lower-hole")
    );
}

#[test]
fn rv_types_1eba1d8a_forward_visibility_never_precedes_reverse_publication() {
    let interner = Interner::new();
    let generation = interner.generation();
    let mut seen = None;
    interner
        .touch_ind_with_test_hooks(
            "auto",
            || {},
            |_| {},
            |id| {
                seen = Some((
                    interner.get_str(&InternerKey::new(id)),
                    interner.generation(),
                ));
            },
        )
        .unwrap();
    assert_eq!(seen, Some((Some(Arc::<str>::from("auto")), generation)));

    let mut explicit_seen = None;
    interner
        .touch_with_id_with_test_hook("explicit", 9, |id| {
            explicit_seen = Some((
                interner.get_str(&InternerKey::new(id)),
                interner.generation(),
            ));
        })
        .unwrap();
    assert_eq!(
        explicit_seen,
        Some((Some(Arc::<str>::from("explicit")), generation + 1))
    );
}

#[test]
fn rv_types_3cfa3fb6_explicit_and_automatic_ids_do_not_collide() {
    let interner = Arc::new(Interner::new());
    let worker_interner = Arc::clone(&interner);
    let automatic = thread::spawn(move || {
        let mut late = None;
        let mut early = None;
        let key = worker_interner
            .touch_ind_with_test_hooks(
                "automatic",
                || {},
                |id| {
                    if worker_interner.publication_lock_held_for_test() {
                        let competitor = Arc::clone(&worker_interner);
                        late = Some(thread::spawn(move || {
                            competitor.touch_with_id("explicit", id)
                        }));
                    } else {
                        early = Some(worker_interner.touch_with_id("explicit", id));
                    }
                },
                |_| {},
            )
            .unwrap()
            .into_key();
        (key, late, early)
    });
    let (key, late, early) = automatic.join().unwrap();
    assert_eq!(interner.get_str(&key).as_deref(), Some("automatic"));
    let result = late.map(|handle| handle.join().unwrap()).or(early).unwrap();
    assert!(matches!(result, Err(InternerError::IdConflict { id, .. }) if id == key.id()));
}

#[test]
fn rv_types_5dd242f6_errors_are_typed_and_rejected_collision_leaves_quota() {
    let interner = Interner::with_quota(
        Vec::new(),
        InternerQuota {
            max_names: 2,
            max_bytes: 32,
        },
    );
    interner.touch_with_id("fixed", 1).unwrap();
    assert!(matches!(
        interner.touch_with_id("other", 1),
        Err(InternerError::IdConflict { .. })
    ));
    assert_eq!(interner.quota_usage(), (1, 5));
    let touched = interner.touch_ind("free").unwrap();
    assert_eq!(interner.get_str(touched.key()).as_deref(), Some("free"));
}

#[test]
fn rv_types_86e037b2_publication_paths_preserve_reverse_consistency() {
    let interner = Interner::new();
    let auto = interner.touch_ind("auto").unwrap().into_key();
    interner.touch_with_id("explicit", 20).unwrap();
    assert_eq!(interner.get_str(&auto).as_deref(), Some("auto"));
    assert_eq!(
        interner.get_str(&InternerKey::new(20)).as_deref(),
        Some("explicit")
    );
}

#[test]
fn rv_types_reverse_set_failures_refund_and_do_not_publish() {
    let interner = Interner::with_quota(
        Vec::new(),
        InternerQuota {
            max_names: 4,
            max_bytes: 32,
        },
    );
    let generation = interner.generation();
    Interner::fail_next_reverse_set_for_test();
    assert!(matches!(
        interner.touch_ind("x"),
        Err(InternerError::IdCollision { .. })
    ));
    assert_eq!(interner.quota_usage(), (0, 0));
    assert_eq!(interner.get_ind("x"), None);
    assert_eq!(interner.generation(), generation);
    Interner::fail_next_reverse_set_for_test();
    assert!(matches!(
        interner.touch_with_id("y", 5),
        Err(InternerError::IdCollision { .. })
    ));
    assert_eq!(interner.quota_usage(), (0, 0));
    assert_eq!(interner.get_ind("y"), None);
    assert_eq!(interner.generation(), generation);
    assert_eq!(interner.touch_ind("x").unwrap().key().id(), 1);
}

#[test]
fn rv_types_automatic_id_overflow_refunds_quota() {
    let interner = Interner::with_quota(
        Vec::new(),
        InternerQuota {
            max_names: 1,
            max_bytes: 8,
        },
    );
    interner.set_current_id_for_test(u64::MAX);
    let generation = interner.generation();
    assert!(matches!(
        interner.touch_ind("x"),
        Err(InternerError::IdCollision { id: u64::MAX })
    ));
    assert_eq!(interner.quota_usage(), (0, 0));
    assert_eq!(interner.generation(), generation);
}
