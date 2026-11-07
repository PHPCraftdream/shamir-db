use crate::core::interner::{
    Interner, InternerError, InternerKey, InternerQuota, InternerQuotaKind, TouchInd, UserKey,
};
use std::sync::{Arc, Barrier};
use std::thread;

fn quota(max_names: u64, max_bytes: u64) -> InternerQuota {
    InternerQuota {
        max_names,
        max_bytes,
    }
}

#[test]
fn rv_types_2eb39dfd_boundaries_do_not_advance_rejected_id() {
    let interner = Interner::with_quota(Vec::new(), quota(2, 3)).unwrap();
    interner.touch_ind("é").unwrap();
    interner.touch_ind("a").unwrap();
    assert!(interner.touch_ind("a").is_ok());
    let generation = interner.generation();
    assert_eq!(
        interner.touch_ind("b"),
        Err(InternerError::QuotaExceeded {
            label: String::new(),
            kind: InternerQuotaKind::Names,
            limit: 2
        })
    );
    assert_eq!(interner.generation(), generation);
    assert_eq!(interner.quota_usage(), (2, 3));

    let bytes = Interner::with_quota(Vec::new(), quota(4, 2)).unwrap();
    bytes.touch_ind("é").unwrap();
    let generation = bytes.generation();
    assert_eq!(
        bytes.touch_ind("x"),
        Err(InternerError::QuotaExceeded {
            label: String::new(),
            kind: InternerQuotaKind::Bytes,
            limit: 2
        })
    );
    assert_eq!(bytes.generation(), generation);
}

#[test]
fn rv_types_2eb39dfd_quota_error_display_names_env_and_label() {
    let interner =
        Interner::with_labeled_quota(Vec::new(), quota(1, 64), "repo-a".to_string()).unwrap();
    let err = interner.touch_ind("first").unwrap();
    assert!(err.is_new());
    let err = interner.touch_ind("second").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("SHAMIR_INTERNER_MAX_NAMES"),
        "message must name the env knobs: {msg}"
    );
    assert!(
        msg.contains("repo-a"),
        "labeled interner must name the repo: {msg}"
    );
}

#[test]
fn rv_types_2eb39dfd_same_name_racers_refund_reservations() {
    const THREADS: usize = 8;
    let interner = Arc::new(
        Interner::with_quota(Vec::new(), quota(THREADS as u64, (THREADS * 4) as u64)).unwrap(),
    );
    let reserved = Arc::new(Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let interner = Arc::clone(&interner);
            let reserved = Arc::clone(&reserved);
            thread::spawn(move || {
                interner.touch_ind_with_test_hook("same", || {
                    reserved.wait();
                })
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Ok(TouchInd::New(_))))
            .count(),
        1
    );
    let ids: Vec<_> = results
        .iter()
        .map(|r| r.as_ref().unwrap().key().id())
        .collect();
    assert!(ids.iter().all(|id| *id == ids[0]));
    assert_eq!(interner.quota_usage(), (1, 4));
}

#[test]
fn rv_types_2eb39dfd_concurrent_distinct_admission_is_exactly_k() {
    const THREADS: usize = 14;
    const K: u64 = 5;
    let interner = Arc::new(Interner::with_quota(Vec::new(), quota(K, 1024)).unwrap());
    let barrier = Arc::new(Barrier::new(THREADS));
    let handles: Vec<_> = (0..THREADS)
        .map(|n| {
            let interner = Arc::clone(&interner);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                interner.touch_ind(format!("field-{n}"))
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Ok(TouchInd::New(_))))
            .count(),
        K as usize
    );
    assert_eq!(interner.quota_usage().0, K);
}

#[test]
fn rv_types_2eb39dfd_hydration_and_touch_with_id_count_over_quota() {
    let initial = vec![
        (InternerKey::new(1), UserKey::from_str("seed")),
        (InternerKey::new(1), UserKey::from_str("seed")),
    ];
    let interner = Interner::with_quota(initial, quota(1, 1)).unwrap();
    assert_eq!(interner.quota_usage(), (1, 4));
    interner.touch_with_id("replay", 3).unwrap();
    assert_eq!(interner.quota_usage(), (2, 10));
    assert!(interner.touch_ind("refused").is_err());
}

#[test]
fn rv_types_2eb39dfd_scratch_touches_are_counted() {
    let interner = Interner::new();
    interner.touch_ind("scratch").unwrap();
    assert_eq!(interner.quota_usage(), (1, 7));
}

#[test]
fn hydration_rejects_conflicting_pairs_and_accepts_recaptures() {
    let pair = |id, name: &str| (InternerKey::new(id), UserKey::from_str(name));
    assert!(matches!(
        Interner::with_state(vec![pair(1, "a"), pair(1, "b")]),
        Err(InternerError::IdConflict { .. })
    ));
    assert!(matches!(
        Interner::with_state(vec![pair(1, "a"), pair(2, "a")]),
        Err(InternerError::NameConflict { .. })
    ));
    assert!(matches!(
        Interner::with_state(vec![pair(0, "a")]),
        Err(InternerError::ReservedId)
    ));
    let interner = Interner::with_state(vec![pair(1, "a"), pair(1, "a"), pair(3, "c")]).unwrap();
    assert_eq!(interner.quota_usage(), (2, 2));
    assert_eq!(interner.get_str(&InternerKey::new(1)).as_deref(), Some("a"));
    assert_eq!(interner.entries_after(0).1, 1);
}
