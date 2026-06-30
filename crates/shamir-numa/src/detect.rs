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
/// In Фаза 1 this always returns [`FallbackSingleNodeTopology`]. Фаза 1b adds a
/// `#[cfg(target_os = "linux")]` branch that probes `/sys/devices/system/node/`
/// and returns a real multi-node `LinuxTopology` when 2+ nodes are present,
/// keeping the fallback for single-socket Linux so the no-op pin path stays
/// uniform. See `docs/research/NUMA-DESIGN-2026-06-29.md`.
pub fn detect() -> Arc<dyn Topology> {
    // Фаза 1b hook:
    //   #[cfg(target_os = "linux")]
    //   if let Ok(t) = crate::linux::LinuxTopology::probe() {
    //       if t.num_nodes() > 1 { return Arc::new(t); }
    //   }
    Arc::new(FallbackSingleNodeTopology::detect())
}
