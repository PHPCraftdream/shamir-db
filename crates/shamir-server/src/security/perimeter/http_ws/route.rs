//! Native and browser bearer-token WebSocket route integration.

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
use shamir_transport_ws::framing::MAX_WS_FRAME_SIZE;
use tokio::io::AsyncReadExt;
use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue, StatusCode};
use tokio_tungstenite::tungstenite::protocol::{
    frame::coding::CloseCode, CloseFrame, Role, WebSocketConfig,
};
use tokio_tungstenite::{tungstenite::Message, WebSocketStream};
use tokio_util::sync::CancellationToken;

use super::browser::first_frame::read_browser_token;
use super::browser::{PreAuthAdmission, PreAuthPermit};
use super::cancel_registry::{TokenWsCancelRegistry, TokenWsRegistration};
use super::upgrade::{response_headers, validate_head, ValidatedHead};
use crate::engine::db_handler::ShamirDbHandler;
use crate::http_rest::{BrowserVerdict, RestGate};
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
    registration: Option<TokenWsRegistration>,
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
        let checked = validate_head(route, head, self.tls, &self.origins, &self.host_allowlist)?;
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
            registration: None,
            peer: head.state.peer,
            permit: head.state.permit,
            service: Arc::clone(&self.service),
        };
        if let Some(grant) = grant {
            self.activate(grant, &mut session)
                .map_err(|error| error.reject())?;
        }
        Ok((session, response_headers(&head.state.head)))
    }

    async fn run(
        &self,
        mut session: Self::Session,
        mut io: UpgradedIo,
        shutdown: CancellationToken,
    ) {
        if session.grant.is_none() {
            let token = tokio::select! {
                _ = shutdown.cancelled() => {
                    drop(session.permit.take());
                    let mut socket = websocket(io).await;
                    close(&mut socket, 1000, &CancellationToken::new()).await;
                    return;
                },
                result = read_browser_token(&mut io) => match result {
                    Ok(token) => token,
                    Err(()) => {
                        self.gate.record_browser_failure(session.peer.clone());
                        drop(session.permit.take());
                        drop(session.registration.take());
                        let mut socket = websocket(io).await;
                        close(&mut socket, 4000, &shutdown).await;
                        return;
                    }
                }
            };
            let grant = match self
                .gate
                .verify_browser(token.as_bytes(), session.peer.clone())
            {
                BrowserVerdict::Verified(grant) => grant,
                BrowserVerdict::Failed => {
                    drop(session.permit.take());
                    drop(session.registration.take());
                    let mut socket = websocket(io).await;
                    close(&mut socket, 4000, &shutdown).await;
                    return;
                }
                BrowserVerdict::ChannelDenied => {
                    drop(session.permit.take());
                    drop(session.registration.take());
                    let mut socket = websocket(io).await;
                    close(&mut socket, 4013, &shutdown).await;
                    return;
                }
                BrowserVerdict::PlaintextWriteDenied => {
                    drop(session.permit.take());
                    drop(session.registration.take());
                    let mut socket = websocket(io).await;
                    close(&mut socket, 4014, &shutdown).await;
                    return;
                }
            };
            if let Err(error) = self.activate(grant, &mut session) {
                drop(session.permit.take());
                drop(session.registration.take());
                if let Some(active) = session.session.as_ref() {
                    self.service.sessions().remove(&active.session_id);
                }
                let mut socket = websocket(io).await;
                close(&mut socket, error.close_code(), &shutdown).await;
                return;
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
        let token_session = Arc::clone(token_session);
        let deadline_ns = session.deadline_ns;
        let binding = match encode_binding(&sid, deadline_ns) {
            Ok(bytes) => bytes,
            Err(()) => return,
        };
        let mut socket = websocket(io).await;
        tokio::select! {
            _ = shutdown.cancelled() => {
                drop(session.registration.take());
                self.service.sessions().remove(&sid);
                close(&mut socket, 1000, &CancellationToken::new()).await;
                return;
            },
            _ = cancel.cancelled() => {
                drop(session.registration.take());
                self.service.sessions().remove(&sid);
                close(&mut socket, 4011, &shutdown).await;
                return;
            }
            result = tokio::time::timeout(Duration::from_secs(5), socket.send(Message::Binary(binding.into()))) => {
                if !matches!(result, Ok(Ok(()))) { return; }
            }
        }
        let context = TokenLoopContext {
            service: Arc::clone(&self.service),
            handler: Arc::clone(&self.handler),
            grant: Arc::new(Verified {
                state: Arc::clone(&grant.state),
            }),
            session: token_session,
            expires_at_ns: deadline_ns,
            cancel: cancel.clone(),
            shutdown,
            max_in_flight: self.max_in_flight,
            ping_interval: self.ping_interval,
            pong_timeout: self.pong_timeout,
            registration: session
                .registration
                .take()
                .expect("active session registration"),
        };
        run_token_loop(context, socket).await;
    }
}

enum ActivationError {
    Busy,
    Revoked,
    Expired,
}

impl ActivationError {
    fn reject(&self) -> WsReject {
        match self {
            Self::Busy => reject(StatusCode::SERVICE_UNAVAILABLE, Some(1)),
            Self::Revoked | Self::Expired => reject(StatusCode::UNAUTHORIZED, None),
        }
    }

    fn close_code(&self) -> u16 {
        match self {
            Self::Busy => 1013,
            Self::Revoked => 4011,
            Self::Expired => 4012,
        }
    }
}

impl WsUpgradeService {
    /// Admits `grant` into the token store and fills `target` for Drop-safe cleanup.
    fn activate(&self, grant: Verified, target: &mut WsSession) -> Result<(), ActivationError> {
        let now = now_ns();
        if now == u64::MAX {
            return Err(ActivationError::Busy);
        }
        let sid = generate_sid().map_err(|()| ActivationError::Busy)?;
        let record = grant.state.record();
        let token_id = record.id;
        let expires_at_ns = record.expires_at_ns;
        let owner_user_id = record.owner_user_id;
        let deadline = session_deadline(now, expires_at_ns, self.session_max_age_hours);
        if deadline <= now {
            return Err(ActivationError::Expired);
        }
        let session = Session::for_token(&grant, (self.username)(&owner_user_id), now, sid);
        let session = self
            .service
            .sessions()
            .admit(sid, session, self.caps)
            .map_err(|_| ActivationError::Busy)?;
        target.session = Some(session);
        let registration = self.registry.register(TokenId(token_id));
        let cancel = registration.token();
        target.registration = Some(registration);
        let now = now_ns();
        if now == u64::MAX {
            return Err(ActivationError::Busy);
        }
        if cancel.is_cancelled() || !self.service.still_valid(&grant, now) {
            return Err(if now >= expires_at_ns {
                ActivationError::Expired
            } else {
                ActivationError::Revoked
            });
        }
        target.cancel = Some(cancel);
        target.deadline_ns = deadline;
        target.grant = Some(grant);
        target.permit = None;
        Ok(())
    }
}

async fn websocket(io: UpgradedIo) -> WebSocketStream<UpgradedIo> {
    let config = WebSocketConfig::default()
        .max_message_size(Some(MAX_WS_FRAME_SIZE))
        .max_frame_size(Some(MAX_WS_FRAME_SIZE));
    WebSocketStream::from_raw_socket(io, Role::Server, Some(config)).await
}

/// Unread client bytes drained after a terminal close so the socket ends with FIN, not RST.
const CLOSE_LINGER_LIMIT: usize = 64 * 1024;
const CLOSE_LINGER: Duration = Duration::from_secs(2);

async fn close(socket: &mut WebSocketStream<UpgradedIo>, code: u16, shutdown: &CancellationToken) {
    let _ = tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            _ = shutdown.cancelled() => {},
            _ = socket.send(Message::Close(Some(CloseFrame {
                code: CloseCode::from(code),
                reason: crate::framer::close_reason(code).into(),
            }))) => {},
        }
    })
    .await;
    let io = socket.get_mut();
    let mut scratch = [0_u8; 1024];
    let mut drained = 0_usize;
    let drain = async {
        while drained < CLOSE_LINGER_LIMIT {
            match io.read(&mut scratch).await {
                Ok(0) | Err(_) => break,
                Ok(read) => drained += read,
            }
        }
    };
    tokio::select! {
        _ = shutdown.cancelled() => {}
        _ = tokio::time::timeout(CLOSE_LINGER, drain) => {}
    }
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
