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

use shamir_collections::{TFxSet, THasher};
use shamir_types::access::Actor;
use shamir_types::types::value::QueryValue;
use std::sync::Arc;

use super::db_gateway::DbGateway;
use super::env_policy::EnvPolicy;
use super::net_gateway::NetGateway;
use super::registry::FunctionRegistry;

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
    data: scc::HashMap<String, QueryValue, THasher>,
}

impl BatchContext {
    /// Create an empty batch context.
    pub fn new() -> Self {
        Self {
            data: scc::HashMap::with_hasher(THasher::default()),
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
    data: scc::HashMap<String, QueryValue, THasher>,
}

impl GlobalVars {
    /// Create an empty global-vars store.
    pub fn new() -> Self {
        Self {
            data: scc::HashMap::with_hasher(THasher::default()),
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
/// Slice 8a adds an optional [`FunctionRegistry`] gateway and a recursion
/// depth counter so a function can call another registered function via
/// `ctx.call` while bounding the call-stack depth.
///
/// Slice 8b adds an optional [`DbGateway`] and a default `repo` so a
/// function can read/write database tables via `ctx.db().table(...)`.
/// When the gateway is `None`, db host imports trap with a clear error.
///
/// Slice 9 adds `secret_grants` — the list of `env.*` variable names the
/// function is allowed to read. `global_get("env.X")` returns absent when
/// `X` is not in `secret_grants`; non-`env.` globals are ungated.
///
/// R2 adds `actor` — the [`Actor`] that initiated the invocation, threaded
/// from the facade. Defaults to `Actor::System`.
#[derive(Clone)]
pub struct FnCtx {
    globals: Arc<GlobalVars>,
    registry: Option<Arc<FunctionRegistry>>,
    depth: u32,
    depth_limit: u32,
    db: Option<Arc<dyn DbGateway>>,
    repo: String,
    net: Option<Arc<dyn NetGateway>>,
    secret_grants: Arc<TFxSet<String>>,
    actor: Actor,
}

impl FnCtx {
    /// Default recursion depth limit.
    pub const DEFAULT_DEPTH_LIMIT: u32 = 32;

    /// Construct a context with fresh empty globals (standalone / back-compat).
    pub fn new() -> Self {
        Self {
            globals: Arc::new(GlobalVars::new()),
            registry: None,
            depth: 0,
            depth_limit: Self::DEFAULT_DEPTH_LIMIT,
            db: None,
            repo: String::new(),
            net: None,
            secret_grants: Arc::new(TFxSet::default()),
            actor: Actor::System,
        }
    }

    /// Construct a context wrapping existing globals.
    pub fn with_globals(globals: Arc<GlobalVars>) -> Self {
        Self {
            globals,
            registry: None,
            depth: 0,
            depth_limit: Self::DEFAULT_DEPTH_LIMIT,
            db: None,
            repo: String::new(),
            net: None,
            secret_grants: Arc::new(TFxSet::default()),
            actor: Actor::System,
        }
    }

    /// Builder: attach a function registry (call gateway).
    pub fn with_registry(mut self, registry: Arc<FunctionRegistry>) -> Self {
        self.registry = Some(registry);
        self
    }

    /// Builder: set the current recursion depth.
    pub fn with_depth(mut self, depth: u32) -> Self {
        self.depth = depth;
        self
    }

    /// Builder: set the recursion depth limit.
    pub fn with_depth_limit(mut self, limit: u32) -> Self {
        self.depth_limit = limit;
        self
    }

    /// Builder: attach a DB gateway and default repo name.
    ///
    /// The gateway routes `ctx.db().table(...)` calls through
    /// `ShamirDb::execute`. `repo` is the function's home repo (used as the
    /// default repo in all db host imports unless overridden).
    pub fn with_db(mut self, gateway: Arc<dyn DbGateway>, repo: String) -> Self {
        self.db = Some(gateway);
        self.repo = repo;
        self
    }

    /// Builder: attach a network gateway for HTTP egress (slice 8c).
    ///
    /// The gateway routes `ctx.http_fetch(req)` calls through the
    /// `NetGateway` implementation (e.g. `CurlNetGateway`).
    pub fn with_net(mut self, gateway: Arc<dyn NetGateway>) -> Self {
        self.net = Some(gateway);
        self
    }

    /// Builder: set the secret grants for `env.*` global reads (slice 9).
    ///
    /// Only env variable names listed here can be read via `global_get`
    /// with an `env.`-prefixed key. Non-`env.` globals are ungated.
    pub fn with_secret_grants(mut self, grants: impl IntoIterator<Item = String>) -> Self {
        self.secret_grants = Arc::new(grants.into_iter().collect());
        self
    }

    /// Builder: set the actor that initiated this invocation (R2).
    pub fn with_actor(mut self, actor: Actor) -> Self {
        self.actor = actor;
        self
    }

    /// The actor that initiated this invocation.
    pub fn actor(&self) -> &Actor {
        &self.actor
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

    /// Access the optional function registry (call gateway).
    pub fn registry(&self) -> Option<&Arc<FunctionRegistry>> {
        self.registry.as_ref()
    }

    /// Current recursion depth.
    pub fn depth(&self) -> u32 {
        self.depth
    }

    /// Recursion depth limit.
    pub fn depth_limit(&self) -> u32 {
        self.depth_limit
    }

    /// Access the optional DB gateway.
    pub fn db_gateway(&self) -> Option<&Arc<dyn DbGateway>> {
        self.db.as_ref()
    }

    /// Access the optional network gateway.
    pub fn net_gateway(&self) -> Option<&Arc<dyn NetGateway>> {
        self.net.as_ref()
    }

    /// The secret grants for `env.*` global reads.
    pub fn secret_grants(&self) -> &Arc<TFxSet<String>> {
        &self.secret_grants
    }

    /// The default repo name for DB operations.
    pub fn repo(&self) -> &str {
        &self.repo
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

impl std::fmt::Debug for FnCtx {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FnCtx")
            .field("depth", &self.depth)
            .field("depth_limit", &self.depth_limit)
            .field("has_registry", &self.registry.is_some())
            .field("has_db", &self.db.is_some())
            .field("repo", &self.repo)
            .field("has_net", &self.net.is_some())
            .field("secret_grants_len", &self.secret_grants.len())
            .finish()
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
