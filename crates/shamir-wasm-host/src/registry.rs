//! Lock-free function registry.
//!
//! `scc::HashMap` (CAS-based) per the engine's concurrency invariants — no
//! `RwLock`. The map key is the invocation name (the catalogue identity);
//! the value is the compiled function. `rename` is a pure re-key (no
//! recompile), `replace` swaps the artifact, `remove` drops it. In-flight
//! invocations hold their own `Arc` and finish on the version they captured
//! (RCU).

use super::builtin::Argon2idFunction;
use super::context::{FnBatch, FnCtx};
use super::contract::ShamirFunction;
use super::error::{FnResult, FunctionError};
use super::params::Params;
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

/// Maps an invocation name to a compiled [`ShamirFunction`].
pub struct FunctionRegistry {
    functions: scc::HashMap<String, Arc<dyn ShamirFunction>>,
}

impl FunctionRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self {
            functions: scc::HashMap::new(),
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
            .insert(name.clone(), f)
            .map_err(|_| FunctionError::AlreadyExists(name))
    }

    /// Register or overwrite (create-or-replace). New invocations pick up the
    /// new artifact; in-flight ones keep the `Arc` they captured.
    pub fn replace(&self, name: impl Into<String>, f: Arc<dyn ShamirFunction>) {
        let name = name.into();
        let _ = self.functions.remove(&name);
        let _ = self.functions.insert(name, f);
    }

    /// Look up a function by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn ShamirFunction>> {
        self.functions.read(name, |_, v| v.clone())
    }

    /// Whether a function is registered under `name`.
    pub fn contains(&self, name: &str) -> bool {
        self.functions.contains(name)
    }

    /// Drop a function. Returns `true` if it existed.
    pub fn remove(&self, name: &str) -> bool {
        self.functions.remove(name).is_some()
    }

    /// Rename `from` → `to` (pure re-key, no recompile). Errors if `from` is
    /// missing or `to` is already taken.
    pub fn rename(&self, from: &str, to: &str) -> FnResult<()> {
        if self.functions.contains(to) {
            return Err(FunctionError::AlreadyExists(to.to_string()));
        }
        let (_, f) = self
            .functions
            .remove(from)
            .ok_or_else(|| FunctionError::NotFound(from.to_string()))?;
        // `to` was free a moment ago; if a racing register grabbed it, put
        // `from` back and report the collision.
        self.functions.insert(to.to_string(), f).map_err(|(_, f)| {
            let _ = self.functions.insert(from.to_string(), f);
            FunctionError::AlreadyExists(to.to_string())
        })
    }

    /// Snapshot of registered names.
    pub fn list(&self) -> Vec<String> {
        let mut out = Vec::with_capacity(self.functions.len());
        self.functions.scan(|k, _| out.push(k.clone()));
        out
    }

    /// Number of registered functions.
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
