use super::{LoginHead, LoginResponse};
use crate::gate::PeerInfo;
use bytes::Bytes;
use std::future::Future;
use std::sync::Arc;

/// Application reservation retained through request handling and released on terminal paths.
pub trait LoginRoutes: Send + Sync + 'static {
    /// RAII reservation associated with an admitted login request.
    type Permit: Send + Sync + 'static;
    /// Whether login endpoints are enabled on this listener.
    fn enabled(&self) -> bool;
    /// Performs asynchronous, header-only admission before the transport polls the body.
    fn admit_head(
        &self,
        route: &[u8],
        head: LoginHead<'_>,
        peer: PeerInfo,
    ) -> impl Future<Output = Result<Self::Permit, LoginResponse>> + Send;
    /// Processes the bounded request body using its admission reservation.
    fn handle(
        &self,
        route: Bytes,
        permit: Arc<Self::Permit>,
        body: Bytes,
    ) -> impl Future<Output = LoginResponse> + Send;
    /// Records a terminal refusal observed before `handle` (e.g. body framing/read timeout).
    fn refused(&self, _route: &[u8], _status: http::StatusCode) {}
    /// Application-specific maximum body size; transport additionally enforces protocol caps.
    fn body_limit(&self, route: &[u8]) -> usize;
}
