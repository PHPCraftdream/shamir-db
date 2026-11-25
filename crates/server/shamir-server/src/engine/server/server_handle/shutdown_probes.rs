#[cfg(test)]
use std::sync::Mutex;

/// One-shot probe fired immediately before the owner starts awaiting the
/// scheduled worker's completion signal. Test-only: a shutdown test that
/// parks the worker waits for this marker instead of guessing how long the
/// earlier teardown stages take, so "shutdown must not return while the
/// worker is parked" can never pass vacuously on a slow runner.
#[cfg(test)]
static SCHEDULED_JOIN_PROBE: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

/// Clears an unfired [`arm_scheduled_join_probe`] on drop; a probe that fires
/// removes itself from the slot.
#[cfg(test)]
pub(crate) struct ScheduledJoinProbeGuard;

#[cfg(test)]
impl Drop for ScheduledJoinProbeGuard {
    fn drop(&mut self) {
        *SCHEDULED_JOIN_PROBE.lock().expect("scheduled join probe") = None;
    }
}

#[cfg(test)]
pub(crate) fn arm_scheduled_join_probe(
    probe: impl FnOnce() + Send + 'static,
) -> ScheduledJoinProbeGuard {
    *SCHEDULED_JOIN_PROBE.lock().expect("scheduled join probe") = Some(Box::new(probe));
    ScheduledJoinProbeGuard
}

#[cfg(test)]
pub(in crate::engine::server) fn fire_scheduled_join_probe() {
    let probe = SCHEDULED_JOIN_PROBE
        .lock()
        .expect("scheduled join probe")
        .take();
    if let Some(probe) = probe {
        probe();
    }
}

/// One-shot marker for the post-deadline fallback join in `ShutdownOwner::drain`.
#[cfg(test)]
static FALLBACK_DRAIN_JOIN_PROBE: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

#[cfg(test)]
pub(crate) struct FallbackDrainJoinProbeGuard;

#[cfg(test)]
impl Drop for FallbackDrainJoinProbeGuard {
    fn drop(&mut self) {
        *FALLBACK_DRAIN_JOIN_PROBE
            .lock()
            .expect("fallback drain join probe") = None;
    }
}

#[cfg(test)]
pub(crate) fn arm_fallback_drain_join_probe(
    probe: impl FnOnce() + Send + 'static,
) -> FallbackDrainJoinProbeGuard {
    *FALLBACK_DRAIN_JOIN_PROBE
        .lock()
        .expect("fallback drain join probe") = Some(Box::new(probe));
    FallbackDrainJoinProbeGuard
}

#[cfg(test)]
fn fire_fallback_drain_join_probe() {
    let probe = FALLBACK_DRAIN_JOIN_PROBE
        .lock()
        .expect("fallback drain join probe")
        .take();
    if let Some(probe) = probe {
        probe();
    }
}

/// Mark entry into the post-expiry owner drain (test-only).
#[cfg(test)]
pub(crate) fn mark_fallback_drain_join() {
    fire_fallback_drain_join_probe();
}

/// One-shot probe fired immediately BEFORE the ordered shutdown's REAL
/// repository flush call (`ShamirDb::flush_all_report`) — never at the point
/// the flush would have started, so a SKIPPED flush leaves the probe unfired
/// and cannot look like an entered one. A shutdown test compares its order with
/// [`arm_scheduled_join_probe`]: the scheduled worker and its in-flight run are
/// joined BEFORE the flush, so the join marker must fire first (mutant: the
/// worker awaited after the flush).
#[cfg(test)]
static REPO_FLUSH_PROBE: Mutex<Option<Box<dyn FnOnce() + Send>>> = Mutex::new(None);

/// Clears an unfired [`arm_repo_flush_probe`]; a probe that fires removes
/// itself.
#[cfg(test)]
pub(crate) struct RepoFlushProbeGuard;

#[cfg(test)]
impl Drop for RepoFlushProbeGuard {
    fn drop(&mut self) {
        *REPO_FLUSH_PROBE.lock().expect("repo flush probe") = None;
    }
}

/// Arm ONE repository-flush probe.
#[cfg(test)]
pub(crate) fn arm_repo_flush_probe(probe: impl FnOnce() + Send + 'static) -> RepoFlushProbeGuard {
    *REPO_FLUSH_PROBE.lock().expect("repo flush probe") = Some(Box::new(probe));
    RepoFlushProbeGuard
}

/// Fire and consume the repository-flush probe, if any.
#[cfg(test)]
pub(in crate::engine::server) fn fire_repo_flush_probe() {
    let probe = REPO_FLUSH_PROBE.lock().expect("repo flush probe").take();
    if let Some(probe) = probe {
        probe();
    }
}
