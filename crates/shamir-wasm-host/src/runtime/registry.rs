//! Lock-free function registry.
//!
//! `scc::HashMap` (CAS-based) per the engine's concurrency invariants — no
//! `RwLock`. The map key is the invocation name (the catalogue identity);
//! the value is the compiled function. `rename` is a pure re-key (no
//! recompile), `replace` swaps the artifact, `remove` drops it. In-flight
//! invocations hold their own `Arc` and finish on the version they captured
//! (RCU).

use super::context::{FnBatch, FnCtx};
use super::contract::ShamirFunction;
use super::error::{FnResult, FunctionError};
use super::params::Params;
use crate::builtin::Argon2idFunction;
use shamir_collections::THasher;
use shamir_types::types::value::QueryValue;
use std::sync::Arc;
#[cfg(test)]
use std::sync::OnceLock;

/// Test-only hook run between rename's install and retire steps.
#[cfg(test)]
type InterleaveHook = Box<dyn Fn(&FunctionRegistry) + Send + Sync>;

/// Maps an invocation name to a compiled [`ShamirFunction`].
pub struct FunctionRegistry {
    functions: scc::HashMap<String, Arc<dyn ShamirFunction>, THasher>,
    #[cfg(test)]
    pub(crate) interleave: OnceLock<InterleaveHook>,
}

impl FunctionRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self {
            functions: scc::HashMap::with_hasher(THasher::default()),
            #[cfg(test)]
            interleave: OnceLock::new(),
        }
    }

    /// A registry pre-loaded with the built-in functions.
    pub fn with_builtins() -> Self {
        let reg = Self::new();
        // Built-ins never collide on a fresh registry.
        let _ = reg.register("argon2id", Arc::new(Argon2idFunction));
        reg
    }

    /// Register `f` under `name`; errors if the name is taken.
    pub fn register(&self, name: impl Into<String>, f: Arc<dyn ShamirFunction>) -> FnResult<()> {
        let name = name.into();
        self.functions
            .insert_sync(name.clone(), f)
            .map_err(|_| FunctionError::AlreadyExists(name))
    }

    /// Register or overwrite (create-or-replace). New invocations pick up the
    /// new artifact; in-flight ones keep the `Arc` they captured.
    pub fn replace(&self, name: impl Into<String>, f: Arc<dyn ShamirFunction>) {
        let name = name.into();
        match self.functions.entry_sync(name) {
            scc::hash_map::Entry::Occupied(mut occupied) => *occupied.get_mut() = f,
            scc::hash_map::Entry::Vacant(vacant) => {
                vacant.insert_entry(f);
            }
        }
    }

    /// Look up a function by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn ShamirFunction>> {
        self.functions.read_sync(name, |_, v| v.clone())
    }

    /// Whether a function is registered under `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.functions.contains_sync(name)
    }

    /// Drop a function. Returns `true` if it existed.
    pub fn remove(&self, name: &str) -> bool {
        self.functions.remove_sync(name).is_some()
    }

    /// Rename `from` → `to` (pure re-key, no recompile). Errors if `from` is
    /// missing or `to` is already taken.
    ///
    /// The target is installed before retiring the source, so concurrent
    /// readers never see an absence caused by the rename itself.
    pub fn rename(&self, from: &str, to: &str) -> FnResult<()> {
        let function = self
            .get(from)
            .ok_or_else(|| FunctionError::NotFound(from.to_string()))?;
        self.functions
            .insert_sync(to.to_string(), function.clone())
            .map_err(|_| FunctionError::AlreadyExists(to.to_string()))?;

        let same = |candidate: &Arc<dyn ShamirFunction>| {
            std::ptr::addr_eq(Arc::as_ptr(candidate), Arc::as_ptr(&function))
        };
        #[cfg(test)]
        if let Some(interleave) = self.interleave.get() {
            interleave(self);
        }
        if self
            .functions
            .remove_if_sync(from, |candidate| same(candidate))
            .is_some()
        {
            return Ok(());
        }

        self.functions
            .remove_if_sync(to, |candidate| same(candidate));
        Err(FunctionError::NotFound(from.to_string()))
    }

    /// Snapshot of registered names.
    #[allow(clippy::disallowed_methods)] // O(N) ack: Vec-capacity sizing at snapshot, off hot path
    pub fn list(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.functions.len());
        self.functions.iter_sync(|k, _| {
            out.push(k.clone());
            true
        });
        out
    }

    /// Number of registered functions.
    #[allow(clippy::disallowed_methods)] // O(N) ack: cardinality accessor, off hot path
    pub fn len(&self) -> usize {
        self.functions.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.functions.is_empty()
    }

    /// Look up `name` and invoke it.
    pub async fn invoke(
        &self,
        name: &str,
        ctx: &FnCtx,
        batch: &FnBatch,
        params: &Params,
    ) -> FnResult<QueryValue> {
        let f = self
            .get(name)
            .ok_or_else(|| FunctionError::NotFound(name.to_string()))?;
        f.call(ctx, batch, params).await
    }
}

impl Default for FunctionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
