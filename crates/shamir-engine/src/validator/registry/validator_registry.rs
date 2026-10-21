//! Atomic validator lifecycle with lock-free snapshot reads.
//!
//! DDL writers clone one immutable state and publish it once. Each read loads
//! one snapshot; separate read calls may observe different committed versions.

use crate::validator::record_validator::RecordValidator;
use arc_swap::ArcSwap;
use shamir_collections::TFxMap;
use shamir_types::types::record_id::RecordId;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex, MutexGuard};

/// Error type for validator registry operations.
#[derive(Debug, thiserror::Error)]
pub enum ValidatorRegistryError {
    /// A validator with the given name already exists.
    #[error("validator already exists: {0}")]
    AlreadyExists(String),
    /// The requested validator was not found.
    #[error("validator not found: {0}")]
    NotFound(String),
    /// The validator is still bound to one or more tables.
    #[error("validator is still bound to tables: {0:?}")]
    StillBound(Vec<String>),
}

#[derive(Clone)]
struct Entry {
    name: String,
    artifact: Arc<dyn RecordValidator>,
    bound_in: BTreeSet<String>,
}

#[derive(Clone, Default)]
struct RegistryState {
    by_id: TFxMap<RecordId, Entry>,
    name_to_id: TFxMap<String, RecordId>,
    // add_binding historically accepts absent ids (e.g. pending native artifacts).
    // Keep those bindings without advertising a live validator in list/lookups.
    pending_bindings: TFxMap<RecordId, BTreeSet<String>>,
}

#[cfg(test)]
type MutationHook = Arc<dyn Fn(&str) + Send + Sync>;

/// Registry of compiled validators with lock-free reads and serialized DDL.
/// Ids are stable across rename/replacement; replacement preserves bindings.
pub struct ValidatorRegistry {
    state: ArcSwap<RegistryState>,
    // DDL op, contention is nil; sync mutations only, never held across .await.
    writer: Mutex<()>,
    #[cfg(test)]
    mutation_hook: Mutex<Option<MutationHook>>,
}

impl ValidatorRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            state: ArcSwap::from_pointee(RegistryState::default()),
            writer: Mutex::new(()),
            #[cfg(test)]
            mutation_hook: Mutex::new(None),
        }
    }

    fn lock_writer(&self) -> MutexGuard<'_, ()> {
        #[cfg(test)]
        self.run_hook("before_lock");
        self.writer.lock().unwrap_or_else(|poisoned| {
            // A panic discards the unpublished clone; the published snapshot
            // is always complete, so recovery needs no repair of shared data.
            self.writer.clear_poison();
            poisoned.into_inner()
        })
    }

    #[cfg(test)]
    pub(in crate::validator) fn set_mutation_hook(&self, hook: Option<MutationHook>) {
        *self.mutation_hook.lock().expect("test hook mutex poisoned") = hook;
    }

    #[cfg(test)]
    fn run_hook(&self, point: &str) {
        // Never invoke a callback under the hook mutex. Test callbacks must not
        // re-enter a mutation while the DDL writer is parked.
        let hook = self
            .mutation_hook
            .lock()
            .expect("test hook mutex poisoned")
            .clone();
        if let Some(hook) = hook {
            hook(point);
        }
    }

    /// Register a compiled validator under `(id, name)`.
    /// Errors if the name or id is already registered, without changing state.
    pub fn register(
        &self,
        id: RecordId,
        name: impl Into<String>,
        compiled: Arc<dyn RecordValidator>,
    ) -> Result<(), ValidatorRegistryError> {
        let name = name.into();
        let _writer = self.lock_writer();
        let mut next = (**self.state.load()).clone();
        if next.name_to_id.contains_key(&name) || next.by_id.contains_key(&id) {
            return Err(ValidatorRegistryError::AlreadyExists(name));
        }
        next.name_to_id.insert(name.clone(), id);
        #[cfg(test)]
        self.run_hook("register_name_staged");
        let bound_in = next.pending_bindings.remove(&id).unwrap_or_default();
        next.by_id.insert(
            id,
            Entry {
                name,
                artifact: compiled,
                bound_in,
            },
        );
        self.state.store(Arc::new(next));
        Ok(())
    }

    /// Look up a compiled validator by id.
    pub fn get_by_id(&self, id: &RecordId) -> Option<Arc<dyn RecordValidator>> {
        self.state
            .load()
            .by_id
            .get(id)
            .map(|entry| entry.artifact.clone())
    }

    /// Atomically insert or replace `(id, name, artifact)`, keeping bindings.
    /// A name owned by another id is rejected without changing state.
    /// In-flight invocations retain their previously captured artifact (RCU).
    pub fn replace(
        &self,
        id: RecordId,
        name: impl Into<String>,
        compiled: Arc<dyn RecordValidator>,
    ) -> Result<(), ValidatorRegistryError> {
        let name = name.into();
        let _writer = self.lock_writer();
        let mut next = (**self.state.load()).clone();
        if next.name_to_id.get(&name).is_some_and(|owner| *owner != id) {
            return Err(ValidatorRegistryError::AlreadyExists(name));
        }
        let bound_in = if let Some(old) = next.by_id.remove(&id) {
            next.name_to_id.remove(&old.name);
            old.bound_in
        } else {
            next.pending_bindings.remove(&id).unwrap_or_default()
        };
        #[cfg(test)]
        self.run_hook("replace_staged");
        next.name_to_id.insert(name.clone(), id);
        next.by_id.insert(
            id,
            Entry {
                name,
                artifact: compiled,
                bound_in,
            },
        );
        self.state.store(Arc::new(next));
        Ok(())
    }

    /// Swap only the artifact, preserving name and bindings. Returns whether
    /// the id existed. In-flight invocations keep their captured Arc (RCU).
    pub fn replace_artifact(&self, id: &RecordId, compiled: Arc<dyn RecordValidator>) -> bool {
        let _writer = self.lock_writer();
        let mut next = (**self.state.load()).clone();
        let Some(entry) = next.by_id.get_mut(id) else {
            return false;
        };
        entry.artifact = compiled;
        self.state.store(Arc::new(next));
        true
    }

    /// Resolve a name to its `RecordId`.
    pub fn id_for_name(&self, name: &str) -> Option<RecordId> {
        self.state.load().name_to_id.get(name).copied()
    }

    /// Rename a validator (`from` → `to`). Id and bindings are unchanged.
    /// Errors if `from` is missing or `to` is already taken.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), ValidatorRegistryError> {
        let _writer = self.lock_writer();
        let mut next = (**self.state.load()).clone();
        if next.name_to_id.contains_key(to) {
            return Err(ValidatorRegistryError::AlreadyExists(to.to_string()));
        }
        let id = next
            .name_to_id
            .remove(from)
            .ok_or_else(|| ValidatorRegistryError::NotFound(from.to_string()))?;
        #[cfg(test)]
        self.run_hook("rename_from_removed");
        next.name_to_id.insert(to.to_string(), id);
        next.by_id
            .get_mut(&id)
            .expect("live name must have an entry")
            .name = to.to_string();
        self.state.store(Arc::new(next));
        Ok(())
    }

    /// Remove by id. The caller must check `is_bound` beforehand; this method
    /// does NOT enforce bound-refusal (the facade does).
    /// Returns whether a live validator existed; pending bindings are cleared.
    pub fn remove(&self, id: &RecordId) -> bool {
        let _writer = self.lock_writer();
        let mut next = (**self.state.load()).clone();
        let old = next.by_id.remove(id);
        #[cfg(test)]
        self.run_hook("remove_artifact_staged");
        let existed = old.is_some();
        if let Some(old) = old {
            next.name_to_id.remove(&old.name);
        }
        next.pending_bindings.remove(id);
        self.state.store(Arc::new(next));
        existed
    }

    /// Whether the validator is bound to at least one table.
    pub fn is_bound(&self, id: &RecordId) -> bool {
        let state = self.state.load();
        state
            .by_id
            .get(id)
            .map(|entry| &entry.bound_in)
            .or_else(|| state.pending_bindings.get(id))
            .is_some_and(|set| !set.is_empty())
    }

    /// Return the list of bound tables, including pending artifacts.
    pub fn bound_tables(&self, id: &RecordId) -> Vec<String> {
        let state = self.state.load();
        state
            .by_id
            .get(id)
            .map(|entry| &entry.bound_in)
            .or_else(|| state.pending_bindings.get(id))
            .map(|set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Record a binding, retaining it even if the artifact is not yet live.
    pub fn add_binding(&self, id: &RecordId, table: impl Into<String>) {
        let table = table.into();
        let _writer = self.lock_writer();
        let mut next = (**self.state.load()).clone();
        if let Some(entry) = next.by_id.get_mut(id) {
            entry.bound_in.insert(table);
        } else {
            next.pending_bindings.entry(*id).or_default().insert(table);
        }
        self.state.store(Arc::new(next));
    }

    /// Remove a binding between a validator and a table.
    pub fn remove_binding(&self, id: &RecordId, table: &str) {
        let _writer = self.lock_writer();
        let mut next = (**self.state.load()).clone();
        if let Some(entry) = next.by_id.get_mut(id) {
            entry.bound_in.remove(table);
        } else if let Some(set) = next.pending_bindings.get_mut(id) {
            set.remove(table);
            if set.is_empty() {
                next.pending_bindings.remove(id);
            }
        }
        self.state.store(Arc::new(next));
    }

    /// Remove a table reference from every binding set in one publication.
    /// Returns live `(id, name)` pairs whose updated bindings need persistence.
    pub fn unbind_all_for_table(&self, table_ref: &str) -> Vec<(RecordId, String)> {
        let _writer = self.lock_writer();
        let mut next = (**self.state.load()).clone();
        let mut affected = Vec::new();
        for (id, entry) in &mut next.by_id {
            if entry.bound_in.remove(table_ref) {
                affected.push((*id, entry.name.clone()));
            }
        }
        next.pending_bindings.retain(|_, set| {
            set.remove(table_ref);
            !set.is_empty()
        });
        self.state.store(Arc::new(next));
        affected
    }

    /// Resolve an id back to its live name.
    pub fn name_for_id(&self, id: &RecordId) -> Option<String> {
        self.state
            .load()
            .by_id
            .get(id)
            .map(|entry| entry.name.clone())
    }

    /// One snapshot of all live validators as `(id, name)` pairs.
    pub fn list(&self) -> Vec<(RecordId, String)> {
        self.state
            .load()
            .by_id
            .iter()
            .map(|(id, entry)| (*id, entry.name.clone()))
            .collect()
    }

    /// Number of live validators. O(1) from the same snapshot as the entries.
    pub fn len(&self) -> usize {
        self.state.load().by_id.len()
    }

    /// Whether there are no live validators. O(1).
    pub fn is_empty(&self) -> bool {
        self.state.load().by_id.is_empty()
    }
}

impl Default for ValidatorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
