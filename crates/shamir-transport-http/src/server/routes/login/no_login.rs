use super::{LoginHead, LoginResponse, LoginRoutes};
use crate::gate::PeerInfo;
use bytes::Bytes;
use http::StatusCode;

/// Built-in disabled login service used by `serve`.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoLogin;

impl LoginRoutes for NoLogin {
    type Permit = ();

    fn enabled(&self) -> bool {
        false
    }

    async fn admit_head(
        &self,
        _route: &[u8],
        _head: LoginHead<'_>,
        _peer: PeerInfo,
    ) -> Result<Self::Permit, LoginResponse> {
        Err(not_found())
    }

    async fn handle(&self, _route: Bytes, _permit: Self::Permit, _body: Bytes) -> LoginResponse {
        not_found()
    }

    fn body_limit(&self, _route: &[u8]) -> usize {
        0
    }
}

fn not_found() -> LoginResponse {
    LoginResponse::new(
        StatusCode::NOT_FOUND,
        Bytes::from_static(b"\x81\xa5error\xa9not_found"),
    )
}
