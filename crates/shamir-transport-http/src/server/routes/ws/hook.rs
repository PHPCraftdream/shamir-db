//! Application hook for HTTP WebSocket upgrades.

use crate::gate::PeerInfo;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::ReadBuf;

/// A validated route name understood by the application.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WsRoute {
    /// Native bearer-token WebSocket route.
    Native,
    /// Browser first-frame bearer-token route.
    Browser,
}

/// Borrowed view of an HTTP/1 upgrade request head.
pub struct UpgradeHead<'a> {
    /// Request path.
    pub path: &'a str,
    /// Request method.
    pub method: &'a http::Method,
    /// Request headers, preserving duplicates.
    pub headers: &'a http::HeaderMap,
}

impl UpgradeHead<'_> {
    /// Returns all byte values for a header name.
    pub fn values(&self, name: &http::header::HeaderName) -> Vec<&[u8]> {
        self.headers
            .get_all(name)
            .iter()
            .map(|value| value.as_bytes())
            .collect()
    }
}

/// Information required to produce the application-owned session.
pub struct HeadOk<S> {
    /// Application-validated request state.
    pub state: S,
}

/// Rejection to return before completing the protocol upgrade.
pub struct WsReject {
    /// HTTP status for the rejection.
    pub status: http::StatusCode,
    /// Optional retry delay in seconds.
    pub retry_after: Option<u64>,
    /// Whether the HTTP connection must close.
    pub close: bool,
}

/// Transport-owned upgraded byte stream; Hyper's upgraded I/O buffer remains intact.
pub struct UpgradedIo(hyper_util::rt::TokioIo<hyper::upgrade::Upgraded>);

impl UpgradedIo {
    pub(crate) fn new(upgraded: hyper::upgrade::Upgraded) -> Self {
        Self(hyper_util::rt::TokioIo::new(upgraded))
    }
}

impl tokio::io::AsyncRead for UpgradedIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut this.0).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for UpgradedIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        Pin::new(&mut this.0).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut this.0).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        Pin::new(&mut this.0).poll_shutdown(cx)
    }
}

/// Application-supplied WebSocket admission and session lifecycle.
///
/// This is an internal-workspace extension point, intentionally generic over the bearer grant.
pub trait WsUpgrade<G>: Send + Sync + 'static {
    /// State produced by secret-free request-head validation.
    type HeadState: Send + 'static;
    /// Authenticated application session.
    type Session: Send + 'static;

    /// Validates route and HTTP head before authentication.
    fn check_head(
        &self,
        route: WsRoute,
        head: UpgradeHead<'_>,
        peer: PeerInfo,
    ) -> Result<HeadOk<Self::HeadState>, WsReject>;

    /// Admits a validated upgrade, optionally with a native bearer grant.
    fn admit(
        &self,
        route: WsRoute,
        head: HeadOk<Self::HeadState>,
        grant: Option<G>,
        peer: PeerInfo,
    ) -> Result<(Self::Session, Vec<(http::HeaderName, http::HeaderValue)>), WsReject>;

    /// Runs the upgraded connection until completion or listener shutdown.
    fn run(
        &self,
        session: Self::Session,
        io: UpgradedIo,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> impl Future<Output = ()> + Send;
}
