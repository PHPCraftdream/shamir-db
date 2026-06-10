//! RAII guard that tracks the `requests_in_flight` metrics gauge.

/// Guard that decrements the `requests_in_flight` gauge on drop.
/// Ensures decrement happens even if the dispatch task panics (§B21/rust-intel).
pub(super) struct InFlightGuard;

impl InFlightGuard {
    pub(super) fn new() -> Self {
        metrics::gauge!("requests_in_flight").increment(1.0);
        InFlightGuard
    }
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        metrics::gauge!("requests_in_flight").decrement(1.0);
    }
}
