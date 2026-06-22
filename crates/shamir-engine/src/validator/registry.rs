//! Lock-free validator registry.
//!
//! `scc::HashMap` (CAS-based) per the engine's concurrency invariants — no
//! `RwLock`. Validators are keyed by `RecordId` (stable catalogue identity);
//! a secondary `name → id` index enforces unique names and supports
//! human-friendly lookups. A per-validator `bound_in` set tracks which
//! tables reference the validator so `drop` can refuse when bindings exist.

use crate::function::ShamirFunction;
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;
use std::collections::BTreeSet;
use std::sync::Arc;

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

/// Lock-free registry of compiled validators, keyed by `RecordId`.
///
/// Mirrors [`FunctionRegistry`](crate::function::FunctionRegistry) but
/// adds `name_to_id` (validators are resolved by id on the write path)
/// and `bound_in` (referential integrity — `drop` refuses while bound).
pub struct ValidatorRegistry {
    /// Compiled validator artifact, keyed by catalogue `_id`.
    by_id: scc::HashMap<RecordId, Arc<dyn ShamirFunction>, THasher>,
    /// Unique-name → id reverse index.
    name_to_id: scc::HashMap<String, RecordId, THasher>,
    /// Tables each validator is bound to (canonical `"db/repo/table"` keys).
    bound_in: scc::HashMap<RecordId, BTreeSet<String>, THasher>,
}

impl ValidatorRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            by_id: scc::HashMap::with_hasher(THasher::default()),
            name_to_id: scc::HashMap::with_hasher(THasher::default()),
            bound_in: scc::HashMap::with_hasher(THasher::default()),
        }
    }

    /// Register a compiled validator under `(id, name)`.
    ///
    /// Errors if a validator with the same name already exists.
    pub fn register(
        &self,
        id: RecordId,
        name: impl Into<String>,
        compiled: Arc<dyn ShamirFunction>,
    ) -> Result<(), ValidatorRegistryError> {
        let name = name.into();
        // Check name uniqueness first.
        if self.name_to_id.contains(&name) {
            return Err(ValidatorRegistryError::AlreadyExists(name));
        }
        // Insert name → id; if a racing register grabbed the name, report.
        self.name_to_id
            .insert(name.clone(), id)
            .map_err(|_| ValidatorRegistryError::AlreadyExists(name.clone()))?;
        // Insert the compiled artifact. If the id already exists (shouldn't
        // happen in normal operation), remove the name mapping and report.
        if self.by_id.insert(id, compiled).is_err() {
            let _ = self.name_to_id.remove(&name);
            return Err(ValidatorRegistryError::AlreadyExists(name));
        }
        Ok(())
    }

    /// Look up a compiled validator by id.
    pub fn get_by_id(&self, id: &RecordId) -> Option<Arc<dyn ShamirFunction>> {
        self.by_id.read(id, |_, v| v.clone())
    }

    /// Swap the compiled artifact for an already-registered `id` in place,
    /// preserving its name and table bindings. In-flight invocations keep the
    /// `Arc` they captured (RCU). Returns `true` if the id existed.
    ///
    /// This is the validator counterpart of
    /// [`FunctionRegistry::replace`](crate::function::FunctionRegistry::replace)
    /// — used to substitute a live artifact (e.g. a native validator) for the
    /// one materialised from the catalogue, without disturbing bindings.
    pub fn replace_artifact(&self, id: &RecordId, compiled: Arc<dyn ShamirFunction>) -> bool {
        self.by_id
            .update(id, |_, v| *v = compiled.clone())
            .is_some()
    }

    /// Resolve a name to its `RecordId`.
    pub fn id_for_name(&self, name: &str) -> Option<RecordId> {
        self.name_to_id.read(name, |_, v| *v)
    }

    /// Rename a validator (`from` → `to`). The id and bindings are unchanged.
    ///
    /// Errors if `from` is missing or `to` is already taken.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), ValidatorRegistryError> {
        if self.name_to_id.contains(to) {
            return Err(ValidatorRegistryError::AlreadyExists(to.to_string()));
        }
        let (_, id) = self
            .name_to_id
            .remove(from)
            .ok_or_else(|| ValidatorRegistryError::NotFound(from.to_string()))?;
        // Insert the new name. If a racing rename grabbed `to`, put `from` back.
        self.name_to_id
            .insert(to.to_string(), id)
            .map_err(|(_, id)| {
                let _ = self.name_to_id.insert(from.to_string(), id);
                ValidatorRegistryError::AlreadyExists(to.to_string())
            })
    }

    /// Remove a validator by id. The caller must check `is_bound` beforehand;
    /// this method does NOT enforce bound-refusal (the facade does).
    ///
    /// Returns `true` if the validator existed.
    pub fn remove(&self, id: &RecordId) -> bool {
        let existed = self.by_id.remove(id).is_some();
        // Remove the name → id entry. We need to scan because we don't have
        // the name here, but the map is small (validators are few).
        let mut name_key: Option<String> = None;
        self.name_to_id.scan(|k, v| {
            if v == id {
                name_key = Some(k.clone());
            }
        });
        if let Some(k) = name_key {
            let _ = self.name_to_id.remove(&k);
        }
        let _ = self.bound_in.remove(id);
        existed
    }

    /// Whether the validator is bound to at least one table.
    pub fn is_bound(&self, id: &RecordId) -> bool {
        self.bound_in
            .read(id, |_, set| !set.is_empty())
            .unwrap_or(false)
    }

    /// Return the list of tables the validator is bound to.
    pub fn bound_tables(&self, id: &RecordId) -> Vec<String> {
        self.bound_in
            .read(id, |_, set| set.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Record a binding between a validator and a table.
    pub fn add_binding(&self, id: &RecordId, table: impl Into<String>) {
        let table = table.into();
        let _ = self.bound_in.entry(*id).and_modify(|set| {
            set.insert(table.clone());
        });
        // If the entry did not exist, insert a fresh set.
        let _ = self.bound_in.insert(*id, BTreeSet::from([table])).ok();
    }

    /// Remove a binding between a validator and a table.
    pub fn remove_binding(&self, id: &RecordId, table: &str) {
        let _ = self.bound_in.entry(*id).and_modify(|set| {
            set.remove(table);
        });
    }

    /// Remove a table reference from **every** validator's `bound_in` set.
    ///
    /// Returns `(id, name)` pairs for each validator whose bound_in set was
    /// non-trivially modified (i.e. the table was actually present). The
    /// caller should persist the updated `bound_in` for those validators.
    pub fn unbind_all_for_table(&self, table_ref: &str) -> Vec<(RecordId, String)> {
        // Step 1: collect ids that contain this table_ref.
        let mut candidate_ids = Vec::new();
        self.bound_in.scan(|id, set| {
            if set.contains(table_ref) {
                candidate_ids.push(*id);
            }
        });

        // Step 2: remove the table_ref from each candidate (entry gives &mut).
        let mut affected = Vec::new();
        for id in candidate_ids {
            let _ = self.bound_in.entry(id).and_modify(|set| {
                if set.remove(table_ref) {
                    if let Some(name) = self.name_for_id(&id) {
                        affected.push((id, name));
                    }
                }
            });
        }
        affected
    }

    /// Resolve a `RecordId` back to its name (reverse of `id_for_name`).
    pub fn name_for_id(&self, id: &RecordId) -> Option<String> {
        let mut found: Option<String> = None;
        self.name_to_id.scan(|name, vid| {
            if vid == id && found.is_none() {
                found = Some(name.clone());
            }
        });
        found
    }

    /// Snapshot of all registered validators as `(id, name)` pairs.
    pub fn list(&self) -> Vec<(RecordId, String)> {
        let mut out = Vec::new();
        self.name_to_id.scan(|name, id| {
            out.push((*id, name.clone()));
        });
        out
    }

    /// Number of registered validators.
    #[allow(clippy::disallowed_methods)] // O(N) ack: cardinality accessor, off hot path
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

impl Default for ValidatorRegistry {
    fn default() -> Self {
        Self::new()
    }
}
