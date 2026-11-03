//! Optional service bundles for HTTP listener routes.

use std::sync::Arc;

/// Login and WebSocket hooks installed together on an HTTP listener.
pub struct RouteServices<L, W> {
    /// Login route implementation.
    pub login: Arc<L>,
    /// WebSocket upgrade implementation.
    pub ws: Arc<W>,
}
