//! [`NodeReplicated<T>`] — per-NUMA-node replicated read-mostly state.

use std::sync::Arc;

use arc_swap::{ArcSwap, Guard};

use crate::core::cache_padded::CachePadded;
use crate::core::node::NodeId;
use crate::core::topology::Topology;

/// Read-mostly data `T` replicated once per NUMA node.
///
/// Each node owns its own [`ArcSwap<T>`], cache-line-padded
/// ([`CachePadded`](crate::CachePadded)) so neighbouring nodes' cells never
/// share a line. A reader on node *N* loads its node-local replica with
/// [`load_local`](Self::load_local) and never touches a remote socket's memory
/// — the read-path locality that motivates NUMA replication of hot registries
/// (Drepper §5; Porobic et al., *OLTP on Hardware Islands*, VLDB'12).
///
/// # Consistency model
///
/// The write path is **copy-on-write across every replica**. [`rcu`](Self::rcu)
/// linearises the update on the node-0 cell with a CAS retry loop — the exact
/// lost-update-safe shape proven in the #292 / #304 `ArcSwap` migrations — then
/// mirrors the winning value to the remaining nodes. Between the node-0 commit
/// and the last mirror store (a few nanoseconds) different nodes may observe
/// old-vs-new: **eventual consistency**. Each mirror store re-reads node 0
/// and only completes while node 0 still holds the stored value, so a
/// publisher whose node-0 commit was superseded cannot leave an older value
/// behind. This is sound for read-mostly
/// registries (index definitions, validator bindings, interner snapshots);
/// data needing strong cross-node consistency must coordinate separately and is
/// out of scope here.
///
/// # Single-node degradation
///
/// When `topology.num_nodes() == 1` there is exactly one replica and writes
/// have no mirror loop. `load_local` still queries the topology and resolves
/// the replica before loading its `ArcSwap`; routing is constant-time.
pub struct NodeReplicated<T> {
    topology: Arc<dyn Topology>,
    /// One cell per node; `replicas.len() == max(num_nodes, 1)`.
    replicas: Box<[CachePadded<ArcSwap<T>>]>,
    /// Deterministic pause gate for the mirror regression tests; compiled
    /// out in production (no size or runtime cost).
    #[cfg(test)]
    gate: MirrorGate,
}

/// Deterministic pause gate for the mirror regression tests
/// (`cfg(test)` only): when armed, the FIRST mirror that reaches the gate
/// pauses after capturing node 0's value and before storing it into a
/// replica until released (one-shot), so a test can interleave a newer
/// publish inside that window.
#[cfg(test)]
#[derive(Default)]
struct MirrorGate {
    armed: std::sync::atomic::AtomicBool,
    paused: std::sync::atomic::AtomicBool,
    release: std::sync::atomic::AtomicBool,
}

impl<T: Send + Sync + 'static> NodeReplicated<T> {
    /// Replicate `initial` across one cell per node of `topology`. All replicas
    /// start sharing the same `Arc<T>` (no `T` clone — only the `Arc` is
    /// cloned), so `T` need not be `Clone` here.
    pub fn new(topology: Arc<dyn Topology>, initial: T) -> Self {
        let n = topology.num_nodes().max(1);
        let shared = Arc::new(initial);
        let replicas = (0..n)
            .map(|_| CachePadded::new(ArcSwap::from(Arc::clone(&shared))))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            topology,
            replicas,
            #[cfg(test)]
            gate: MirrorGate::default(),
        }
    }

    /// Number of per-node replicas (`== max(num_nodes, 1)`).
    pub fn num_replicas(&self) -> usize {
        self.replicas.len()
    }

    /// The `Topology` this instance was built against.
    pub fn topology(&self) -> &Arc<dyn Topology> {
        &self.topology
    }

    /// Load the snapshot for the **calling thread's current node**. `O(1)`:
    /// `Topology::current_node` + one `ArcSwap::load`.
    pub fn load_local(&self) -> Guard<Arc<T>> {
        self.replica(self.topology.current_node()).load()
    }

    /// Load a specific node's snapshot (cross-node inspection / staged config).
    /// An out-of-range node falls back to node 0.
    pub fn load_node(&self, node: NodeId) -> Guard<Arc<T>> {
        self.replica(node).load()
    }

    /// Publish `value` to **every** replica (copy-on-write, all nodes).
    /// Each mirror store re-reads node 0 and only completes while node 0
    /// still holds this publish's value, so a superseded publisher cannot
    /// leave an older value behind.
    pub fn store(&self, value: T) {
        self.replicas[0].0.store(Arc::new(value));
        self.mirror_from_node0();
    }

    /// Copy-on-write update applied to every replica.
    ///
    /// `f` receives the current node-0 value and returns the replacement. The
    /// update is linearised on the node-0 cell with a CAS retry loop (so
    /// concurrent `rcu` callers retry instead of clobbering each other), then
    /// the winning value is mirrored to the other nodes. Each mirror store
    /// re-reads node 0 and only completes while node 0 still holds the value
    /// it stored, so a publisher whose node-0 commit was superseded cannot
    /// overwrite a newer value on a non-zero node. `f` may run more than
    /// once under contention — it must recompute purely from its argument.
    pub fn rcu(&self, mut f: impl FnMut(&T) -> T) {
        // CAS loop on the node-0 cell — mirror of `IndexInfo::add_index` (#292).
        self.replicas[0].0.rcu(|cur| f(cur));
        self.mirror_from_node0();
    }

    /// Mirror node 0's current value to every other replica. Each store
    /// re-reads node 0 and only completes when node 0 still holds the value
    /// it stored, so a publisher whose node-0 commit was later superseded
    /// cannot leave an older value behind: the final writer per cell always
    /// stores a value read from current node 0.
    fn mirror_from_node0(&self) {
        let node0 = &self.replicas[0].0;
        for r in self.replicas.iter().skip(1) {
            loop {
                let p = node0.load_full();
                // Test seam: park the FIRST publisher that reaches here with
                // its captured value, before the replica store (the real
                // race window between reading node 0 and storing).
                #[cfg(test)]
                self.gate_pause();
                r.0.store(Arc::clone(&p));
                if Arc::ptr_eq(&node0.load_full(), &p) {
                    break;
                }
            }
        }
    }

    #[cfg(test)]
    fn gate_pause(&self) {
        if self
            .gate
            .armed
            .compare_exchange(
                true,
                false,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
        {
            self.gate
                .paused
                .store(true, std::sync::atomic::Ordering::Release);
            while !self.gate.release.load(std::sync::atomic::Ordering::Acquire) {
                std::hint::spin_loop();
            }
        }
    }

    /// Arm the gate: the next mirror pauses between its node-0 read and its replica store (one-shot).
    #[cfg(test)]
    pub(crate) fn arm_mirror_gate(&self) {
        self.gate
            .release
            .store(false, std::sync::atomic::Ordering::Release);
        self.gate
            .armed
            .store(true, std::sync::atomic::Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn wait_mirror_paused(&self) {
        while !self.gate.paused.load(std::sync::atomic::Ordering::Acquire) {
            std::hint::spin_loop();
        }
    }

    #[cfg(test)]
    pub(crate) fn release_mirror_gate(&self) {
        self.gate
            .release
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Overwrite a **single** node's replica, leaving the others untouched.
    ///
    /// This deliberately creates per-node divergence and is intended for
    /// staged per-node configuration and for tests; ordinary updates use
    /// [`store`](Self::store) / [`rcu`](Self::rcu). An out-of-range node maps
    /// to node 0.
    pub fn store_node(&self, node: NodeId, value: T) {
        self.replica(node).store(Arc::new(value));
    }

    /// Resolve a node to its replica cell, clamping out-of-range to node 0.
    fn replica(&self, node: NodeId) -> &ArcSwap<T> {
        let idx = if node.0 < self.replicas.len() {
            node.0
        } else {
            0
        };
        &self.replicas[idx].0
    }
}
