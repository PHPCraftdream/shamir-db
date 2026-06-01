//! Execution-context handles for the function engine.
//!
//! Slice 1 kept these minimal; slice 5 adds the real bodies:
//!
//! * [`FnCtx`] carries database-global variables (process-lifetime, shared
//!   across batches) via [`GlobalVars`].
//! * [`FnBatch`] carries a per-batch scratchpad ([`BatchContext`]) so
//!   operations/functions in one batch can leave traces for each other.
//!
//! The `new()` constructors remain and produce usable standalone contexts
//! (fresh empty stores), preserving back-compat with every call site that
//! existed when these were placeholder structs.
//!
//! Database-access helpers (`db()`, `repo()`, `store()`) arrive in a later
//! slice.

use shamir_types::types::value::QueryValue;
use std::sync::Arc;

use super::env_policy::EnvPolicy;

// ── BatchContext ──────────────────────────────────────────────────────

/// Per-batch shared scratchpad.
///
/// Functions executing inside the same batch exchange intermediate values
/// through this map. Wrapped in `Arc` so all functions in the batch share
/// one instance while the `FnBatch` handle is cloned freely.
///
/// Uses `scc::HashMap` (lock-free, CAS-based) per the engine concurrency
/// invariants — no `std::sync::Mutex` / `RwLock` / `parking_lot`.
pub struct BatchContext {
    data: scc::HashMap<String, QueryValue>,
}

impl BatchContext {
    /// Create an empty batch context.
    pub fn new() -> Self {
        Self {
            data: scc::HashMap::new(),
        }
    }

    /// Insert or replace a value.
    pub fn put(&self, key: impl Into<String>, value: QueryValue) {
        let key = key.into();
        let _ = self.data.remove(&key);
        let _ = self.data.insert(key, value);
    }

    /// Read a value (cloned out). Returns `None` if absent.
    pub fn get(&self, key: &str) -> Option<QueryValue> {
        self.data.read(key, |_, v| v.clone())
    }

    /// Whether the key exists.
    pub fn contains(&self, key: &str) -> bool {
        self.data.contains(key)
    }

    /// All keys (snapshot).
    pub fn keys(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.data.scan(|k, _| out.push(k.clone()));
        out
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether the context is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Atomically read-modify-write a key using scc's entry API.
    ///
    /// The closure receives the current value (`None` if absent) and must
    /// return the new value. The entry is held for the duration of the
    /// closure, so concurrent updates to the same key do not race.
    pub fn update<F: FnOnce(Option<QueryValue>) -> QueryValue>(
        &self,
        key: &str,
        f: F,
    ) -> QueryValue {
        match self.data.entry(key.to_string()) {
            scc::hash_map::Entry::Occupied(mut occ) => {
                let old = occ.get().clone();
                let new = f(Some(old));
                *occ.get_mut() = new.clone();
                new
            }
            scc::hash_map::Entry::Vacant(vac) => {
                let new = f(None);
                vac.insert_entry(new.clone());
                new
            }
        }
    }

    /// Atomic integer increment: missing/non-Int treated as 0.
    ///
    /// Returns the new value after adding `delta`.
    pub fn incr(&self, key: &str, delta: i64) -> i64 {
        let new = self.update(key, |old| match old {
            Some(QueryValue::Int(n)) => QueryValue::Int(n + delta),
            _ => QueryValue::Int(delta),
        });
        match new {
            QueryValue::Int(n) => n,
            _ => unreachable!(),
        }
    }
}

impl std::fmt::Debug for BatchContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BatchContext")
            .field("len", &self.data.len())
            .finish()
    }
}

impl Default for BatchContext {
    fn default() -> Self {
        Self::new()
    }
}

// ── GlobalVars ────────────────────────────────────────────────────────

/// Database-global variables (process-lifetime, shared across all batches).
///
/// In-memory only for now. Durable globals backed by the system store are
/// a follow-up.
///
/// Uses `scc::HashMap` (lock-free, CAS-based) per the engine concurrency
/// invariants.
pub struct GlobalVars {
    data: scc::HashMap<String, QueryValue>,
}

impl GlobalVars {
    /// Create an empty global-vars store.
    pub fn new() -> Self {
        Self {
            data: scc::HashMap::new(),
        }
    }

    /// Insert or replace a global variable.
    pub fn set(&self, key: impl Into<String>, value: QueryValue) {
        let key = key.into();
        let _ = self.data.remove(&key);
        let _ = self.data.insert(key, value);
    }

    /// Read a global variable (cloned out). Returns `None` if absent.
    pub fn get(&self, key: &str) -> Option<QueryValue> {
        self.data.read(key, |_, v| v.clone())
    }

    /// Remove a global variable. Returns `true` if it existed.
    pub fn remove(&self, key: &str) -> bool {
        self.data.remove(key).is_some()
    }

    /// Whether the key exists.
    pub fn contains(&self, key: &str) -> bool {
        self.data.contains(key)
    }

    /// All keys (snapshot).
    pub fn keys(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.data.scan(|k, _| out.push(k.clone()));
        out
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Seed OS environment variables into the `env.*` namespace.
    ///
    /// For each `(name, value)` pair where `policy.includes(&name)`,
    /// inserts key `env.{name}` with value `QueryValue::Str(value)`.
    pub fn seed_env(&self, policy: &EnvPolicy) {
        for (name, value) in std::env::vars() {
            if policy.includes(&name) {
                self.set(format!("env.{}", name), QueryValue::Str(value));
            }
        }
    }

    /// Atomically read-modify-write a key using scc's entry API.
    ///
    /// The closure receives the current value (`None` if absent) and must
    /// return the new value. The entry is held for the duration of the
    /// closure, so concurrent updates to the same key do not race.
    pub fn update<F: FnOnce(Option<QueryValue>) -> QueryValue>(
        &self,
        key: &str,
        f: F,
    ) -> QueryValue {
        match self.data.entry(key.to_string()) {
            scc::hash_map::Entry::Occupied(mut occ) => {
                let old = occ.get().clone();
                let new = f(Some(old));
                *occ.get_mut() = new.clone();
                new
            }
            scc::hash_map::Entry::Vacant(vac) => {
                let new = f(None);
                vac.insert_entry(new.clone());
                new
            }
        }
    }

    /// Atomic integer increment: missing/non-Int treated as 0.
    ///
    /// Returns the new value after adding `delta`.
    pub fn incr(&self, key: &str, delta: i64) -> i64 {
        let new = self.update(key, |old| match old {
            Some(QueryValue::Int(n)) => QueryValue::Int(n + delta),
            _ => QueryValue::Int(delta),
        });
        match new {
            QueryValue::Int(n) => n,
            _ => unreachable!(),
        }
    }
}

impl std::fmt::Debug for GlobalVars {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlobalVars")
            .field("len", &self.data.len())
            .finish()
    }
}

impl Default for GlobalVars {
    fn default() -> Self {
        Self::new()
    }
}

// ── FnCtx ─────────────────────────────────────────────────────────────

/// Access to the DBMS on the current transaction.
///
/// Carries database-global variables shared across all batches for the
/// lifetime of the database process. The `new()` constructor creates a
/// fresh standalone context (empty globals) for backward compatibility.
///
/// Database-access helpers (`db()`, `repo()`, `store()`) arrive in a
/// later slice.
#[derive(Debug, Clone)]
pub struct FnCtx {
    globals: Arc<GlobalVars>,
}

impl FnCtx {
    /// Construct a context with fresh empty globals (standalone / back-compat).
    pub fn new() -> Self {
        Self {
            globals: Arc::new(GlobalVars::new()),
        }
    }

    /// Construct a context wrapping existing globals.
    pub fn with_globals(globals: Arc<GlobalVars>) -> Self {
        Self { globals }
    }

    /// Read a global variable (cloned out).
    pub fn global_get(&self, key: &str) -> Option<QueryValue> {
        self.globals.get(key)
    }

    /// Set a global variable.
    pub fn global_set(&self, key: impl Into<String>, value: QueryValue) {
        self.globals.set(key, value);
    }

    /// Remove a global variable. Returns `true` if it existed.
    pub fn global_remove(&self, key: &str) -> bool {
        self.globals.remove(key)
    }

    /// All global variable keys (snapshot).
    pub fn global_keys(&self) -> Vec<String> {
        self.globals.keys()
    }

    /// Access the underlying globals store.
    pub fn globals(&self) -> &Arc<GlobalVars> {
        &self.globals
    }

    /// Atomic integer increment on a global variable.
    ///
    /// Missing/non-Int treated as 0. Returns the new value.
    pub fn global_incr(&self, key: &str, delta: i64) -> i64 {
        self.globals.incr(key, delta)
    }

    /// Atomic read-modify-write on a global variable.
    pub fn global_update<F: FnOnce(Option<QueryValue>) -> QueryValue>(
        &self,
        key: &str,
        f: F,
    ) -> QueryValue {
        self.globals.update(key, f)
    }
}

impl Default for FnCtx {
    fn default() -> Self {
        Self::new()
    }
}

// ── FnBatch ───────────────────────────────────────────────────────────

/// The batch the function executes within — read aliases, append ops,
/// and a per-batch scratchpad for inter-function data exchange.
///
/// The `new()` constructor creates a fresh standalone batch (empty context)
/// for backward compatibility.
#[derive(Debug, Clone)]
pub struct FnBatch {
    context: Arc<BatchContext>,
}

impl FnBatch {
    /// Construct a batch with a fresh empty context (standalone / back-compat).
    pub fn new() -> Self {
        Self {
            context: Arc::new(BatchContext::new()),
        }
    }

    /// Construct a batch wrapping an existing batch context.
    pub fn with_context(context: Arc<BatchContext>) -> Self {
        Self { context }
    }

    /// Write a value into the batch scratchpad.
    pub fn put(&self, key: impl Into<String>, value: QueryValue) {
        self.context.put(key, value);
    }

    /// Read a value from the batch scratchpad (cloned out).
    pub fn get(&self, key: &str) -> Option<QueryValue> {
        self.context.get(key)
    }

    /// All scratchpad keys (snapshot).
    pub fn keys(&self) -> Vec<String> {
        self.context.keys()
    }

    /// Whether the key exists in the scratchpad.
    pub fn contains(&self, key: &str) -> bool {
        self.context.contains(key)
    }

    /// Access the underlying batch context.
    pub fn context(&self) -> &Arc<BatchContext> {
        &self.context
    }
}

impl Default for FnBatch {
    fn default() -> Self {
        Self::new()
    }
}
