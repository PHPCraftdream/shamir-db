//! `ScalarResolver` — 2-layer scalar function lookup (user → builtin).
//!
//! Built-in scalars live in a process-global [`ScalarRegistry`] (`builtin_scalars()`).
//! An embedder can register additional native scalars per-database via
//! [`UserScalarLayer`]. At call time the resolver checks the user layer first and
//! falls back to the built-in registry, giving user functions name-shadowing
//! priority while keeping the built-in fast-path (empty user layer → one hash-miss
//! on an empty `scc::HashMap` → straight to the static builtin).

use std::sync::Arc;

use scc::HashMap as SccHashMap;
use shamir_collections::THasher;

use crate::registry::{FnEntry, ScalarError, ScalarResult};
use shamir_types::types::value::QueryValue;

// -----------------------------------------------------------------
// UserScalarLayer — the per-DB user-registered scalar table.
// -----------------------------------------------------------------

/// Per-database user scalar table. Lives behind an `Arc` so a
/// [`ScalarResolver`] can hold a cheap reference on the hot filter path.
///
/// Registration is lock-free via `scc::HashMap` (optimistic CAS). Lookups are
/// read-only hash probes with no locking.
pub struct UserScalarLayer {
    fns: SccHashMap<String, FnEntry, THasher>,
}

impl UserScalarLayer {
    /// Create an empty user layer.
    pub fn new() -> Self {
        Self {
            fns: SccHashMap::with_hasher(THasher::default()),
        }
    }

    /// Register (or replace) a user scalar under `name`.
    pub fn register(&self, name: impl Into<String>, entry: FnEntry) {
        let _ = self.fns.insert_sync(name.into(), entry);
    }

    /// Look up an entry by name.
    pub fn get(&self, name: &str) -> Option<FnEntry> {
        self.fns.get_sync(name).map(|e| e.get().clone())
    }

    /// Whether the layer holds any functions.
    pub fn is_empty(&self) -> bool {
        self.fns.is_empty()
    }
}

impl Default for UserScalarLayer {
    fn default() -> Self {
        Self::new()
    }
}

// -----------------------------------------------------------------
// ScalarResolver — the 2-layer dispatch struct threaded into FilterContext.
// -----------------------------------------------------------------

/// 2-layer scalar resolver: user layer (per-DB) checked first, built-in
/// fallback second.
///
/// Designed for the hot filter path: the user layer is an `Arc` to a lock-free
/// `scc::HashMap`; the builtin layer is a `&'static ScalarRegistry`. When the
/// user layer is empty (the common case for built-in-only databases), the
/// extra cost is exactly one hash-miss on the empty `scc::HashMap` before
/// falling through to the static builtin lookup.
#[derive(Clone)]
pub struct ScalarResolver {
    /// Per-database user scalars. Empty when no user scalars are registered.
    pub user: Arc<UserScalarLayer>,
    /// Process-global built-in scalars (`builtin_scalars()`).
    pub builtin: &'static crate::registry::ScalarRegistry,
}

impl ScalarResolver {
    /// Build a resolver backed by only the built-in scalars (empty user layer).
    /// This is the zero-cost default used when no per-DB user scalars exist.
    ///
    /// Returns a `&'static`-backed clone — the internal `Arc<UserScalarLayer>`
    /// is shared across ALL callers via a `OnceLock`, so `builtins_only()` is
    /// allocation-free after the first call (one Arc for the process lifetime).
    pub fn builtins_only() -> Self {
        use std::sync::OnceLock;
        static EMPTY: OnceLock<Arc<UserScalarLayer>> = OnceLock::new();
        let user = EMPTY.get_or_init(|| Arc::new(UserScalarLayer::new()));
        // Clone the Arc — cheap (refcount bump), no allocation.
        Self {
            user: Arc::clone(user),
            builtin: crate::static_builtin(),
        }
    }

    /// Build a resolver with a user layer + the built-in fallback.
    pub fn new(user: Arc<UserScalarLayer>) -> Self {
        Self {
            user,
            builtin: crate::static_builtin(),
        }
    }

    /// Dispatch `name` with `args`, checking the user layer first then the
    /// builtin registry. Arity validation follows the same semantics as
    /// [`ScalarRegistry::call`](crate::registry::ScalarRegistry::call).
    pub fn call(&self, name: &str, args: &[QueryValue]) -> ScalarResult {
        // Fast path: user layer (lock-free read, typically empty → one hash-miss).
        if let Some(entry) = self.user.get(name) {
            return dispatch_entry(&entry, args);
        }
        // Fallback: built-in static registry.
        dispatch_entry(
            self.builtin
                .get(name)
                .ok_or_else(|| ScalarError::new("unknown_function"))?,
            args,
        )
    }

    /// Look up an entry by name (user layer first, builtin second).
    pub fn get(&self, name: &str) -> Option<FnEntry> {
        if let Some(e) = self.user.get(name) {
            return Some(e);
        }
        self.builtin.get(name).cloned()
    }
}

/// Shared arity-validation + dispatch logic (identical to ScalarRegistry::call).
#[inline]
fn dispatch_entry(entry: &FnEntry, args: &[QueryValue]) -> ScalarResult {
    if args.len() < entry.min_args {
        return Err(ScalarError::new("arity"));
    }
    if let Some(max) = entry.max_args {
        if args.len() > max {
            return Err(ScalarError::new("arity"));
        }
    }
    (entry.f)(args)
}
