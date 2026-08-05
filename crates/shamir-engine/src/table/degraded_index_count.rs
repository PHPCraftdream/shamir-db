//! #984 — passive (push-based) degraded-index count for `/metrics`.
//!
//! [`TableManager::degraded_index_count`] walks the in-memory index
//! registries and returns how many indexes are NOT in `Ready` state. Unlike
//! [`verify`](super::doctor::TableManager::verify) (which streams the entire
//! data store to compare expected-vs-actual entry counts), this method is
//! O(number of indexes on this table) and performs **zero store reads** —
//! it inspects only the `IndexDefinition.state` /
//! `SortedIndexDefinition.state` / `IndexDescriptor.state` fields already
//! resident in memory.
//!
//! See the `observability.rs` poller in `shamir-server` for how this count
//! is aggregated up through `RepoInstance` → `ShamirDb` and bridged into the
//! `shamir_degraded_indexes_total` Prometheus gauge.

use crate::index2::state::IndexState;

use super::table_manager::TableManager;

impl TableManager {
    /// Count indexes NOT in `Ready` state across all four families
    /// (regular, unique, sorted, index2).
    ///
    /// **Zero store I/O** — reads only the in-memory `.state` fields of
    /// already-registered definitions. Cost is O(number of indexes on this
    /// table), not O(rows).
    ///
    /// Returns a single total rather than a per-family or per-state
    /// breakdown. Justification: `IndexState` has only two variants
    /// (`Ready` / `Building`), so a per-state split would just duplicate
    /// this number (`Building` count == degraded count). A per-family
    /// split (regular / unique / sorted / index2) would help an operator
    /// know *where* to look, but the pull-based `doctor::verify()` already
    /// gives the full per-family, per-index breakdown when invoked — this
    /// gauge is the cheap always-on "something is wrong" signal, not the
    /// diagnostic detail.
    pub async fn degraded_index_count(&self) -> u64 {
        let mut count = 0u64;

        // Regular (hash) indexes — in-memory iterator (yields owned clones).
        for def in self.index_manager_ref().iter_indexes() {
            if def.state != IndexState::Ready {
                count += 1;
            }
        }
        // Unique indexes — in-memory iterator (yields owned clones).
        for def in self.index_manager_ref().iter_unique_indexes() {
            if def.state != IndexState::Ready {
                count += 1;
            }
        }
        // Sorted (B-tree) indexes — in-memory RCU snapshot (yields a Vec).
        for def in self.sorted_indexes().iter_indexes() {
            if def.state != IndexState::Ready {
                count += 1;
            }
        }
        // index2 (fts / functional / vector) — in-memory async iteration
        // over the registry's `scc::HashMap`; no store I/O.
        for desc in self.index2_registry().all_descriptors().await {
            if desc.state != IndexState::Ready {
                count += 1;
            }
        }
        count
    }
}
