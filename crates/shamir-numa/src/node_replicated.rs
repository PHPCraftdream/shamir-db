//! [`NodeReplicated<T>`] — per-NUMA-node replicated read-mostly state.

use std::sync::Arc;

use arc_swap::{ArcSwap, Guard};

use crate::cache_padded::CachePadded;
use crate::node::NodeId;
use crate::topology::Topology;

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
/// old-vs-new: **eventual consistency**. This is sound for read-mostly
/// registries (index definitions, validator bindings, interner snapshots);
/// data needing strong cross-node consistency must coordinate separately and is
/// out of scope here.
///
/// # Single-node degradation
///
/// When `topology.num_nodes() == 1` there is exactly one replica,
/// [`load_local`](Self::load_local) always hits it, and [`rcu`](Self::rcu) /
/// [`store`](Self::store) update one cell — identical to a bare `ArcSwap<T>`,
/// with no mirror loop. Consumers therefore use `NodeReplicated<T>`
/// unconditionally: it costs nothing on single-socket, Windows, or CI.
pub struct NodeReplicated<T> {
    topology: Arc<dyn Topology>,
    /// One cell per node; `replicas.len() == max(num_nodes, 1)`.
    replicas: Box<[CachePadded<ArcSwap<T>>]>,
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
        Self { topology, replicas }
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
    pub fn store(&self, value: T) {
        let next = Arc::new(value);
        for r in self.replicas.iter() {
            r.0.store(Arc::clone(&next));
        }
    }

    /// Copy-on-write update applied to every replica.
    ///
    /// `f` receives the current node-0 value and returns the replacement. The
    /// update is linearised on the node-0 cell with a CAS retry loop (so
    /// concurrent `rcu` callers retry instead of clobbering each other), then
    /// the winning value is mirrored to the other nodes. `f` may run more than
    /// once under contention — it must recompute purely from its argument.
    pub fn rcu(&self, mut f: impl FnMut(&T) -> T) {
        // CAS loop on the node-0 cell — mirror of `IndexInfo::add_index` (#292).
        self.replicas[0].0.rcu(|cur| f(cur));
        if self.replicas.len() > 1 {
            // Mirror whatever node 0 now holds to the remaining nodes. Reading
            // it back (rather than capturing inside the closure) keeps the
            // mirror consistent with the value that actually won the CAS.
            let published = self.replicas[0].0.load_full();
            for r in self.replicas.iter().skip(1) {
                r.0.store(Arc::clone(&published));
            }
        }
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
