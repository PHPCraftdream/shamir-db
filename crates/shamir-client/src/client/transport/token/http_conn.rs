use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full, LengthLimitError, Limited};
use hyper::client::conn::http1::{self, SendRequest};
use hyper::header::{
    HeaderMap, HeaderValue, ACCEPT, AUTHORIZATION, CONNECTION, CONTENT_ENCODING, CONTENT_LENGTH,
    CONTENT_TYPE, HOST, RETRY_AFTER,
};
use hyper::{Method, Request, Uri, Version};
use hyper_util::rt::TokioIo;
use rustls::pki_types::ServerName;
use scc::Bag;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;

use super::status::parse_retry_after;
use super::{build_connector, parse_server_name, TokenEndpoint, TokenScheme};
use crate::error::{ClientError, CloseCause};

const PATH_REQUEST: &str = "/shamir/v1/request";
const PATH_PING: &str = "/shamir/v1/ping";
const MEDIA_MSGPACK: &str = "application/msgpack";
const MAX_RESPONSE_BODY: usize = 16 * 1024 * 1024;
const MAX_IDLE: usize = 4;
const MAX_BUF_SIZE: usize = 64 * 1024;

/// Raw HTTP reply; mapping to results lives in `status.rs`.
#[derive(Debug)]
pub(crate) struct TokenReply {
    pub(crate) status: u16,
    pub(crate) content_type_msgpack: bool,
    pub(crate) has_content_encoding: bool,
    pub(crate) retry_after: Option<Duration>,
    pub(crate) body: Bytes,
}

enum Outcome {
    Reply { reply: TokenReply, reusable: bool },
    NotWritten,
    Failed(ClientError),
}

struct TlsDial {
    connector: TlsConnector,
    server_name: ServerName<'static>,
}

/// HTTP/1.1 client over a small lock-free pool of idle connections.
pub(crate) struct TokenTransport {
    addr: SocketAddr,
    tls: Option<TlsDial>,
    host: HeaderValue,
    authorization: HeaderValue,
    connect_timeout: Option<Duration>,
    request_timeout: Option<Duration>,
    pool: Bag<SendRequest<Full<Bytes>>>,
    idle: AtomicUsize,
}

impl TokenTransport {
    /// Dials up front and parks the connection in the pool.
    pub(crate) async fn connect(
        endpoint: &TokenEndpoint,
        authorization: HeaderValue,
        connect_timeout: Option<Duration>,
        request_timeout: Option<Duration>,
    ) -> Result<Self, ClientError> {
        let tls = match endpoint.scheme() {
            TokenScheme::Https(trust) => Some(TlsDial {
                connector: build_connector(trust)?,
                server_name: parse_server_name(endpoint.server_name())?,
            }),
            TokenScheme::PlainLoopback => None,
        };
        let host = HeaderValue::from_str(&endpoint.host_header())
            .map_err(|_| ClientError::handshake("invalid host header"))?;
        let transport = Self {
            addr: endpoint.addr(),
            tls,
            host,
            authorization,
            connect_timeout,
            request_timeout,
            pool: Bag::default(),
            idle: AtomicUsize::new(0),
        };
        let sender = transport.dial_timed().await?;
        transport.release(sender);
        Ok(transport)
    }

    /// `POST /shamir/v1/request`.
    pub(crate) async fn post_request(&self, body: Vec<u8>) -> Result<TokenReply, ClientError> {
        self.round_trip(Method::POST, PATH_REQUEST, Bytes::from(body))
            .await
    }

    /// `GET /shamir/v1/ping`.
    pub(crate) async fn get_ping(&self) -> Result<TokenReply, ClientError> {
        self.round_trip(Method::GET, PATH_PING, Bytes::new()).await
    }

    /// Drops every pooled connection.
    pub(crate) async fn close(&self) {
        while self.pool.pop().is_some() {
            self.idle.fetch_sub(1, Ordering::AcqRel);
        }
    }

    async fn round_trip(
        &self,
        method: Method,
        path: &'static str,
        body: Bytes,
    ) -> Result<TokenReply, ClientError> {
        let mut fresh = false;
        loop {
            let mut sender = if fresh {
                self.dial_timed().await?
            } else {
                self.acquire().await?
            };
            let req = self.build(method.clone(), path, body.clone());
            let outcome = match self.request_timeout {
                Some(limit) => tokio::time::timeout(limit, Self::attempt(&mut sender, req))
                    .await
                    .map_err(|_| ClientError::RequestTimeout(limit))?,
                None => Self::attempt(&mut sender, req).await,
            };
            match outcome {
                Outcome::Reply { reply, reusable } => {
                    if reusable {
                        self.release(sender);
                    }
                    return Ok(reply);
                }
                Outcome::NotWritten if !fresh => fresh = true,
                Outcome::NotWritten => {
                    return Err(ClientError::ConnectionClosed(CloseCause::PeerEof));
                }
                Outcome::Failed(err) => return Err(err),
            }
        }
    }

    fn build(&self, method: Method, path: &'static str, body: Bytes) -> Request<Full<Bytes>> {
        let msgpack = method == Method::POST;
        let mut req = Request::new(Full::new(body));
        *req.method_mut() = method;
        *req.uri_mut() = Uri::from_static(path);
        let headers = req.headers_mut();
        headers.insert(HOST, self.host.clone());
        headers.insert(AUTHORIZATION, self.authorization.clone());
        if msgpack {
            headers.insert(CONTENT_TYPE, HeaderValue::from_static(MEDIA_MSGPACK));
            headers.insert(ACCEPT, HeaderValue::from_static(MEDIA_MSGPACK));
        }
        req
    }

    async fn attempt(sender: &mut SendRequest<Full<Bytes>>, req: Request<Full<Bytes>>) -> Outcome {
        if sender.ready().await.is_err() {
            return Outcome::NotWritten;
        }
        let resp = match sender.try_send_request(req).await {
            Ok(resp) => resp,
            Err(err) => {
                if err.message().is_some() {
                    return Outcome::NotWritten;
                }
                return Outcome::Failed(ClientError::Protocol(format!(
                    "http exchange failed: {}",
                    err.error()
                )));
            }
        };
        let (parts, body) = resp.into_parts();
        let declared = parts
            .headers
            .get(CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok());
        if declared.is_some_and(|n| n > MAX_RESPONSE_BODY as u64) {
            return Outcome::Failed(body_too_large());
        }
        let body = match Limited::new(body, MAX_RESPONSE_BODY).collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(err) if err.is::<LengthLimitError>() => {
                return Outcome::Failed(body_too_large());
            }
            Err(_) => {
                return Outcome::Failed(ClientError::Protocol("response body read failed".into()));
            }
        };
        let reusable = parts.version == Version::HTTP_11
            && !connection_close(&parts.headers)
            && !sender.is_closed();
        let reply = TokenReply {
            status: parts.status.as_u16(),
            content_type_msgpack: is_msgpack(&parts.headers),
            has_content_encoding: parts.headers.contains_key(CONTENT_ENCODING),
            retry_after: parts
                .headers
                .get(RETRY_AFTER)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_retry_after),
            body,
        };
        Outcome::Reply { reply, reusable }
    }

    async fn acquire(&self) -> Result<SendRequest<Full<Bytes>>, ClientError> {
        while let Some(mut sender) = self.pool.pop() {
            self.idle.fetch_sub(1, Ordering::AcqRel);
            if sender.ready().await.is_ok() {
                return Ok(sender);
            }
        }
        self.dial_timed().await
    }

    fn release(&self, sender: SendRequest<Full<Bytes>>) {
        if self.idle.fetch_add(1, Ordering::AcqRel) >= MAX_IDLE {
            self.idle.fetch_sub(1, Ordering::AcqRel);
            return;
        }
        self.pool.push(sender);
    }

    async fn dial_timed(&self) -> Result<SendRequest<Full<Bytes>>, ClientError> {
        match self.connect_timeout {
            Some(limit) => tokio::time::timeout(limit, self.dial())
                .await
                .map_err(|_| ClientError::ConnectTimeout(limit))?,
            None => self.dial().await,
        }
    }

    async fn dial(&self) -> Result<SendRequest<Full<Bytes>>, ClientError> {
        let tcp = TcpStream::connect(self.addr).await?;
        let _ = tcp.set_nodelay(true);
        let Some(tls) = &self.tls else {
            return handshake_io(tcp).await;
        };
        let stream = tls
            .connector
            .connect(tls.server_name.clone(), tcp)
            .await
            .map_err(|e| ClientError::tls("token connect", e))?;
        if stream.get_ref().1.alpn_protocol() != Some(&b"http/1.1"[..]) {
            return Err(ClientError::handshake("server did not negotiate http/1.1"));
        }
        handshake_io(stream).await
    }
}

async fn handshake_io<I>(io: I) -> Result<SendRequest<Full<Bytes>>, ClientError>
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut builder = http1::Builder::new();
    builder.max_buf_size(MAX_BUF_SIZE);
    let (sender, conn) = builder
        .handshake::<_, Full<Bytes>>(TokioIo::new(io))
        .await
        .map_err(|e| ClientError::handshake_with("http1 handshake", e))?;
    tokio::spawn(async move {
        let _ = conn.await;
    });
    Ok(sender)
}

fn body_too_large() -> ClientError {
    ClientError::Protocol("response body too large".into())
}

fn is_msgpack(headers: &HeaderMap) -> bool {
    headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|media| media.trim().eq_ignore_ascii_case(MEDIA_MSGPACK))
}

fn connection_close(headers: &HeaderMap) -> bool {
    headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case("close"))
}
