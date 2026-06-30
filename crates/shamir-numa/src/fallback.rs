//! Single-node topology for non-Linux platforms and single-socket Linux.

use crate::error::AffinityError;
use crate::node::{CpuId, NodeId};
use crate::topology::Topology;

/// A topology that reports exactly one UMA node owning every CPU.
///
/// Used on Windows, macOS, single-socket Linux, and anywhere the real
/// `LinuxTopology` probe finds `< 2` nodes. With this topology a
/// [`NodeReplicated`](crate::NodeReplicated) collapses to a single replica and
/// behaves identically to a bare `ArcSwap` — the zero-overhead path that lets
/// consumers use `NodeReplicated` unconditionally.
pub struct FallbackSingleNodeTopology {
    cores: Vec<CpuId>,
}

impl FallbackSingleNodeTopology {
    /// Build from the host's reported parallelism
    /// (`std::thread::available_parallelism`, falling back to 1).
    pub fn detect() -> Self {
        let n = std::thread::available_parallelism()
            .map(|x| x.get())
            .unwrap_or(1);
        Self::with_cpus(n)
    }

    /// Build a single node owning CPUs `0..n` (at least one).
    pub fn with_cpus(n: usize) -> Self {
        let n = n.max(1);
        Self {
            cores: (0..n).map(CpuId).collect(),
        }
    }
}

impl Topology for FallbackSingleNodeTopology {
    fn num_nodes(&self) -> usize {
        1
    }

    fn cores_on_node(&self, node: NodeId) -> &[CpuId] {
        if node.0 == 0 {
            &self.cores
        } else {
            &[]
        }
    }

    fn current_node(&self) -> NodeId {
        NodeId(0)
    }

    fn pin_current_thread_to_node(&self, node: NodeId) -> Result<(), AffinityError> {
        // Pinning to "the only node" is a no-op that trivially satisfies its
        // post-condition — the thread is already on node 0's CPUs because that
        // is every CPU. Any other node is out of range.
        if node.0 == 0 {
            Ok(())
        } else {
            Err(AffinityError::NodeOutOfRange {
                requested: node.0,
                available: 1,
            })
        }
    }
}
