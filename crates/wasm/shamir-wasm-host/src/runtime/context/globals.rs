use shamir_collections::THasher;
use shamir_types::types::value::QueryValue;
use std::sync::atomic::{
    AtomicUsize,
    Ordering::{AcqRel, Acquire},
};
#[cfg(test)]
use std::sync::Arc;

use crate::gateway::env_policy::EnvPolicy;

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
