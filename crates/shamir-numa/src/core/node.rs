//! NUMA node and logical-CPU identifiers.

/// Index of a NUMA node in the range `[0, Topology::num_nodes())`.
///
/// A single-socket (UMA) system has exactly one node, [`NodeId(0)`](NodeId).
/// The index doubles as the slot into [`NodeReplicated`](crate::NodeReplicated)'s
/// per-node replica array, so it is always dense and zero-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub usize);

/// Logical CPU (hardware thread) identifier as the OS numbers it.
///
/// On Linux this matches the `cpu` numbering exposed under
/// `/sys/devices/system/node/nodeN/cpulist` and the bit index used in the
/// `cpu_set_t` passed to `sched_setaffinity(2)` (Drepper §5.2–5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CpuId(pub usize);
