//! Durable [`ConsumedCounterStore`] backed by `redb` (spec §6.2 NORMATIVE
//! release blocker for SESSION_RESUMPTION).
//!
//! `redb` provides a single-file embedded key-value store with ACID
//! guarantees. Each `try_advance()` performs a write transaction that
//! commits before returning `true` — so the on-disk state always reflects
//! the highest counter value the server has ever returned `true` for.
//! Crash-restart cannot replay a previously-consumed ticket because the
//! durable counter snapshot survived the crash.
//!
//! Per spec §6.2:
//! > Implementation **MUST** persist durably (fsync) before returning `true`.
//!
//! `redb` calls `fsync` on commit (`Durability::Immediate` is the default).
//! We make the durability level explicit + verify with a crash-restart
//! test.
//!
//! ## Feature flag
//!
//! Enable with `--features durable-redb`. Without the feature this module
//! compiles to nothing and only `InMemoryConsumedCounters` (in
//! `server/resume.rs`) is available.

use crate::common::time::{ns, UnixNanos};
use crate::common::types::limits;
use crate::server::resume::ConsumedCounterStore;
use redb::{Database, Durability, ReadableDatabase, ReadableTable, TableDefinition};
use std::path::Path;
use std::sync::Arc;

/// Per spec §6.2: idle entries dropped after `RESUMPTION_MAX_CHAIN_AGE`
/// (= 24h) of no activity.
const GC_IDLE_NS: u64 = 24 * ns::HOUR;

/// Table layout: key = `user_id (16) || family_id (16)` (32 bytes total),
/// value = `counter (u64_be) || last_observed_at_ns (u64_be)` (16 bytes).
const COUNTERS_TABLE: TableDefinition<&[u8; 32], &[u8; 16]> =
    TableDefinition::new("consumed_counters_v1");

/// Durable replay-protection counter store.
///
/// All operations commit synchronously with `Durability::Immediate` (fsync)
/// per spec §6.2 NORMATIVE.
pub struct RedbConsumedCounters {
    db: Arc<Database>,
}

impl core::fmt::Debug for RedbConsumedCounters {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RedbConsumedCounters")
            .field("db", &"<redb::Database>")
            .finish()
    }
}

impl RedbConsumedCounters {
    /// Open or create the database file at `path`.
    ///
    /// On first use the table is created. Subsequent opens reuse the
    /// existing data (counter state survives restart).
    pub fn open(path: impl AsRef<Path>) -> Result<Self, redb::Error> {
        let db = Database::create(path)?;
        // Ensure the table exists.
        let txn = db.begin_write()?;
        {
            let _t = txn.open_table(COUNTERS_TABLE)?;
        }
        txn.commit()?;
        Ok(Self { db: Arc::new(db) })
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

    fn unpack_value(v: &[u8; 16]) -> (u64, u64) {
        let mut c = [0u8; 8];
        let mut t = [0u8; 8];
        c.copy_from_slice(&v[..8]);
        t.copy_from_slice(&v[8..]);
        (u64::from_be_bytes(c), u64::from_be_bytes(t))
    }

    /// Test helper: snapshot the current counter for a key.
    pub fn peek(
        &self,
        user_id: &[u8; 16],
        family_id: &[u8; limits::TICKET_FAMILY_ID_BYTES],
    ) -> Option<u64> {
        let txn = self.db.begin_read().ok()?;
        let table = txn.open_table(COUNTERS_TABLE).ok()?;
        let key = Self::key(user_id, family_id);
        let entry = table.get(&key).ok().flatten()?;
        let v = entry.value();
        Some(Self::unpack_value(v).0)
    }
}

impl ConsumedCounterStore for RedbConsumedCounters {
    fn try_advance(
        &self,
        user_id: &[u8; 16],
        family_id: &[u8; limits::TICKET_FAMILY_ID_BYTES],
        new_counter: u64,
    ) -> bool {
        let key = Self::key(user_id, family_id);
        let now_ns = UnixNanos::now().as_u64();

        // Spec §6.2: Durability::Immediate ensures fsync before commit
        // returns. This is the default in redb 3.x but we set it explicitly
        // for documentation + future-proofing if the default ever changes.
        let mut txn = match self.db.begin_write() {
            Ok(t) => t,
            Err(_) => return false,
        };
        txn.set_durability(Durability::Immediate).ok();

        let accepted = {
            let mut table = match txn.open_table(COUNTERS_TABLE) {
                Ok(t) => t,
                Err(_) => return false,
            };
            let prior = match table.get(&key) {
                Ok(opt) => opt,
                Err(_) => return false,
            };
            let prior_counter = prior.map(|v| Self::unpack_value(v.value()).0);
            let accept = match prior_counter {
                Some(c) => new_counter > c,
                None => true,
            };
            if accept {
                let v = Self::pack_value(new_counter, now_ns);
                if table.insert(&key, &v).is_err() {
                    return false;
                }
            }
            accept
        };

        if txn.commit().is_err() {
            return false;
        }
        accepted
    }

    fn gc(&self, now_ns: u64) {
        let cutoff = now_ns.saturating_sub(GC_IDLE_NS);
        let txn = match self.db.begin_write() {
            Ok(t) => t,
            Err(_) => return,
        };
        let to_remove: Vec<[u8; 32]> = {
            let table = match txn.open_table(COUNTERS_TABLE) {
                Ok(t) => t,
                Err(_) => return,
            };
            let mut victims = Vec::new();
            if let Ok(iter) = table.iter() {
                for entry in iter.flatten() {
                    let v = entry.1.value();
                    let (_, last) = Self::unpack_value(v);
                    if last < cutoff {
                        let k = entry.0.value();
                        victims.push(*k);
                    }
                }
            }
            victims
        };
        if !to_remove.is_empty() {
            if let Ok(mut table) = txn.open_table(COUNTERS_TABLE) {
                for key in &to_remove {
                    if let Err(e) = table.remove(key) {
                        log::warn!("durable_counters::gc: remove failed: {}", e);
                    }
                }
            }
        }
        if let Err(e) = txn.commit() {
            log::warn!("durable_counters::gc: commit failed: {}", e);
        }
    }
}

// Tests live in crate::server::tests::durable_counters_tests.
