use bytes::Bytes;
use scc::hash_map::Entry::{Occupied, Vacant};

use super::TxContext;
use crate::types::IsolationLevel;

impl TxContext {
    /// Record a read for SSI validation (only if Serializable).
    ///
    /// `&mut self` overload, kept for existing call sites that hold the tx
    /// mutably (tests, benches, manually-driven read tracking). Delegates to
    /// [`record_read_shared`](Self::record_read_shared); both write the same
    /// interior-mutable `read_set`.
    pub fn record_read(&mut self, table_id: u64, key: Bytes, version: u64) {
        self.record_read_shared(table_id, key, version);
    }

    /// Record a read for SSI validation (only if Serializable), taking
    /// `&self` via interior mutability (`read_set` is an `scc::HashMap`).
    ///
    /// This is the entry point the engine's tx-aware read path uses:
    /// `TableManager::read_one_tx` holds the tx by shared reference
    /// (`Option<&TxContext>`), so it cannot call the `&mut self` overload.
    /// Wiring this in is what makes Serializable isolation actually populate
    /// the read-set in production (HIGH-C) — previously `record_read` was
    /// reachable only from unit tests, so `read_set` was always empty at
    /// commit and SSI silently degraded to Snapshot isolation.
    ///
    /// No-op under Snapshot isolation.
    ///
    /// **First-read-wins**: the version recorded for a key is the one observed
    /// at the *first* read; a later re-read of the same key does NOT overwrite
    /// it. This is the load-bearing SSI semantic. Versions are monotonic, so
    /// the first read captures the lowest (earliest) version the tx ever saw —
    /// the conservative bound for conflict detection. Overwriting with a newer
    /// version (last-write-wins, the previous `HashMap::insert` behaviour, fine
    /// only while reads were recorded once from unit tests) would mask a real
    /// conflict: e.g. an update's internal old-value read runs AFTER a
    /// concurrent committer bumped the key, and last-write-wins would re-record
    /// the key at the post-commit version, defeating the abort.
    pub fn record_read_shared(&self, table_id: u64, key: Bytes, version: u64) {
        if self.isolation == IsolationLevel::Serializable {
            match self.read_set.entry_sync((table_id, key)) {
                // First-read-wins: keep the earliest observed version.
                Occupied(_) => {}
                Vacant(ve) => {
                    ve.insert_entry(version);
                }
            }
        }
    }

    /// FG-7: record a CAS check unconditionally (any isolation). Overwrite
    /// semantics (last write wins) — unlike `read_set`'s first-read-wins,
    /// there is no monotonic-conservative-bound concern here: `expected`
    /// IS the exact version this specific check demands, not an observed
    /// read version. (If the SAME key is CAS-checked twice in one tx with
    /// two different `expected` values, that is almost certainly a caller
    /// bug — the later check's `expected` is what commit validates.)
    pub fn record_cas(&self, table_id: u64, key: Bytes, expected: u64) {
        self.cas_set.upsert_sync((table_id, key), expected);
    }

    /// Record a predicate dependency for SSI phantom detection.
    ///
    /// No-op under Snapshot isolation — zero-overhead invariant: the
    /// isolation gate runs BEFORE any work. Takes `&self` via interior
    /// mutability so engine scan paths can append through `&TxContext`.
    pub fn record_predicate_shared(&self, dep: crate::predicate_set::PredicateDep) {
        if self.isolation == IsolationLevel::Serializable {
            self.predicate_set.push(dep);
        }
    }

    /// F-28 Step 5 (S3-C): require this tx's commit footprint to be
    /// published for `table_token`, regardless of isolation level.
    ///
    /// Called by the engine's insert/update/set staging path when the
    /// target table is flagged as an FK child (`FkReverseCache::is_fk_child`)
    /// — a plain Snapshot writer into such a table still needs its footprint
    /// recorded in the commit-write log so a concurrent Serializable
    /// FK-parent-delete's phantom check (Phase 2-bis) has something to
    /// conflict against. Idempotent (a `TFxSet` insert of an already-present
    /// token is a no-op). Returns `&mut Self` for builder-style chaining.
    pub fn require_footprint_for(&mut self, table_token: u64) -> &mut Self {
        self.footprint_tokens.insert(table_token);
        self
    }

    /// F-40b (RI barrier): record that this tx performed an FK reverse-check
    /// scan of `table_token` (a child table), **regardless of isolation
    /// level**. Called from `fk_restrict.rs::child_has_reference` (and, in
    /// Step 2, the cascade / on-update probes). At commit time, Phase 2-bis
    /// re-checks every recorded token against the commit-write log so a
    /// concurrent committer that touched the child table aborts this tx —
    /// closing the cross-transaction FK TOCTOU race for EXPLICIT
    /// `Snapshot`-isolation parent mutations (which never record a
    /// Serializable `predicate_set` entry).
    ///
    /// Takes `&self` via interior mutability so the FK scan path (which holds
    /// the tx by shared reference) can record without a signature break.
    /// Idempotent — inserting an already-present token is a no-op (the
    /// `Err` from `scc::HashSet::insert_sync` on a duplicate is discarded).
    pub fn record_ri_barrier(&self, table_token: u64) {
        let _ = self.ri_barrier_tokens.insert_sync(table_token);
    }

    /// F-40b: true when no RI barrier tokens have been recorded. The
    /// zero-overhead gate on the commit path (Phase 2-bis and the
    /// commit-lock acquisition both check this before doing any work).
    pub fn ri_barrier_tokens_is_empty(&self) -> bool {
        self.ri_barrier_tokens.is_empty()
    }

    /// F-40b: append one `PredicateDep::TableScan { table_token }` per
    /// recorded barrier token into `deps`. Used by Phase 2-bis to build the
    /// unified predicate-deps slice passed to `predicate_conflicts_batch`.
    /// The caller has already verified non-emptiness via
    /// [`ri_barrier_tokens_is_empty`](Self::ri_barrier_tokens_is_empty).
    pub fn append_ri_barrier_deps(&self, deps: &mut Vec<crate::predicate_set::PredicateDep>) {
        self.ri_barrier_tokens.iter_sync(|&token| {
            deps.push(crate::predicate_set::PredicateDep::TableScan { table_token: token });
            true
        });
    }

    /// Validate the read-set against current committed versions.
    ///
    /// For Serializable Snapshot Isolation: every key the tx read must
    /// still be at the same version when we're about to commit. If any
    /// key has advanced, another tx wrote there → abort with the
    /// offending key.
    ///
    /// `version_provider(table_id, key) -> Option<u64>` is supplied by
    /// the caller. `None` = unknown table → conflict. `Some(0)` is the
    /// safe default for registered tables where the key has never been
    /// written (0 <= any version_seen → passes).
    pub fn validate_read_set<F>(&self, mut version_provider: F) -> Result<(), (u64, Bytes)>
    where
        F: FnMut(u64, &Bytes) -> Option<u64>,
    {
        // `iter_sync` short-circuits: the visitor returns `false` once the
        // first conflict is recorded, so at most one conflict is captured.
        // Iteration order is unspecified (it was already with
        // `std::HashMap`), so which key surfaces on a multi-key conflict is
        // not contractual — callers test single-key scenarios.
        let mut conflict: Option<(u64, Bytes)> = None;
        self.read_set.iter_sync(|(table_id, key), version_seen| {
            if conflict.is_some() {
                return false;
            }
            match version_provider(*table_id, key) {
                None => conflict = Some((*table_id, key.clone())),
                Some(current) if current > *version_seen => {
                    conflict = Some((*table_id, key.clone()));
                }
                Some(_) => {}
            }
            true
        });
        match conflict {
            Some(c) => Err(c),
            None => Ok(()),
        }
    }
}
