//! Durable [`ConsumedCounterStore`] backed by `fjall` (spec §6.2 NORMATIVE
//! release blocker for SESSION_RESUMPTION).
//!
//! `fjall` is an embedded LSM key-value store. Each `try_advance()` performs
//! a write that persists with `PersistMode::SyncAll` (fsync) before
//! returning `true` — so the on-disk state always reflects the highest
//! counter value the server has ever returned `true` for. Crash-restart
//! cannot replay a previously-consumed ticket because the durable counter
//! snapshot survived the crash.
//!
//! Per spec §6.2:
//! > Implementation **MUST** persist durably (fsync) before returning `true`.
//!
//! ## Atomicity
//!
//! fjall has no nested ACID transaction primitive that scopes a
//! `get → conditional-insert`. We serialise `try_advance` and `gc` through
//! a single in-process `parking_lot::Mutex` — this is replay-protection
//! infrastructure, not a hot path (one call per session resumption), so
//! the serialisation cost is irrelevant. `peek` is lock-free (read-only).
//!
//! ## Feature flag
//!
//! Enable with `--features durable-fjall`. Without the feature this module
//! compiles to nothing and only `InMemoryConsumedCounters` (in
//! `server/resume.rs`) is available.

use crate::common::time::{ns, UnixNanos};
use crate::common::types::limits;
use crate::server::resume::ConsumedCounterStore;
use fjall::{Database, Keyspace, KeyspaceCreateOptions, PersistMode};
use parking_lot::Mutex;
use std::path::Path;
use std::sync::Arc;

/// Per spec §6.2: idle entries dropped after `RESUMPTION_MAX_CHAIN_AGE`
/// (= 24h) of no activity.
const GC_IDLE_NS: u64 = 24 * ns::HOUR;

/// Single keyspace name. Key = `user_id (16) || family_id (16)` (32 bytes),
/// value = `counter (u64_be) || last_observed_at_ns (u64_be)` (16 bytes).
const COUNTERS_KEYSPACE: &str = "consumed_counters_v1";

/// Durable replay-protection counter store.
///
/// All accepted advances are persisted with `PersistMode::SyncAll` (fsync)
/// before returning, per spec §6.2 NORMATIVE.
pub struct FjallConsumedCounters {
    db: Arc<Database>,
    keyspace: Keyspace,
    /// Serialises `try_advance` and `gc` so concurrent advancers don't
    /// race on the get-then-conditional-insert sequence. Not held across
    /// the fsync — fjall's `persist` is synchronous and short.
    write_lock: Mutex<()>,
}

impl core::fmt::Debug for FjallConsumedCounters {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FjallConsumedCounters")
            .field("db", &"<fjall::Database>")
            .finish()
    }
}

impl FjallConsumedCounters {
    /// Open or create the database at `path`.
    ///
    /// On first use the counters keyspace is created. Subsequent opens
    /// reuse the existing data (counter state survives restart).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, fjall::Error> {
        let db = Database::builder(path.as_ref()).open()?;
        let keyspace = db.keyspace(COUNTERS_KEYSPACE, KeyspaceCreateOptions::default)?;
        Ok(Self {
            db: Arc::new(db),
            keyspace,
            write_lock: Mutex::new(()),
        })
    }

    fn key(user_id: &[u8; 16], family_id: &[u8; limits::TICKET_FAMILY_ID_BYTES]) -> [u8; 32] {
        let mut k = [0u8; 32];
        k[..16].copy_from_slice(user_id);
        k[16..].copy_from_slice(family_id);
        k
    }

    fn pack_value(counter: u64, last_observed_at_ns: u64) -> [u8; 16] {
        let mut v = [0u8; 16];
        v[..8].copy_from_slice(&counter.to_be_bytes());
        v[8..].copy_from_slice(&last_observed_at_ns.to_be_bytes());
        v
    }

    fn unpack_value(v: &[u8]) -> (u64, u64) {
        let mut c = [0u8; 8];
        let mut t = [0u8; 8];
        c.copy_from_slice(&v[..8]);
        t.copy_from_slice(&v[8..16]);
        (u64::from_be_bytes(c), u64::from_be_bytes(t))
    }

    /// Test helper: snapshot the current counter for a key.
    pub fn peek(
        &self,
        user_id: &[u8; 16],
        family_id: &[u8; limits::TICKET_FAMILY_ID_BYTES],
    ) -> Option<u64> {
        let key = Self::key(user_id, family_id);
        let slice = self.keyspace.get(&key[..]).ok().flatten()?;
        Some(Self::unpack_value(&slice).0)
    }
}

impl ConsumedCounterStore for FjallConsumedCounters {
    fn try_advance(
        &self,
        user_id: &[u8; 16],
        family_id: &[u8; limits::TICKET_FAMILY_ID_BYTES],
        new_counter: u64,
    ) -> bool {
        let key = Self::key(user_id, family_id);
        let now_ns = UnixNanos::now().as_u64();

        let _guard = self.write_lock.lock();

        let prior = match self.keyspace.get(&key[..]) {
            Ok(opt) => opt,
            Err(_) => return false,
        };
        let prior_counter = prior.map(|s| Self::unpack_value(&s).0);
        let accept = match prior_counter {
            Some(c) => new_counter > c,
            None => true,
        };
        if !accept {
            return false;
        }

        let v = Self::pack_value(new_counter, now_ns);
        if self.keyspace.insert(&key[..], &v[..]).is_err() {
            return false;
        }

        // Spec §6.2 NORMATIVE: fsync before returning `true`. Without
        // this the on-disk state may lag the in-memory journal and a
        // crash could replay a "consumed" ticket.
        if self.db.persist(PersistMode::SyncAll).is_err() {
            return false;
        }
        true
    }

    fn gc(&self, now_ns: u64) {
        let cutoff = now_ns.saturating_sub(GC_IDLE_NS);
        let _guard = self.write_lock.lock();

        let to_remove: Vec<[u8; 32]> = {
            let mut victims = Vec::new();
            for guard in self.keyspace.iter() {
                let Ok((k, v)) = guard.into_inner() else {
                    continue;
                };
                if k.len() != 32 || v.len() != 16 {
                    continue;
                }
                let (_, last) = Self::unpack_value(&v);
                if last < cutoff {
                    let mut kk = [0u8; 32];
                    kk.copy_from_slice(&k);
                    victims.push(kk);
                }
            }
            victims
        };

        for key in &to_remove {
            if let Err(e) = self.keyspace.remove(&key[..]) {
                log::warn!("durable_counters::gc: remove failed: {}", e);
            }
        }
        if !to_remove.is_empty() {
            if let Err(e) = self.db.persist(PersistMode::SyncAll) {
                log::warn!("durable_counters::gc: persist failed: {}", e);
            }
        }
    }
}

// Tests live in crate::server::tests::durable_counters_tests.
