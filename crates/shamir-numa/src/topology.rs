//! The [`Topology`] trait — the single point of indirection that makes
//! NUMA-aware code testable without real multi-socket hardware.

use crate::error::AffinityError;
use crate::node::{CpuId, NodeId};

/// A read-only view of the machine's NUMA topology plus best-effort thread
/// pinning.
///
/// Production code depends on `Arc<dyn Topology>`; tests inject
/// [`MockTopology`](crate::MockTopology). The real source of truth on Linux is
/// `/sys/devices/system/node/` for discovery (Drepper §5.3) and
/// `sched_setaffinity(2)` for pinning (Drepper §5.2). Every non-Linux platform
/// and every single-socket box uses
/// [`FallbackSingleNodeTopology`](crate::FallbackSingleNodeTopology), which
/// reports a single UMA node.
///
/// Implementations must be cheap to query — `num_nodes` / `cores_on_node` are
/// read on construction paths, and `current_node` may be consulted on the read
/// hot path by [`NodeReplicated::load_local`](crate::NodeReplicated::load_local).
pub trait Topology: Send + Sync {
    /// Number of NUMA nodes. Always `>= 1`; a single-socket system reports 1.
    fn num_nodes(&self) -> usize;

    /// The logical CPUs that belong to `node`. Returns an empty slice for an
    /// out-of-range node rather than panicking.
    fn cores_on_node(&self, node: NodeId) -> &[CpuId];

    /// The NUMA node the calling thread is currently running on. Best-effort:
    /// on a single-socket / non-Linux topology this is always `NodeId(0)`. The
    /// value may change across calls if the OS migrated the thread (unless it
    /// was pinned).
    fn current_node(&self) -> NodeId;

    /// Restrict the calling thread to the CPUs of `node`. Idempotent.
    ///
    /// Returns [`AffinityError::NodeOutOfRange`] for an invalid node and
    /// [`AffinityError::Unsupported`] / [`AffinityError::Syscall`] when the
    /// platform cannot honour the request. The single-node fallback treats a
    /// pin to node 0 as a successful no-op (the post-condition — "thread runs
    /// on node 0's CPUs" — already holds on a one-node machine).
    fn pin_current_thread_to_node(&self, node: NodeId) -> Result<(), AffinityError>;
}
