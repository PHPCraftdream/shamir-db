//! Concurrent growth, gap semantics, and `entries_after` capacity tests.

use crate::core::interner::{Interner, InternerError, InternerKey, InternerQuota, UserKey};
use shamir_collections::TFxSet;
use std::sync::{Arc, Barrier};
use std::thread;

#[test]
fn rv_types_65dde5b8_generation_tracks_publication() {
    let interner = Interner::new();
    let initial = interner.generation();
    interner.touch_ind("generation-contract").unwrap();
    assert!(interner.generation() > initial);
}
#[test]
fn rv_types_056200a9_generation_tracks_explicit_hole_publication() {
    let interner = Interner::new();
    interner.touch_with_id("upper", 2).unwrap();
    let sampled = interner.generation();
    let (initial, high_water) = interner.entries_after(0);
    assert_eq!(initial.len(), 1);
    assert_eq!(high_water, 0);
    interner.touch_with_id("lower-hole", 1).unwrap();
    assert!(interner.generation() > sampled);
    let (published, high_water) = interner.entries_after(high_water);
    assert_eq!(published.len(), 2);
    assert_eq!(high_water, 2);
}

#[test]
fn rv_types_1eba1d8a_forward_publication_and_collision_rollback_are_atomic() {
    let interner = Interner::new();
    let id = InternerKey::new(7);
    interner.touch_with_id("stable", 7).unwrap();
    assert_eq!(interner.get_ind("stable"), Some(id.clone()));
    assert_eq!(interner.get_str(&id).as_deref(), Some("stable"));
    assert!(matches!(
        interner.touch_with_id("loser", 7),
        Err(InternerError::IdConflict { .. })
    ));
    assert_eq!(interner.get_ind("loser"), None);
    assert_eq!(interner.get_str(&id).as_deref(), Some("stable"));
}

#[test]
fn rv_types_3cfa3fb6_paused_automatic_touch_observes_explicit_high_water() {
    let interner = Arc::new(Interner::new());
    let admitted = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    let automatic = {
        let interner = Arc::clone(&interner);
        let admitted = Arc::clone(&admitted);
        let resume = Arc::clone(&resume);
        thread::spawn(move || {
            interner
                .touch_ind_with_test_hooks(
                    "automatic",
                    || {
                        admitted.wait();
                        resume.wait();
                    },
                    |_| {},
                    |_| {},
                )
                .unwrap()
                .into_key()
        })
    };
    admitted.wait();
    interner.touch_with_id("explicit", 1).unwrap();
    resume.wait();
    let automatic = automatic.join().unwrap();
    assert_ne!(automatic.id(), 1);
    assert_eq!(interner.get_str(&automatic).as_deref(), Some("automatic"));
    assert_eq!(
        interner.get_str(&InternerKey::new(1)).as_deref(),
        Some("explicit")
    );
}

#[test]
fn rv_types_5dd242f6_rejected_collision_leaves_quota_unchanged() {
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
    interner.touch_ind("free").unwrap();
}

#[test]
fn rv_types_86e037b2_explicit_and_automatic_publications_resolve() {
    let interner = Interner::new();
    let automatic = interner.touch_ind("auto").unwrap().into_key();
    interner.touch_with_id("explicit", 20).unwrap();
    assert_eq!(interner.get_str(&automatic).as_deref(), Some("auto"));
    assert_eq!(
        interner.get_str(&InternerKey::new(20)).as_deref(),
        Some("explicit")
    );
}

/// #501 concurrent-growth stress: many threads each touch a large batch of
/// DISTINCT fresh names concurrently. This forces reverse-spine population —
/// the rare doubling-growth clone-forward and the common `OnceLock::set`
/// in-place path, both serialized by `reverse_write_lock` — to interleave
/// heavily across threads.
///
/// Asserts that afterwards EVERY name is resolvable both directions
/// (`get_ind` forward + `get_str` reverse round-trip), with no lost touches,
/// no panics, and no duplicate-id assignment (every id appears exactly once).
/// This test is genuinely load-bearing: it reproducibly failed against an
/// earlier fully-lock-free two-phase design (ensure-capacity-then-set-then-
/// confirm-and-retry) that had a data-loss race — see the struct doc on
/// `Interner::reverse_write_lock` for the exact mechanism.

#[test]
fn test_concurrent_growth_no_lost_touches_no_dup_ids() {
    let interner = Arc::new(Interner::new());
    let num_threads = 32;
    let keys_per_thread = 400; // 12,800 distinct names — forces many grows.
    let mut handles = vec![];

    for t in 0..num_threads {
        let interner_clone = Arc::clone(&interner);
        handles.push(thread::spawn(move || {
            let mut local: Vec<(String, u64)> = Vec::with_capacity(keys_per_thread);
            for j in 0..keys_per_thread {
                let name = format!("t{t}_k{j}");
                let id = interner_clone.touch_ind(&name).unwrap().key().id();
                local.push((name, id));
            }
            local
        }));
    }

    let mut all: Vec<(String, u64)> = Vec::new();
    for h in handles {
        all.extend(h.join().unwrap());
    }

    let total = num_threads * keys_per_thread;
    assert_eq!(all.len(), total, "every touch must return a result");
    assert_eq!(
        interner.len(),
        total,
        "no lost touches — forward map holds every distinct name"
    );

    // Every name resolves both directions and round-trips through its id.
    let mut seen_ids: TFxSet<u64> = TFxSet::default();
    for (name, id) in &all {
        // Reverse: id -> name.
        let back = interner.get_str(&InternerKey::new(*id));
        assert_eq!(
            back.as_deref(),
            Some(name.as_str()),
            "reverse lookup for id {id} must yield '{name}'"
        );
        // Forward: name -> id (must match the id returned by touch_ind).
        assert_eq!(
            interner.get_ind(name.as_str()).map(|k| k.id()),
            Some(*id),
            "forward lookup for '{name}' must yield id {id}"
        );
        // No duplicate-id assignment: each id appears exactly once across all
        // distinct names.
        assert!(
            seen_ids.insert(*id),
            "duplicate id {id} assigned to two distinct names"
        );
    }
    assert_eq!(seen_ids.len(), total, "ids must be unique and dense");
}

/// #501 gap-semantics regression: with the `OnceLock` spine, a reserved-but-
/// unset id (a gap) must behave IDENTICALLY to the pre-change `Option<Arc<str>>`
/// design — `entries_after` captures entries above the gap but freezes the
/// high-water mark before it, and `entries_in_id_range` simply skips the gap.
///
/// A gap is produced deterministically via `with_state` (id 2 omitted), which
/// mirrors the "reserved-but-unswapped / leaked id" slot state a raced/aborted
/// `touch_ind` leaves behind (both read as an unset cell = `None`).
#[test]
fn test_gap_semantics_unchanged_with_oncelock_spine() {
    // idx: 0=sentinel, 1=Some("a"), 2=unset gap, 3=Some("c"), 4=Some("d")
    let interner = Interner::with_state(vec![
        (InternerKey::new(1), UserKey::from_str("a")),
        (InternerKey::new(3), UserKey::from_str("c")),
        (InternerKey::new(4), UserKey::from_str("d")),
    ]);

    // entries_after: captures 1, 3, 4 (above-gap entries not lost) but the
    // high-water mark freezes at 1 (just before the gap at id 2).
    let (entries, high_water) = interner.entries_after(0);
    let ids: Vec<u64> = entries.iter().map(|(k, _)| k.id()).collect();
    assert!(ids.contains(&1), "id 1 (before gap) captured");
    assert!(ids.contains(&3), "id 3 (above gap) captured — no data loss");
    assert!(ids.contains(&4), "id 4 (above gap) captured — no data loss");
    assert!(!ids.contains(&2), "gap id 2 is unset — not captured");
    assert_eq!(high_water, 1, "high-water freezes before the gap");

    // entries_in_id_range skips the gap, returns only populated ids in range.
    let ranged = interner.entries_in_id_range(0, 4);
    let ranged_ids: Vec<u64> = ranged.iter().map(|(k, _)| k.id()).collect();
    assert_eq!(ranged_ids.len(), 3, "only 3 populated ids in (0..=4]");
    assert!(ranged_ids.contains(&1));
    assert!(ranged_ids.contains(&3));
    assert!(ranged_ids.contains(&4));
    assert!(!ranged_ids.contains(&2), "gap id 2 skipped");

    // The gap id reverse-resolves to None (unset OnceLock reads as absent).
    assert_eq!(interner.get_str(&InternerKey::new(2)), None);
}

/// #501: `entries_after`'s gap-handling must also treat TRAILING doubling-
/// growth capacity (unset cells past the highest touched id, left behind by
/// the last growth event) exactly like any other gap — distinct from
/// `test_gap_semantics_unchanged_with_oncelock_spine` above, which only
/// covers a MID-vec gap produced via `with_state` (an exact-size vec with no
/// trailing capacity at all).
///
/// Starting empty, `Interner::new()`'s reverse vec has capacity 1 (sentinel
/// only). Touch 1 (id=1) grows it to len 2 (exact fit, no trailing slack).
/// Touch 2 (id=2) grows it to len 4 (`(2*2).max(3) == 4`), leaving idx 3
/// unset — trailing capacity nobody has touched yet.
#[test]
fn test_entries_after_trailing_doubling_capacity_is_a_gap() {
    let interner = Interner::new();
    interner.touch_ind("a").unwrap(); // id 1, grows vec to len 2
    interner.touch_ind("b").unwrap(); // id 2, grows vec to len 4 (idx 3 unset)

    let (entries, new_high) = interner.entries_after(0);
    let ids: Vec<u64> = entries.iter().map(|(k, _)| k.id()).collect();
    assert_eq!(
        ids,
        vec![1, 2],
        "both touched ids captured, trailing idx 3 skipped"
    );
    assert_eq!(
        new_high, 2,
        "high-water must land on the highest TOUCHED id (2), not len-1 (3) — \
         the trailing unset cell from doubling growth must freeze the \
         high-water mark exactly like a raced/leaked-id gap does"
    );
}

/// A later-filled gap below the saved cursor is visible when the caller
/// recaptures from before the hole.
#[test]
fn entries_after_recaptures_filled_gap_below_cursor() {
    let interner = Interner::with_state(vec![(InternerKey::new(2), UserKey::from_str("upper"))]);
    let (initial, initial_cursor) = interner.entries_after(0);
    assert_eq!(
        initial.iter().map(|(k, _)| k.id()).collect::<Vec<_>>(),
        vec![2]
    );
    assert_eq!(initial_cursor, 0);

    interner.touch_with_id("hole", 1).unwrap();
    let (filled, next_cursor) = interner.entries_after(initial_cursor);
    assert!(filled.iter().any(|(k, _)| k.id() == 1));
    assert!(filled.iter().any(|(k, _)| k.id() == 2));
    assert_eq!(next_cursor, 2);
}

/// Regression test for the silent data-loss bug in `entries_after`:
/// dropping every populated entry above the gap (never persisted →
/// missing after restart). The fix keeps scanning past gaps but
/// freezes the high-water mark so the gap is re-captured by the
/// next persist.
#[test]
fn entries_after_captures_entries_above_gap() {
    // with_state creates reverse vec with None holes for missing ids:
    //   idx 0 = None (sentinel)
    //   idx 1 = Some("a")
    //   idx 2 = None  (gap — id 2 was never interned)
    //   idx 3 = Some("c")
    let interner = Interner::with_state(vec![
        (InternerKey::new(1), UserKey::from_str("a")),
        (InternerKey::new(3), UserKey::from_str("c")),
    ]);

    let (entries, new_high) = interner.entries_after(0);

    // The entry above the gap (id 3) MUST be captured — was the
    // data-loss bug.
    assert!(
        entries.iter().any(|(k, _)| k.id() == 3),
        "entry above the gap must still be captured (was the data-loss bug)"
    );
    // The entry before the gap (id 1) must also be present.
    assert!(
        entries.iter().any(|(k, _)| k.id() == 1),
        "entry before the gap must be captured"
    );
    // The high-water mark must be frozen before the gap so it is
    // re-captured on the next persist.
    assert_eq!(new_high, 1, "high-water mark must freeze before the gap");
}
