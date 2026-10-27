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

#[cfg(test)]
use crate::wasm::FuelObserver;
use shamir_collections::{TFxSet, THasher};
use shamir_types::access::Actor;
use shamir_types::types::value::QueryValue;
use std::sync::atomic::{
    AtomicUsize,
    Ordering::{AcqRel, Acquire},
};
use std::sync::Arc;

use super::registry::FunctionRegistry;
use crate::gateway::db_gateway::DbGateway;
use crate::gateway::env_policy::EnvPolicy;
use crate::gateway::net_gateway::NetGateway;

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
        match self.data.entry_sync(key) {
            scc::hash_map::Entry::Occupied(mut occupied) => *occupied.get_mut() = value,
            scc::hash_map::Entry::Vacant(vacant) => {
                vacant.insert_entry(value);
            }
        }
    }

    /// Read a value (cloned out). Returns `None` if absent.
    pub fn get(&self, key: &str) -> Option<QueryValue> {
        self.data.read_sync(key, |_, v| v.clone())
    }

    /// Whether the key exists.
    pub fn contains(&self, key: &str) -> bool {
        self.data.contains_sync(key)
    }

    /// All keys (snapshot).
    pub fn keys(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.data.iter_sync(|k, _| {
            out.push(k.clone());
            true
        });
        out
    }

    /// Number of entries.
    #[allow(clippy::disallowed_methods)] // O(N) ack: cardinality accessor, off hot path
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
        match self.data.entry_sync(key.to_string()) {
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
    #[allow(clippy::disallowed_methods)] // O(N) ack: Debug formatting, off hot path
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalsQuota {
    pub max_entries: usize,
    pub max_bytes: usize,
    pub max_value_bytes: usize,
}
impl Default for GlobalsQuota {
    fn default() -> Self {
        Self {
            max_entries: 16_384,
            max_bytes: 64 * 1024 * 1024,
            max_value_bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GlobalsQuotaError {
    #[error("globals quota exceeded: value is {len} bytes, max {max}")]
    ValueTooLarge { len: usize, max: usize },
    #[error("globals quota exceeded: entry limit {max}")]
    Entries { max: usize },
    #[error("globals quota exceeded: byte limit {max}")]
    Bytes { max: usize },
}

struct Slot {
    value: QueryValue,
    cost: usize,
}
const SLOT_OVERHEAD: usize = std::mem::size_of::<Slot>() + std::mem::size_of::<String>();
#[derive(Clone, Copy, Default)]
struct Charge {
    entries: usize,
    bytes: usize,
}
enum Admission {
    Capped,
    Uncapped,
}
#[cfg(test)]
pub(crate) type AdmitProbe = Arc<dyn Fn(&str) + Send + Sync>;

fn slot_cost(key: &str, n: usize) -> usize {
    key.len().saturating_add(n).saturating_add(SLOT_OVERHEAD)
}
fn encoded_len(value: &QueryValue) -> usize {
    value.to_bytes().map_or(0, |bytes| bytes.len())
}
fn add_capped(counter: &AtomicUsize, amount: usize, max: usize) -> bool {
    amount == 0
        || counter
            .fetch_update(AcqRel, Acquire, |current| {
                current.checked_add(amount).filter(|next| *next <= max)
            })
            .is_ok()
}
fn add_saturating(counter: &AtomicUsize, amount: usize) {
    if amount != 0 {
        let _ = counter.fetch_update(AcqRel, Acquire, |current| {
            Some(current.saturating_add(amount))
        });
    }
}
fn sub_saturating(counter: &AtomicUsize, amount: usize) {
    if amount != 0 {
        let _ = counter.fetch_update(AcqRel, Acquire, |current| {
            Some(current.saturating_sub(amount))
        });
    }
}
impl Charge {
    fn growth(old: Option<usize>, cost: usize) -> Self {
        match old {
            None => Self {
                entries: 1,
                bytes: cost,
            },
            Some(old) => Self {
                entries: 0,
                bytes: cost.saturating_sub(old),
            },
        }
    }
    fn covers(self, need: Self) -> bool {
        self.entries >= need.entries && self.bytes >= need.bytes
    }
    fn surplus(self, need: Self) -> Self {
        Self {
            entries: self.entries.saturating_sub(need.entries),
            bytes: self.bytes.saturating_sub(need.bytes),
        }
    }
}

/// Database-global variables (process-lifetime, shared across all batches).
///
/// In-memory only for now. Durable globals backed by the system store are
/// a follow-up.
///
/// Uses `scc::HashMap` (lock-free, CAS-based) per the engine concurrency
/// invariants.
pub struct GlobalVars {
    data: scc::HashMap<String, Slot, THasher>,
    entries: AtomicUsize,
    bytes: AtomicUsize,
    quota: GlobalsQuota,
    #[cfg(test)]
    admit_probe: Option<AdmitProbe>,
}

impl GlobalVars {
    /// Create an empty global-vars store.
    pub fn new() -> Self {
        Self::with_quota(GlobalsQuota::default())
    }

    pub fn with_quota(quota: GlobalsQuota) -> Self {
        Self {
            data: scc::HashMap::with_hasher(THasher::default()),
            entries: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
            quota,
            #[cfg(test)]
            admit_probe: None,
        }
    }

    pub fn quota(&self) -> GlobalsQuota {
        self.quota
    }

    #[cfg(test)]
    pub(crate) fn with_admit_probe(mut self, probe: AdmitProbe) -> Self {
        self.admit_probe = Some(probe);
        self
    }

    #[cfg(test)]
    pub(crate) fn bytes_used(&self) -> usize {
        self.bytes.load(Acquire)
    }

    fn reserve(&self, charge: Charge) -> Result<(), GlobalsQuotaError> {
        if !add_capped(&self.entries, charge.entries, self.quota.max_entries) {
            return Err(GlobalsQuotaError::Entries {
                max: self.quota.max_entries,
            });
        }
        if !add_capped(&self.bytes, charge.bytes, self.quota.max_bytes) {
            sub_saturating(&self.entries, charge.entries);
            return Err(GlobalsQuotaError::Bytes {
                max: self.quota.max_bytes,
            });
        }
        Ok(())
    }

    fn charge(&self, charge: Charge) {
        add_saturating(&self.entries, charge.entries);
        add_saturating(&self.bytes, charge.bytes);
    }

    fn release(&self, charge: Charge) {
        sub_saturating(&self.entries, charge.entries);
        sub_saturating(&self.bytes, charge.bytes);
    }

    fn admit(&self, key: String, mut slot: Slot, mode: Admission) -> Result<(), GlobalsQuotaError> {
        loop {
            let observed = self.data.read_sync(&key, |_, existing| existing.cost);
            let held = Charge::growth(observed, slot.cost);
            match mode {
                Admission::Capped => self.reserve(held)?,
                Admission::Uncapped => self.charge(held),
            }
            #[cfg(test)]
            if let Some(probe) = &self.admit_probe {
                probe(&key);
            }
            match self.publish(key.clone(), slot, held) {
                Ok(release) => {
                    self.release(release);
                    return Ok(());
                }
                Err(returned) => {
                    self.release(held);
                    slot = returned;
                }
            }
        }
    }

    /// Publish under the entry guard only when the reservation covers actual growth.
    fn publish(&self, key: String, slot: Slot, held: Charge) -> Result<Charge, Slot> {
        match self.data.entry_sync(key) {
            scc::hash_map::Entry::Occupied(mut entry) => {
                let old = entry.get().cost;
                let need = Charge::growth(Some(old), slot.cost);
                if !held.covers(need) {
                    return Err(slot);
                }
                let shrink = old.saturating_sub(slot.cost);
                *entry.get_mut() = slot;
                let mut release = held.surplus(need);
                release.bytes = release.bytes.saturating_add(shrink);
                Ok(release)
            }
            scc::hash_map::Entry::Vacant(entry) => {
                let need = Charge::growth(None, slot.cost);
                if !held.covers(need) {
                    return Err(slot);
                }
                entry.insert_entry(slot);
                Ok(held.surplus(need))
            }
        }
    }

    /// Insert or replace a global variable.
    pub fn set(&self, key: impl Into<String>, value: QueryValue) {
        let key = key.into();
        let cost = slot_cost(&key, encoded_len(&value));
        let _ = self.admit(key, Slot { value, cost }, Admission::Uncapped);
    }

    pub fn try_set(
        &self,
        key: impl Into<String>,
        value: QueryValue,
        encoded_len: usize,
    ) -> Result<(), GlobalsQuotaError> {
        if encoded_len > self.quota.max_value_bytes {
            return Err(GlobalsQuotaError::ValueTooLarge {
                len: encoded_len,
                max: self.quota.max_value_bytes,
            });
        }
        let key = key.into();
        let cost = slot_cost(&key, encoded_len);
        self.admit(key, Slot { value, cost }, Admission::Capped)
    }

    /// Read a global variable (cloned out). Returns `None` if absent.
    pub fn get(&self, key: &str) -> Option<QueryValue> {
        self.data.read_sync(key, |_, slot| slot.value.clone())
    }

    /// Remove a global variable. Returns `true` if it existed.
    pub fn remove(&self, key: &str) -> bool {
        if let Some((_, slot)) = self.data.remove_sync(key) {
            self.release(Charge {
                entries: 1,
                bytes: slot.cost,
            });
            true
        } else {
            false
        }
    }

    /// Whether the key exists.
    pub fn contains(&self, key: &str) -> bool {
        self.data.contains_sync(key)
    }

    /// All keys (snapshot).
    pub fn keys(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.data.iter_sync(|k, _| {
            out.push(k.clone());
            true
        });
        out
    }

    /// O(1); includes in-flight guest admissions; exact when quiescent.
    pub fn len(&self) -> usize {
        self.entries.load(Acquire)
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
        for (name, value) in std::env::vars_os() {
            let (Ok(name), Ok(value)) = (name.into_string(), value.into_string()) else {
                continue;
            };
            if policy.includes(&name) {
                self.set(format!("env.{name}"), QueryValue::Str(value));
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
        match self.data.entry_sync(key.to_string()) {
            scc::hash_map::Entry::Occupied(mut entry) => {
                let old_cost = entry.get().cost;
                let old = entry.get().value.clone();
                let new = f(Some(old));
                let cost = slot_cost(key, encoded_len(&new));
                self.charge(Charge::growth(Some(old_cost), cost));
                *entry.get_mut() = Slot {
                    value: new.clone(),
                    cost,
                };
                drop(entry);
                self.release(Charge {
                    entries: 0,
                    bytes: old_cost.saturating_sub(cost),
                });
                new
            }
            scc::hash_map::Entry::Vacant(entry) => {
                let new = f(None);
                let cost = slot_cost(key, encoded_len(&new));
                self.charge(Charge {
                    entries: 1,
                    bytes: cost,
                });
                entry.insert_entry(Slot {
                    value: new.clone(),
                    cost,
                });
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
    #[allow(clippy::disallowed_methods)] // O(N) ack: Debug formatting, off hot path
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
/// function is allowed to read. This is enforced at the WASM host-import
/// boundary: the `global_get` host import returns absent for `env.X` when
/// `X` is not in `secret_grants` (see `wasm/bindings/globals.rs`). The
/// native Rust `FnCtx::global_get` performs NO secret-grant filtering and
/// is for trusted native callers; non-`env.` globals are ungated.
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
    /// Shared FREE-capacity pool of the exclusive fuel-lease accounting
    /// (P0.1): consumption leaves the pool at reservation time; unused fuel
    /// is refunded on settlement. NOT "remaining fuel including outstanding
    /// Store leases".
    fuel_budget: Option<Arc<std::sync::atomic::AtomicU64>>,
    /// Test-only observability of the fuel-lease protocol (P0.1), threaded
    /// parent→child exactly like the fuel pool itself.
    #[cfg(test)]
    fuel_observer: Option<Arc<FuelObserver>>,
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
            fuel_budget: None,
            #[cfg(test)]
            fuel_observer: None,
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
            fuel_budget: None,
            #[cfg(test)]
            fuel_observer: None,
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
    /// These grants are enforced at the WASM host-import boundary (`global_get`
    /// host import): only env variable names listed here can be read by a guest
    /// via an `env.`-prefixed key. The native Rust `FnCtx::global_get` does NOT
    /// filter on grants. Non-`env.` globals are ungated.
    pub fn with_secret_grants(mut self, grants: impl IntoIterator<Item = String>) -> Self {
        self.secret_grants = Arc::new(grants.into_iter().collect());
        self
    }

    /// Builder: set the actor that initiated this invocation (R2).
    pub fn with_actor(mut self, actor: Actor) -> Self {
        self.actor = actor;
        self
    }

    /// Builder: attach a shared aggregate fuel-budget counter (task #612).
    /// Used internally by `host_call.rs` to thread the SAME counter into a
    /// nested call's `FnCtx` — do not call this from outside `shamir-wasm-host`.
    pub(crate) fn with_fuel_budget(mut self, budget: Arc<std::sync::atomic::AtomicU64>) -> Self {
        self.fuel_budget = Some(budget);
        self
    }

    /// The aggregate fuel-budget counter, if one has been seeded (always
    /// `Some` once a top-level `WasmFunction::call` has run at least once
    /// for this ctx chain).
    pub(crate) fn fuel_budget(&self) -> Option<&Arc<std::sync::atomic::AtomicU64>> {
        self.fuel_budget.as_ref()
    }

    /// Test-only: attach the per-invocation fuel-lease observer, threaded
    /// into every nested child `FnCtx` by `host_call`.
    #[cfg(test)]
    pub(crate) fn with_fuel_observer(mut self, observer: Arc<FuelObserver>) -> Self {
        self.fuel_observer = Some(observer);
        self
    }

    /// Test-only: the per-invocation fuel-lease observer, if attached.
    #[cfg(test)]
    pub(crate) fn fuel_observer(&self) -> Option<&Arc<FuelObserver>> {
        self.fuel_observer.as_ref()
    }

    /// The actor that initiated this invocation.
    pub fn actor(&self) -> &Actor {
        &self.actor
    }

    /// Read a global variable (cloned out). Performs NO secret-grant
    /// filtering — `secret_grants` are enforced only at the WASM host-import
    /// boundary; this API is for trusted native callers.
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
