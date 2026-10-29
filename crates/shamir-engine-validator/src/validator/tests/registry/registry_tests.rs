//! `ValidatorRegistry` unit tests — concurrency (group 11, P1) plus the
//! id→name reverse-map / cardinality-mirror behavior added alongside it.

use crate::validator::{
    RecordFields, RecordValidator, Validation, ValidatorCtx, ValidatorRegistry,
};
use async_trait::async_trait;
use shamir_types::types::record_id::RecordId;
use std::sync::{Arc, Barrier};
use std::thread;

struct AcceptAll;

#[async_trait]
impl RecordValidator for AcceptAll {
    async fn validate(
        &self,
        _new: Option<&dyn RecordFields>,
        _old: Option<&dyn RecordFields>,
        _ctx: &ValidatorCtx<'_>,
    ) -> Validation {
        Validation::accept()
    }
}

/// Group 11 regression: two concurrent `add_binding` calls for the SAME
/// validator id targeting DIFFERENT tables must both survive.
///
/// Pre-fix, `add_binding` was a check-then-act on the lock-free `bound_in`
/// map: `entry_sync(id).and_modify(..)` (a no-op while the entry is vacant)
/// followed by a SEPARATE `insert_sync(id, BTreeSet::from([table])).ok()`
/// that silently discarded scc's `Err` when a racing caller had already
/// inserted the entry between the two calls. Two threads racing on the same
/// id could both observe "vacant" and both take the `insert_sync` branch —
/// the second `insert_sync` fails (key now occupied) and its table binding
/// vanished from `bound_in` with no error surfaced anywhere.
///
/// A single trial is not guaranteed to land in that window, so this runs
/// many trials (fresh id + registry each time) with a `Barrier` forcing both
/// threads to enter `add_binding` in lockstep, maximizing the chance either
/// buggy interleaving is hit at least once across the loop.
#[test]
fn concurrent_add_binding_same_id_different_tables_both_survive() {
    for i in 0..500 {
        let registry = Arc::new(ValidatorRegistry::new());
        let id = RecordId::system(&format!("race_v{i}"));
        let barrier = Arc::new(Barrier::new(2));

        let handles: Vec<_> = ["table_a", "table_b"]
            .into_iter()
            .map(|table| {
                let registry = Arc::clone(&registry);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    registry.add_binding(&id, table);
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        let mut bound = registry.bound_tables(&id);
        bound.sort();
        assert_eq!(
            bound,
            vec!["table_a".to_string(), "table_b".to_string()],
            "iteration {i}: both concurrent add_binding calls must survive"
        );
    }
}

/// `remove` must drop the id→name reverse mapping too, so a later `register`
/// under the same name succeeds (name_to_id no longer points at the removed
/// id) and `name_for_id` returns `None` for the removed id.
#[test]
fn remove_clears_name_and_id_reverse_maps() {
    let registry = ValidatorRegistry::new();
    let id = RecordId::system("removable");
    registry
        .register(id, "removable", Arc::new(AcceptAll))
        .unwrap();
    assert_eq!(registry.name_for_id(&id), Some("removable".to_string()));
    assert_eq!(registry.id_for_name("removable"), Some(id));

    assert!(registry.remove(&id));

    assert_eq!(registry.name_for_id(&id), None);
    assert_eq!(registry.id_for_name("removable"), None);

    // The name must be free again for a fresh registration.
    let id2 = RecordId::system("removable2");
    registry
        .register(id2, "removable", Arc::new(AcceptAll))
        .unwrap();
    assert_eq!(registry.id_for_name("removable"), Some(id2));
}

/// `len`/`is_empty` must track registrations and removals exactly (the
/// atomic mirror, not a full scc traversal).
#[test]
fn len_and_is_empty_track_register_and_remove() {
    let registry = ValidatorRegistry::new();
    assert!(registry.is_empty());
    assert_eq!(registry.len(), 0);

    let id_a = RecordId::system("len_a");
    let id_b = RecordId::system("len_b");
    registry
        .register(id_a, "len_a", Arc::new(AcceptAll))
        .unwrap();
    assert!(!registry.is_empty());
    assert_eq!(registry.len(), 1);

    registry
        .register(id_b, "len_b", Arc::new(AcceptAll))
        .unwrap();
    assert_eq!(registry.len(), 2);

    assert!(registry.remove(&id_a));
    assert_eq!(registry.len(), 1);
    assert!(!registry.is_empty());

    assert!(registry.remove(&id_b));
    assert_eq!(registry.len(), 0);
    assert!(registry.is_empty());

    // Removing a nonexistent id must not underflow the mirror.
    assert!(!registry.remove(&id_b));
    assert_eq!(registry.len(), 0);
}
