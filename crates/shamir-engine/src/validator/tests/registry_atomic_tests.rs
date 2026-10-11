//! B08 lifecycle regressions. Parking occurs in the private draft at the
//! historical split-map failure window. Reads while parked must see the old
//! complete state, and reads after joining must see the new complete state.
//! No assertion combines separate reads while publication can still occur.
//! Counterfactual failures are inferred with equivalent hooks in the old
//! split-map operations; no pre-fix or post-fix execution was performed.
//! Synchronous tests deliberately use real OS threads: the registry mutations
//! are synchronous DDL, and a Tokio runtime adds no scheduling coverage here.

use crate::validator::{
    RecordFields, RecordValidator, Validation, ValidatorCtx, ValidatorRegistry,
    ValidatorRegistryError,
};
use async_trait::async_trait;
use shamir_types::types::record_id::RecordId;
use std::sync::{mpsc, Arc, Barrier};
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

fn artifact() -> Arc<dyn RecordValidator> {
    Arc::new(AcceptAll)
}

struct Park {
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}

impl Park {
    fn install(registry: &ValidatorRegistry, point: &'static str) -> Self {
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let hook_entered = entered.clone();
        let hook_release = release.clone();
        registry.set_mutation_hook(Some(Arc::new(move |observed| {
            if observed == point {
                hook_entered.wait();
                hook_release.wait();
            }
        })));
        Self { entered, release }
    }

    fn wait(&self) {
        self.entered.wait();
    }

    fn resume(self, registry: &ValidatorRegistry) {
        registry.set_mutation_hook(None);
        self.release.wait();
    }
}

fn assert_consistent(registry: &ValidatorRegistry) {
    // Call only when all writers are joined or parked before publication.
    let listed = registry.list();
    assert_eq!(registry.len(), listed.len());
    assert_eq!(registry.is_empty(), listed.is_empty());
    for (id, name) in listed {
        assert_eq!(registry.id_for_name(&name), Some(id));
        assert_eq!(registry.name_for_id(&id), Some(name));
        assert!(registry.get_by_id(&id).is_some());
    }
}

/// Inferred red with equivalent hooks in the old name-first register:
/// a name is visible with no artifact, while list/count/reverse lookup disagree.
/// The draft must not escape.
#[test]
fn register_publishes_name_id_artifact_and_count_together() {
    let registry = Arc::new(ValidatorRegistry::new());
    let id = RecordId::system("atomic_register");
    let park = Park::install(&registry, "register_name_staged");
    let writer_registry = registry.clone();
    let writer = thread::spawn(move || writer_registry.register(id, "new", artifact()));
    park.wait();
    let before = (
        registry.id_for_name("new"),
        registry.get_by_id(&id),
        registry.name_for_id(&id),
        registry.list(),
        registry.len(),
        registry.is_empty(),
    );
    // Release before any assertion can panic or any join can block.
    park.resume(&registry);
    writer.join().unwrap().unwrap();
    assert_eq!(before.0, None);
    assert!(before.1.is_none());
    assert_eq!(before.2, None);
    assert!(before.3.is_empty());
    assert_eq!(before.4, 0);
    assert!(before.5);
    assert_eq!(registry.id_for_name("new"), Some(id));
    assert_consistent(&registry);
}

/// Inferred red with equivalent hooks in rename's remove-from/insert-to window:
/// the original name vanishes before the rename commits. A competing register
/// must reuse from only AFTER rename commits, without overwriting the reverse map.
#[test]
fn rename_and_register_old_name_are_serialized() {
    let registry = Arc::new(ValidatorRegistry::new());
    let id = RecordId::system("renamed");
    let fresh = RecordId::system("reuses_old_name");
    registry.register(id, "from", artifact()).unwrap();
    registry.add_binding(&id, "db/repo/table");
    let park = Park::install(&registry, "rename_from_removed");
    let writer_registry = registry.clone();
    let writer = thread::spawn(move || writer_registry.rename("from", "to"));
    park.wait();
    let before = (
        registry.id_for_name("from"),
        registry.id_for_name("to"),
        registry.name_for_id(&id),
        registry.bound_tables(&id),
    );
    // The parked callback owns its own cloned hook. Replace the instance hook
    // with an admission handshake so the competitor reaches the lock boundary.
    let (tx, rx) = mpsc::sync_channel(1);
    registry.set_mutation_hook(Some(Arc::new(move |point| {
        if point == "before_lock" {
            tx.send(()).unwrap();
        }
    })));
    let competitor_registry = registry.clone();
    let competitor = thread::spawn(move || competitor_registry.register(fresh, "from", artifact()));
    rx.recv().unwrap();
    park.resume(&registry);
    writer.join().unwrap().unwrap();
    competitor.join().unwrap().unwrap();
    assert_eq!(before.0, Some(id));
    assert_eq!(before.1, None);
    assert_eq!(before.2, Some("from".to_string()));
    assert_eq!(before.3, vec!["db/repo/table".to_string()]);
    assert_eq!(registry.id_for_name("from"), Some(fresh));
    assert_eq!(registry.id_for_name("to"), Some(id));
    assert!(registry.is_bound(&id));
    assert_consistent(&registry);
}

/// Inferred red with equivalent hooks in artifact-first remove: readers still
/// resolve the old name but the artifact has disappeared. Without serialized
/// writers, racing rename can recreate a dangling name. Test both writer orders.
#[test]
fn remove_racing_rename_leaves_no_dangling_name() {
    for remove_first in [true, false] {
        let registry = Arc::new(ValidatorRegistry::new());
        let id = RecordId::system("remove_rename");
        let old = artifact();
        registry.register(id, "from", old.clone()).unwrap();
        let point = if remove_first {
            "remove_artifact_staged"
        } else {
            "rename_from_removed"
        };
        let park = Park::install(&registry, point);
        let writer_registry = registry.clone();
        let writer = thread::spawn(move || {
            if remove_first {
                assert!(writer_registry.remove(&id));
            } else {
                writer_registry.rename("from", "to").unwrap();
            }
        });
        park.wait();
        let before = (
            registry.id_for_name("from"),
            registry.name_for_id(&id),
            registry.get_by_id(&id),
            registry.list(),
        );
        let (tx, rx) = mpsc::sync_channel(1);
        registry.set_mutation_hook(Some(Arc::new(move |point| {
            if point == "before_lock" {
                tx.send(()).unwrap();
            }
        })));
        let competitor_registry = registry.clone();
        let competitor = thread::spawn(move || {
            if remove_first {
                competitor_registry.rename("from", "to")
            } else {
                assert!(competitor_registry.remove(&id));
                Ok(())
            }
        });
        rx.recv().unwrap();
        // Never join a competitor blocked behind a parked writer mutex.
        park.resume(&registry);
        writer.join().unwrap();
        let result = competitor.join().unwrap();
        if remove_first {
            assert!(matches!(result, Err(ValidatorRegistryError::NotFound(_))));
        } else {
            result.unwrap();
        }
        assert_eq!(before.0, Some(id));
        assert_eq!(before.1, Some("from".to_string()));
        assert!(Arc::ptr_eq(&before.2.unwrap(), &old));
        assert_eq!(before.3, vec![(id, "from".to_string())]);
        assert_eq!(registry.id_for_name("from"), None);
        assert_eq!(registry.id_for_name("to"), None);
        assert_eq!(registry.name_for_id(&id), None);
        assert!(registry.get_by_id(&id).is_none());
        assert_consistent(&registry);
    }
}

/// Inferred red if replace is implemented as remove+register with equivalent
/// hooks: the parked window loses both artifact and bindings. After replacement,
/// is_bound must still prevent the facade's unbound-drop branch. Captured old
/// Arcs remain valid (RCU). This is a registry test, not a facade drop invocation.
#[test]
fn replacing_bound_validator_never_publishes_a_gap_or_loses_bindings() {
    let registry = Arc::new(ValidatorRegistry::new());
    let id = RecordId::system("bound_replacement");
    let old = artifact();
    let new = artifact();
    registry.register(id, "bound", old.clone()).unwrap();
    registry.add_binding(&id, "db/repo/table");
    let captured = registry.get_by_id(&id).unwrap();
    let park = Park::install(&registry, "replace_staged");
    let writer_registry = registry.clone();
    let replacement = new.clone();
    let writer = thread::spawn(move || writer_registry.replace(id, "bound", replacement));
    park.wait();
    let before = (
        registry.id_for_name("bound"),
        registry.get_by_id(&id),
        registry.is_bound(&id),
        registry.bound_tables(&id),
    );
    park.resume(&registry);
    writer.join().unwrap().unwrap();
    assert_eq!(before.0, Some(id));
    assert!(Arc::ptr_eq(&before.1.unwrap(), &old));
    assert!(before.2);
    assert_eq!(before.3, vec!["db/repo/table".to_string()]);
    assert!(registry.is_bound(&id), "facade must refuse an unbound drop");
    assert_eq!(registry.bound_tables(&id), before.3);
    assert!(Arc::ptr_eq(&registry.get_by_id(&id).unwrap(), &new));
    assert!(Arc::ptr_eq(&captured, &old));
    assert_consistent(&registry);
}

/// Rejections must not publish a partial rename, replacement or registration.
/// Also pins replace's upsert semantics and source-compatible replace_artifact.
#[test]
fn lifecycle_conflicts_leave_artifacts_names_bindings_and_count_unchanged() {
    let registry = ValidatorRegistry::new();
    let a = RecordId::system("conflict_a");
    let b = RecordId::system("conflict_b");
    let old = artifact();
    registry.register(a, "a", old.clone()).unwrap();
    registry.replace(b, "b", artifact()).unwrap();
    registry.add_binding(&a, "table");
    assert!(registry.register(a, "unused", artifact()).is_err());
    assert!(registry
        .register(RecordId::system("other"), "a", artifact())
        .is_err());
    assert!(registry.rename("a", "b").is_err());
    assert!(registry.rename("a", "a").is_err());
    assert!(registry.rename("missing", "unused").is_err());
    assert!(registry.replace(a, "b", artifact()).is_err());
    assert_eq!(registry.id_for_name("unused"), None);
    assert!(Arc::ptr_eq(&registry.get_by_id(&a).unwrap(), &old));
    assert_eq!(registry.bound_tables(&a), vec!["table".to_string()]);
    assert_eq!(registry.len(), 2);
    let new = artifact();
    assert!(registry.replace_artifact(&a, new.clone()));
    assert!(!registry.replace_artifact(&RecordId::system("absent"), artifact()));
    assert!(Arc::ptr_eq(&registry.get_by_id(&a).unwrap(), &new));
    registry.replace(a, "renamed", old).unwrap();
    assert_eq!(registry.id_for_name("a"), None);
    assert_eq!(registry.id_for_name("renamed"), Some(a));
    assert!(registry.is_bound(&a));
    assert_eq!(registry.len(), 2);
    assert_consistent(&registry);
}

/// A state containing only live entries would silently discard the existing
/// add_binding(absent-id) contract. Pending bindings must transfer to the live
/// entry on register OR replace, and stay invisible to list/name/artifact reads.
#[test]
fn pending_bindings_survive_materialisation_and_table_cleanup() {
    for replace in [false, true] {
        let registry = ValidatorRegistry::new();
        let id = RecordId::system("pending");
        registry.add_binding(&id, "keep");
        registry.add_binding(&id, "remove");
        registry.remove_binding(&id, "remove");
        registry.add_binding(&id, "drop_table");
        assert!(registry.unbind_all_for_table("drop_table").is_empty());
        assert!(registry.is_empty());
        assert_eq!(registry.name_for_id(&id), None);
        assert!(registry.get_by_id(&id).is_none());
        assert_eq!(registry.bound_tables(&id), vec!["keep".to_string()]);
        if replace {
            registry.replace(id, "pending", artifact()).unwrap();
        } else {
            registry.register(id, "pending", artifact()).unwrap();
        }
        assert_eq!(registry.bound_tables(&id), vec!["keep".to_string()]);
        assert_eq!(
            registry.unbind_all_for_table("keep"),
            vec![(id, "pending".to_string())],
        );
        assert!(!registry.is_bound(&id));
        assert_consistent(&registry);
        assert!(registry.remove(&id));
        assert!(!registry.remove(&id));
        assert_consistent(&registry);
    }
}

/// Hooks belong to their registry instance, never a global. A second registry
/// must be usable even while the first one's writer is parked.
#[test]
fn parking_hook_does_not_affect_another_registry() {
    let first = Arc::new(ValidatorRegistry::new());
    let second = ValidatorRegistry::new();
    let park = Park::install(&first, "register_name_staged");
    let writer_registry = first.clone();
    let id = RecordId::system("instance_local");
    let writer = thread::spawn(move || writer_registry.register(id, "first", artifact()));
    park.wait();
    let result = second.register(id, "second", artifact());
    park.resume(&first);
    writer.join().unwrap().unwrap();
    result.unwrap();
    assert_eq!(second.id_for_name("second"), Some(id));
    assert_eq!(first.id_for_name("second"), None);
    assert_consistent(&first);
    assert_consistent(&second);
}
