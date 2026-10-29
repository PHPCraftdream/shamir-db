//! `ScalarResolver` — 2-layer scalar function lookup (user → builtin).
//!
//! Built-in scalars live in a process-global [`ScalarRegistry`] (`builtin_scalars()`).
//! An embedder can register additional native scalars per-database via
//! [`UserScalarLayer`]. At call time the resolver checks the user layer first and
//! falls back to the built-in registry, giving user functions name-shadowing
//! priority while keeping the built-in fast-path (empty user layer → one hash-miss
//! on an empty `scc::HashMap` → straight to the static builtin).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use scc::hash_map::Entry;
use scc::HashMap as SccHashMap;
use shamir_collections::THasher;

use crate::registry::{FnEntry, ScalarError, ScalarResult};
use shamir_types::types::value::QueryValue;

/// Number of argon2-named calls run inline while a non-multi-thread runtime is active.
pub static ARGON2_INLINE_ON_RUNTIME: AtomicU64 = AtomicU64::new(0);
/// Whether the process-wide warning for inline runtime execution has been issued.
pub static ARGON2_INLINE_WARNING_EMITTED: AtomicBool = AtomicBool::new(false);

// -----------------------------------------------------------------
// UserScalarLayer — the per-DB user-registered scalar table.
// -----------------------------------------------------------------

/// Per-database user scalar table. Lives behind an `Arc` so a
/// [`ScalarResolver`] can hold a cheap reference on the hot filter path.
///
/// Registration takes the bucket's writer lock (`entry_sync`, rare admin
/// path); lookups take a short shared bucket read lock via `read_sync`.
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
        match self.fns.entry_sync(name.into()) {
            Entry::Occupied(mut occupied) => {
                let _ = occupied.insert(entry);
            }
            Entry::Vacant(vacant) => {
                vacant.insert_entry(entry);
            }
        }
    }

    /// Look up an entry by name.
    pub fn get(&self, name: &str) -> Option<FnEntry> {
        self.fns.read_sync(name, |_, e| e.clone())
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
        // Fast path: user layer (shared bucket read lock, typically empty → one hash-miss).
        if let Some(entry) = self.user.get(name) {
            return dispatch_entry(&entry, args);
        }
        // Fallback: built-in static registry.
        dispatch_entry(
            self.builtin
                .get(name)
                .ok_or_else(|| ScalarError::builtin("unknown_function"))?,
            args,
        )
    }

    /// Dispatch argon2-named scalars on the blocking pool on multi-thread Tokio
    /// runtimes, including user scalars shadowing the builtin. Other scalars run
    /// inline. Current-thread runtimes run inline with a once-per-process warning
    /// and counter; calls outside a runtime run inline without either.
    pub fn call_offloaded(&self, name: &str, args: &[QueryValue]) -> ScalarResult {
        if name != "crypto/argon2id" && name != "argon2id" {
            return self.call(name, args);
        }
        match tokio::runtime::Handle::try_current() {
            Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
                let resolver = self.clone();
                let name = name.to_owned();
                let args = args.to_vec();
                tokio::task::block_in_place(|| {
                    handle.block_on(tokio::task::spawn_blocking(move || {
                        resolver.call(&name, &args)
                    }))
                })
                .map_err(|_| ScalarError::new("scalar_task_failed"))?
            }
            Ok(_) => {
                ARGON2_INLINE_ON_RUNTIME.fetch_add(1, Ordering::Relaxed);
                if !ARGON2_INLINE_WARNING_EMITTED.swap(true, Ordering::Relaxed) {
                    log::warn!(
                        "argon2 scalar runs inline and may stall the current-thread runtime worker"
                    );
                }
                self.call(name, args)
            }
            Err(_) => self.call(name, args),
        }
    }

    /// Call a resolved entry only when it is trusted for functional indexing.
    pub fn call_indexable(&self, name: &str, args: &[QueryValue]) -> ScalarResult {
        let entry = self
            .get(name)
            .ok_or_else(|| ScalarError::new("unknown_function"))?;
        if !entry.is_indexable() {
            return Err(ScalarError::new("not_indexable"));
        }
        dispatch_entry(&entry, args)
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
        return Err(ScalarError::builtin("arity"));
    }
    if let Some(max) = entry.max_args {
        if args.len() > max {
            return Err(ScalarError::builtin("arity"));
        }
    }
    (entry.f)(args)
}
