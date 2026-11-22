use std::sync::Arc;

use shamir_connect::server::session::SessionStore;

use crate::conn_limiter::ConnLimiter;
use crate::observability::{LoadMetricsSnapshot, ObservabilityState};
use crate::tokens::TokenCounters;
use crate::user_directory::ScramDerivationGate;

pub(super) fn bind_load_source(
    state: &ObservabilityState,
    conn_limiter: ConnLimiter,
    session_store: &Arc<SessionStore>,
    token_counters: &Arc<TokenCounters>,
    gate: &Arc<ScramDerivationGate>,
) {
    // Keep native cumulative counts available until the metrics route stops.
    let sessions_for_load = Arc::clone(session_store);
    let token_counters = Arc::clone(token_counters);
    let gate = Arc::clone(gate);
    state.set_load_source(Arc::new(move || {
        let native_rejected = sessions_for_load.post_auth_rejected_total();
        let semaphore = gate.semaphore();
        LoadMetricsSnapshot {
            connections_active: conn_limiter.live(),
            connections_limit: conn_limiter.cap(),
            connections_rejected_total: conn_limiter.rejected_total(),
            post_auth_rejected_total: native_rejected
                .wrapping_add(token_counters.post_auth_rejected_total()),
            argon2_permits_capacity: semaphore.capacity(),
            argon2_permits_available: semaphore.available().max(0),
            argon2_busy_refusals_total: gate.busy_refusals(),
        }
    }));
}
