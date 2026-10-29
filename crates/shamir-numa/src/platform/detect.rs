//! Best-effort topology detection.

#[cfg(target_os = "linux")]
use std::io::Write;
use std::sync::{Arc, OnceLock};

use crate::core::topology::Topology;
#[cfg(any(test, target_os = "linux"))]
use crate::error::AffinityError;
use crate::platform::fallback::FallbackSingleNodeTopology;

static CACHED_TOPOLOGY: OnceLock<Arc<dyn Topology>> = OnceLock::new();
#[cfg(target_os = "linux")]
static FALLBACK_DIAGNOSTIC: OnceLock<()> = OnceLock::new();

#[cfg(any(test, target_os = "linux"))]
pub(crate) fn detect_with(
    probe: impl FnOnce() -> Result<Arc<dyn Topology>, AffinityError>,
    diagnostic: &OnceLock<()>,
    report: impl FnOnce(&AffinityError),
) -> Arc<dyn Topology> {
    match probe() {
        Ok(topology) => topology,
        Err(error) => {
            diagnostic.get_or_init(|| report(&error));
            Arc::new(FallbackSingleNodeTopology::detect())
        }
    }
}

pub(crate) fn cached_with(
    cache: &OnceLock<Arc<dyn Topology>>,
    probe: impl FnOnce() -> Arc<dyn Topology>,
) -> Arc<dyn Topology> {
    Arc::clone(cache.get_or_init(probe))
}

/// Probe the platform afresh, falling back to a single node on failure.
///
/// Linux reads sysfs and emits at most one process-wide fallback diagnostic;
/// other platforms use the fallback directly. Each call performs fresh detection.
#[cfg(target_os = "linux")]
pub fn detect() -> Arc<dyn Topology> {
    detect_with(
        || {
            super::linux::LinuxTopology::probe()
                .map(|topology| Arc::new(topology) as Arc<dyn Topology>)
        },
        &FALLBACK_DIAGNOSTIC,
        |error| {
            let _ = writeln!(
                std::io::stderr().lock(),
                "shamir-numa: topology probe failed; using single-node fallback: {error:?}"
            );
        },
    )
}

/// Detect a fresh single-node fallback on non-Linux platforms.
#[cfg(not(target_os = "linux"))]
pub fn detect() -> Arc<dyn Topology> {
    Arc::new(FallbackSingleNodeTopology::detect())
}

/// Return the process-wide topology, initialized once, including any fallback.
///
/// Topology changes require fresh [`detect`], explicit injection, or a process
/// restart; cached topology and existing replicas are not updated in place.
pub fn detect_cached() -> Arc<dyn Topology> {
    cached_with(&CACHED_TOPOLOGY, detect)
}
