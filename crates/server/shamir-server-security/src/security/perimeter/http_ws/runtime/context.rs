use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use shamir_connect::server::session::Session;
use shamir_connect::server::token::Verified;
use tokio_tungstenite::WebSocketStream;
use tokio_util::sync::CancellationToken;

use crate::http_rest::NativeDbHandler;
use crate::http_ws::cancel_registry::TokenWsRegistration;
use crate::tokens::TokenService;
use shamir_transport_http::UpgradedIo;

/// Context shared with the token WebSocket request loop.
pub struct TokenLoopContext {
    pub service: Arc<TokenService>,
    pub handler: Arc<dyn NativeDbHandler>,
    pub grant: Arc<Verified>,
    pub session: Arc<Session>,
    pub expires_at_ns: u64,
    pub cancel: CancellationToken,
    pub shutdown: CancellationToken,
    pub max_in_flight: usize,
    pub ping_interval: Duration,
    pub pong_timeout: Duration,
    pub registration: TokenWsRegistration,
}

/// Boxed callback for handing an admitted WebSocket to the token loop.
pub type TokenWsRunner = Arc<
    dyn Fn(
            TokenLoopContext,
            WebSocketStream<UpgradedIo>,
        ) -> Pin<Box<dyn Future<Output = ()> + Send>>
        + Send
        + Sync,
>;
