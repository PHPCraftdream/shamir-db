//! `MockTopology` — a dependency-injection double for [`Topology`] that lets
//! NUMA-aware code be tested on single-socket / Windows / CI machines.

use std::cell::Cell;
use std::sync::Mutex;
use std::thread::ThreadId;

use crate::error::AffinityError;
use crate::node::{CpuId, NodeId};
use crate::topology::Topology;

thread_local! {
    /// Per-thread "current node" override consulted by every [`MockTopology`]
    /// on the calling thread. Defaults to node 0. Set via
    /// [`MockTopology::set_current_node_for_test`].
    ///
    /// Thread-local (not a shared field) so that a test can spawn a thread,
    /// declare it "running on node N", and exercise node-local code paths
    /// deterministically — exactly how a real `sched_getcpu` would differ
    /// per thread.
    static MOCK_CURRENT_NODE: Cell<usize> = const { Cell::new(0) };
}

/// A synthetic, fully-configurable topology for tests.
///
/// * `num_nodes` and the CPUs per node are fixed at construction.
/// * The "current node" is a per-thread value the test controls via
///   [`set_current_node_for_test`](MockTopology::set_current_node_for_test) —
///   it is shared by every `MockTopology` consulted on that thread.
/// * Every successful pin is appended to a log readable via
///   [`pin_log`](MockTopology::pin_log) for assertions.
pub struct MockTopology {
    /// `cores[node]` is that node's CPU list.
    cores: Vec<Vec<CpuId>>,
    /// Recorded `(thread, node)` pairs from successful pins. A `Mutex` is
    /// sanctioned here: this is a test fixture, never on a hot path.
    pin_log: Mutex<Vec<(ThreadId, NodeId)>>,
}

impl MockTopology {
    /// `num_nodes` nodes, each owning `cpus_per_node` synthetic CPUs numbered
    /// contiguously: node `n` owns CPUs
    /// `[n * cpus_per_node, (n + 1) * cpus_per_node)`.
    ///
    /// Panics if `num_nodes == 0` — a topology always has at least one node.
    pub fn with_nodes(num_nodes: usize, cpus_per_node: usize) -> Self {
        assert!(num_nodes >= 1, "a topology must have at least one node");
        let cores = (0..num_nodes)
            .map(|n| {
                let base = n * cpus_per_node;
                (base..base + cpus_per_node).map(CpuId).collect()
            })
            .collect();
        Self {
            cores,
            pin_log: Mutex::new(Vec::new()),
        }
    }

    /// Override the current node **for the calling thread only**. Affects every
    /// `MockTopology` consulted on this thread until changed. An out-of-range
    /// value is clamped to node 0 by [`current_node`](Topology::current_node).
    pub fn set_current_node_for_test(node: NodeId) {
        MOCK_CURRENT_NODE.with(|c| c.set(node.0));
    }

    /// Snapshot of every `(thread, node)` recorded by a successful
    /// [`pin_current_thread_to_node`](Topology::pin_current_thread_to_node).
    pub fn pin_log(&self) -> Vec<(ThreadId, NodeId)> {
        self.pin_log
            .lock()
            .expect("mock pin_log mutex poisoned")
            .clone()
    }
}

impl Topology for MockTopology {
    fn num_nodes(&self) -> usize {
        self.cores.len()
    }

    fn cores_on_node(&self, node: NodeId) -> &[CpuId] {
        self.cores.get(node.0).map(Vec::as_slice).unwrap_or(&[])
    }

    fn current_node(&self) -> NodeId {
        let raw = MOCK_CURRENT_NODE.with(Cell::get);
        // Clamp so a stale thread-local from an earlier, larger mock can never
        // index past this mock's replica array.
        NodeId(if raw < self.cores.len() { raw } else { 0 })
    }

    fn pin_current_thread_to_node(&self, node: NodeId) -> Result<(), AffinityError> {
        if node.0 >= self.cores.len() {
            return Err(AffinityError::NodeOutOfRange {
                requested: node.0,
                available: self.cores.len(),
            });
        }
        // Record the pin and make the calling thread subsequently report this
        // node — mirroring how a real `sched_setaffinity` followed by
        // `sched_getcpu` would observe the move.
        self.pin_log
            .lock()
            .expect("mock pin_log mutex poisoned")
            .push((std::thread::current().id(), node));
        MOCK_CURRENT_NODE.with(|c| c.set(node.0));
        Ok(())
    }
}
