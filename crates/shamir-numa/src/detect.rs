//! Best-effort topology detection.

use std::sync::Arc;

use crate::fallback::FallbackSingleNodeTopology;
use crate::topology::Topology;

/// Return the richest [`Topology`] the current platform supports, as an
/// `Arc<dyn Topology>` ready to hand to [`NodeReplicated`](crate::NodeReplicated).
///
/// Never fails — detection always degrades to a single-node fallback so callers
/// get a usable topology unconditionally.
///
/// On Linux, probes `/sys/devices/system/node/` via [`LinuxTopology`] first.
/// If the probe succeeds and the host exposes at least one node, the real
/// multi-node topology is returned. Otherwise (missing sysfs, container without
/// NUMA, or single-socket host) the call falls back to
/// [`FallbackSingleNodeTopology`].
///
/// On every other platform the fallback is returned directly.
///
/// [`LinuxTopology`]: crate::LinuxTopology
#[cfg(target_os = "linux")]
pub fn detect() -> Arc<dyn Topology> {
    if let Ok(topo) = crate::linux::LinuxTopology::probe() {
        if topo.num_nodes() > 0 {
            return Arc::new(topo);
        }
    }
    Arc::new(FallbackSingleNodeTopology::detect())
}

/// Return the richest [`Topology`] the current platform supports, as an
/// `Arc<dyn Topology>` ready to hand to [`NodeReplicated`](crate::NodeReplicated).
///
/// Never fails — detection always degrades to a single-node fallback so callers
/// get a usable topology unconditionally.
///
/// On non-Linux platforms this always returns [`FallbackSingleNodeTopology`].
#[cfg(not(target_os = "linux"))]
pub fn detect() -> Arc<dyn Topology> {
    Arc::new(FallbackSingleNodeTopology::detect())
}
