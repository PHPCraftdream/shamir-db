//! Token WebSocket session resource cleanup.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::http_ws::TokenWsRegistration;
use crate::security::http_rest::NativeDbHandler;
use crate::subscriptions::SubscriptionRegistry;
use crate::tokens::TokenService;

/// RAII cleanup shared with in-flight dispatches so it runs after their final exit.
pub(crate) struct TokenSessionTeardown {
    service: Arc<TokenService>,
    handler: Arc<dyn NativeDbHandler>,
    sid: [u8; 32],
    registry: Arc<SubscriptionRegistry>,
    registration: Option<TokenWsRegistration>,
    active_metric: AtomicBool,
}

impl TokenSessionTeardown {
    /// Create teardown ownership for one activated token session.
    pub(crate) fn new(
        service: Arc<TokenService>,
        handler: Arc<dyn NativeDbHandler>,
        sid: [u8; 32],
        registry: Arc<SubscriptionRegistry>,
        registration: TokenWsRegistration,
    ) -> Self {
        Self {
            service,
            handler,
            sid,
            registry,
            registration: Some(registration),
            active_metric: AtomicBool::new(false),
        }
    }

    /// Mark activation and increment the active-socket gauge.
    pub(crate) fn activate_metric(&self) {
        metrics::gauge!("shamir_token_active_ws").increment(1.0);
        self.active_metric.store(true, Ordering::Release);
    }

    fn cleanup(&mut self) {
        self.registry.close_all();
        drop(self.registration.take());
        self.service.sessions().remove(&self.sid);
        self.handler.cleanup_session(&self.sid);
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
