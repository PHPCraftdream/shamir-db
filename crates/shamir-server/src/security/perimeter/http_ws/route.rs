//! Native and browser bearer-token WebSocket route integration.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::SinkExt;
use shamir_connect::server::session::Session;
use shamir_connect::server::token::{TokenId, Verified};
use shamir_connect::server::token_session::TokenSocketCaps;
use shamir_transport_http::server::routes::ws::{
    HeadOk, UpgradeHead, UpgradedIo, WsReject, WsRoute, WsUpgrade,
};
use shamir_transport_http::PeerInfo;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use tokio_util::sync::CancellationToken;

use super::first_frame::read_browser_token;
use super::pre_auth::{PreAuthAdmission, PreAuthPermit};
use super::upgrade::{response_headers, validate_head, validate_allowlist, ValidatedHead};
use crate::engine::db_handler::ShamirDbHandler;
use crate::http_rest::{BrowserVerdict, RestGate};
use crate::security::perimeter::http_ws::cancel_registry::{
    TokenWsCancelRegistry, TokenWsRegistration,
};
use crate::sync::connection::token_ws::{
    encode_binding, generate_sid, run_token_loop, session_deadline, TokenLoopContext,
};
use crate::tokens::TokenService;

/// Listener policy and dependencies for the native and browser token routes.
pub(crate) struct WsUpgradeService {
    pub(crate) gate: Arc<RestGate>,
    pub(crate) service: Arc<TokenService>,
    pub(crate) handler: Arc<ShamirDbHandler>,
    pub(crate) registry: Arc<TokenWsCancelRegistry>,
    pub(crate) pre_auth: Arc<PreAuthAdmission>,
    pub(crate) tls: bool,
    pub(crate) browser_available: bool,
    pub(crate) origins: Vec<String>,
    pub(crate) host_allowlist: Vec<String>,
    pub(crate) username: Arc<dyn Fn(&[u8; 16]) -> String + Send + Sync>,
    pub(crate) caps: TokenSocketCaps,
    pub(crate) session_max_age_hours: u64,
    pub(crate) ping_interval: Duration,
    pub(crate) pong_timeout: Duration,
    pub(crate) max_in_flight: usize,
    pub(crate) resolve_peer: Arc<dyn Fn(&PeerInfo) -> SocketAddr + Send + Sync>,
}

/// Admitted token session; its drop removes the token-store entry on all exits.
pub(crate) struct WsSession {
    grant: Option<Verified>,
    session: Option<Arc<Session>>,
    deadline_ns: u64,
    cancel: Option<CancellationToken>,
    _registration: Option<TokenWsRegistration>,
    permit: Option<PreAuthPermit>,
    peer: PeerInfo,
    service: Arc<TokenService>,
}

impl Drop for WsSession {
    fn drop(&mut self) {
        if let Some(session) = &self.session {
            self.service.sessions().remove(&session.session_id);
        }
    }
}

/// Validated route and held pre-auth reservation passed through the hook.
pub(crate) struct WsHeadState {
    head: ValidatedHead,
    route: WsRoute,
    permit: Option<PreAuthPermit>,
    peer: PeerInfo,
}

impl WsUpgrade<Verified> for WsUpgradeService {
    type HeadState = WsHeadState;
    type Session = WsSession;

    fn check_head(
        &self,
        route: WsRoute,
        head: UpgradeHead<'_>,
        mut peer: PeerInfo,
    ) -> Result<HeadOk<Self::HeadState>, WsReject> {
        if route == WsRoute::Browser && !self.browser_available {
            return Err(reject(StatusCode::NOT_FOUND, None));
        }
        let resolved = (self.resolve_peer)(&peer);
        peer.addr = resolved;
        if !self.origins.iter().all(|origin| super::browser_policy::valid_origin(origin.as_bytes()).as_deref() == Some(origin.as_str()))
            || !validate_allowlist(&self.host_allowlist, self.tls)
        {
            return Err(reject(StatusCode::INTERNAL_SERVER_ERROR, None));
        }
        let checked = validate_head(
            route,
            head,
            self.tls,
            &self.origins,
            &self.host_allowlist,
        )?;
        let permit = if route == WsRoute::Browser {
            Some(
                self.pre_auth
                    .try_acquire(resolved.ip())
                    .ok_or_else(|| reject(StatusCode::SERVICE_UNAVAILABLE, Some(1)))?,
            )
        } else {
            None
        };
        Ok(HeadOk {
            state: WsHeadState {
                head: checked.state,
                route,
                permit,
                peer,
            },
        })
    }

    fn admit(
        &self,
        route: WsRoute,
        head: HeadOk<Self::HeadState>,
        grant: Option<Verified>,
        _peer: PeerInfo,
    ) -> Result<(Self::Session, Vec<(HeaderName, HeaderValue)>), WsReject> {
        if head.state.route != route || (route == WsRoute::Native) != head.state.permit.is_none() {
            return Err(reject(StatusCode::BAD_REQUEST, None));
        }
        let grant = match (route, grant) {
            (WsRoute::Native, Some(grant)) => Some(grant),
            (WsRoute::Browser, None) => None,
            _ => return Err(reject(StatusCode::UNAUTHORIZED, None)),
        };
        let mut session = WsSession {
            grant: None,
            session: None,
            deadline_ns: 0,
            cancel: None,
            _registration: None,
            peer: head.state.peer.clone(),
            permit: head.state.permit,
            service: Arc::clone(&self.service),
        };
        if let Some(grant) = grant {
            let activated = self.activate(grant, session.peer.clone(), None)?;
            session.grant = activated.grant;
            session.session = activated.session;
            session.deadline_ns = activated.deadline_ns;
            session.cancel = activated.cancel;
            session._registration = activated._registration;
        }
        Ok((session, response_headers(&head.state.head)))
    }

    fn run(
        &self,
        mut session: Self::Session,
        mut io: UpgradedIo,
        shutdown: CancellationToken,
    ) -> impl Future<Output = ()> + Send {
        async move {
            if session.grant.is_none() {
            let token = tokio::select! {
                _ = shutdown.cancelled() => return,
                result = read_browser_token(&mut io) => match result {
                    Ok(token) => token,
                    Err(()) => {
                        let mut socket = websocket(io).await;
                        close(&mut socket, 4000, "authentication_failed").await;
                        return;
                    }
                }
            };
            let grant = match self.gate.verify_browser(token.as_bytes(), session.peer.clone()) {
                BrowserVerdict::Verified(grant) => grant,
                BrowserVerdict::Failed => {
                    self.gate.record_browser_failure(session.peer.clone());
                    let mut socket = websocket(io).await;
                    close(&mut socket, 4000, "authentication_failed").await;
                    return;
                }
                BrowserVerdict::ChannelDenied => {
                    let mut socket = websocket(io).await;
                    close(&mut socket, 4013, "token_channel_denied").await;
                    return;
                }
                BrowserVerdict::PlaintextWriteDenied => {
                    let mut socket = websocket(io).await;
                    close(&mut socket, 4014, "token_plaintext_write_denied").await;
                    return;
                }
            };
            match self.activate(grant, session.peer.clone(), session.permit.take()) {
                Ok(activated) => {
                    session.grant = activated.grant;
                    session.session = activated.session;
                    session.deadline_ns = activated.deadline_ns;
                    session.cancel = activated.cancel;
                    session._registration = activated._registration;
                    session.permit = None;
                }
                Err(error) => {
                    let mut socket = websocket(io).await;
                    let code = if error.status == StatusCode::SERVICE_UNAVAILABLE { 1013 } else { 4000 };
                    close(&mut socket, code, "authentication_failed").await;
                    return;
                }
            }
        }
        let (Some(grant), Some(token_session), Some(cancel)) = (
            session.grant.as_ref(),
            session.session.as_ref(),
            session.cancel.as_ref(),
        ) else {
            return;
        };
        let sid = token_session.session_id;
        let binding = match encode_binding(&sid, session.deadline_ns) {
            Ok(bytes) => bytes,
            Err(()) => return,
        };
        let mut socket = websocket(io).await;
        if socket.send(Message::Binary(binding.into())).await.is_err() {
            return;
        }
        let context = TokenLoopContext {
            service: Arc::clone(&self.service),
            handler: Arc::clone(&self.handler),
            grant: Verified {
                state: Arc::clone(&grant.state),
            },
            session: Arc::clone(token_session),
            expires_at_ns: session.deadline_ns,
            cancel: cancel.clone(),
            shutdown,
            max_in_flight: self.max_in_flight,
            ping_interval: self.ping_interval,
            pong_timeout: self.pong_timeout,
        };
        run_token_loop(context, socket).await;
        }
    }
}

impl WsUpgradeService {
    fn activate(
        &self,
        grant: Verified,
        peer: PeerInfo,
        permit: Option<PreAuthPermit>,
    ) -> Result<WsSession, WsReject> {
        let now = now_ns();
        let now = now_ns();
        if now == u64::MAX {
            return Err(reject(StatusCode::SERVICE_UNAVAILABLE, Some(1)));
        }
        let sid = generate_sid().map_err(|_| reject(StatusCode::SERVICE_UNAVAILABLE, Some(1)))?;
        let deadline = session_deadline(
            now,
            grant.state.record().expires_at_ns,
            self.session_max_age_hours,
        );
        if deadline <= now {
            return Err(reject(StatusCode::UNAUTHORIZED, None));
        }
        let session = Session::for_token(
            &grant,
            (self.username)(&grant.state.record().owner_user_id),
            now,
            sid,
        );
        let session = self
            .service
            .sessions()
            .admit(sid, session, self.caps)
            .map_err(|_| reject(StatusCode::SERVICE_UNAVAILABLE, Some(1)))?;
        let registration = self.registry.register(TokenId(grant.state.record().id));
        let cancel = registration.token();
        let result = WsSession {
            grant: Some(grant),
            session: Some(session),
            deadline_ns: deadline,
            cancel: Some(cancel),
            _registration: Some(registration),
            permit,
            peer,
            service: Arc::clone(&self.service),
        };
        if result.grant.as_ref().map_or(true, |grant| !self.service.still_valid(grant, now))
            || result.cancel.as_ref().is_some_and(CancellationToken::is_cancelled)
        {
            return Err(reject(StatusCode::UNAUTHORIZED, None));
        }
        Ok(result)
    }
}

async fn websocket(io: UpgradedIo) -> WebSocketStream<UpgradedIo> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(16 * 1024 * 1024))
        .max_frame_size(Some(16 * 1024 * 1024));
    WebSocketStream::from_raw_socket(
        io,
        tokio_tungstenite::tungstenite::protocol::Role::Server,
        Some(config),
    )
    .await
}

async fn close<S>(socket: &mut WebSocketStream<S>, code: u16, reason: &'static str)
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    use tokio_tungstenite::tungstenite::protocol::{frame::coding::CloseCode, CloseFrame};
    let _ = socket
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::from(code),
            reason: reason.into(),
        })))
        .await;
}

fn reject(status: StatusCode, retry_after: Option<u64>) -> WsReject {
    WsReject {
        status,
        retry_after,
        close: true,
    }
}

fn now_ns() -> u64 {
    shamir_connect::common::time::UnixNanos::try_now()
        .map(|value| value.as_u64())
        .unwrap_or(u64::MAX)
}
