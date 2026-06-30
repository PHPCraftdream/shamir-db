//! Error type for thread-affinity operations.

use thiserror::Error;

/// Failure modes for [`Topology::pin_current_thread_to_node`].
///
/// [`Topology::pin_current_thread_to_node`]: crate::Topology::pin_current_thread_to_node
#[derive(Debug, Error)]
pub enum AffinityError {
    /// The platform / topology cannot pin threads to nodes (a real
    /// `LinuxTopology` not yet wired, or a genuinely affinity-less OS). A
    /// best-effort NUMA policy should treat this as a soft no-op, not a hard
    /// failure. Note that the single-node fallback returns `Ok(())` for node 0
    /// instead — see [`FallbackSingleNodeTopology`](crate::FallbackSingleNodeTopology).
    #[error("thread affinity is not supported on this platform/topology")]
    Unsupported,

    /// The requested node is outside `[0, num_nodes())`.
    #[error("NUMA node {requested} out of range (topology has {available} node(s))")]
    NodeOutOfRange {
        /// The node index the caller asked to pin to.
        requested: usize,
        /// How many nodes the topology actually has.
        available: usize,
    },

    /// The underlying affinity syscall failed — e.g. `EPERM` when the process
    /// lacks `CAP_SYS_NICE` inside a restricted container.
    #[error("affinity syscall failed: {0}")]
    Syscall(#[from] std::io::Error),
}
