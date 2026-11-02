//! Token WebSocket session resource cleanup.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::engine::db_handler::ShamirDbHandler;
use crate::engine::subscriptions::SubscriptionRegistry;
use crate::security::tokens::TokenService;

/// RAII cleanup shared with in-flight dispatches so it runs after their final exit.
pub(crate) struct TokenSessionTeardown {
    service: Arc<TokenService>,
    handler: Arc<ShamirDbHandler>,
    sid: [u8; 32],
    registry: Arc<SubscriptionRegistry>,
    active_metric: AtomicBool,
}

impl TokenSessionTeardown {
    /// Create teardown ownership for one activated token session.
    pub(crate) fn new(
        service: Arc<TokenService>,
        handler: Arc<ShamirDbHandler>,
        sid: [u8; 32],
        registry: Arc<SubscriptionRegistry>,
    ) -> Self {
        Self {
            service,
            handler,
            sid,
            registry,
            active_metric: AtomicBool::new(false),
        }
    }

    /// Mark activation after the route increments the active-socket gauge.
    pub(crate) fn activate_metric(&self) {
        metrics::gauge!("shamir_token_active_ws").increment(1.0);
        self.active_metric.store(true, Ordering::Release);
    }

    fn cleanup(&self) {
        self.registry.close_all();
        self.service.sessions().remove(&self.sid);
        drop(self.handler.tx_registry().remove_for_session(&self.sid));
        drop(
            self.handler
                .cursor_registry()
                .remove_all_for_session(&self.sid),
        );
        if self.active_metric.swap(false, Ordering::AcqRel) {
            metrics::gauge!("shamir_token_active_ws").decrement(1.0);
        }
    }
}

impl Drop for TokenSessionTeardown {
    fn drop(&mut self) {
        self.cleanup();
    }
}
