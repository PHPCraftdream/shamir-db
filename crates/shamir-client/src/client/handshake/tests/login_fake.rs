//! Fake plaintext loopback server speaking the HTTP password-login protocol.
//!
//! Each connection serves exactly one request and is then closed, so the
//! number of accepts equals the number of TCP connections the client opened.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_bytes::{ByteBuf, Bytes};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::server::http_login::{
    build_server_signature, build_transcript, encode_scope, verify_http_login_proof,
    HttpLoginChannels, HttpLoginMode, HttpLoginScope, HttpLoginTranscriptInput,
};
use shamir_connect::server::token::TokenId;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

use crate::client::TokenEndpoint;

pub const BEGIN_PATH: &str = "/shamir/v1/auth/begin";
pub const FINISH_PATH: &str = "/shamir/v1/auth/finish";

const HANDSHAKE_ID: [u8; 32] = [3; 32];
const SERVER_NONCE: [u8; 32] = [5; 32];
const SALT: [u8; 16] = [7; 16];
const TOKEN_ID: [u8; 16] = [9; 16];
const OTHER_TOKEN_ID: [u8; 16] = [8; 16];
const CREATED_AT_NS: u64 = 1_000_000_000;
const EXPIRES_AT_NS: u64 = 2_000_000_000;

/// A raw HTTP reply (status, extra headers, body).
#[derive(Clone)]
pub struct RawReply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after: Option<u64>,
}

impl RawReply {
    /// Reply with an msgpack content type and the given extra headers.
    pub fn new(status: u16, headers: &[(&str, &str)], body: Vec<u8>) -> Self {
        Self {
            status,
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                .collect(),
            body,
        }
    }

    /// Reply carrying `{ "error": code }` (plus `retry_after` when given).
    pub fn error(
        status: u16,
        headers: &[(&str, &str)],
        code: &str,
        retry_after: Option<u64>,
    ) -> Self {
        let body = rmp_serde::to_vec_named(&ErrorBody {
            error: code,
            retry_after,
        })
        .unwrap();
        Self::new(status, headers, body)
    }

    fn encode(&self) -> Vec<u8> {
        let extra: String = self
            .headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}\r\n"))
            .collect();
        let head = format!(
            "HTTP/1.1 {} Test\r\ncontent-type: application/msgpack\r\nconnection: close\r\ncontent-length: {}\r\n{extra}\r\n",
            self.status,
            self.body.len()
        );
        let mut bytes = head.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

/// How the fake answers the finish request.
#[derive(Clone)]
pub enum FinishMode {
    Honest,
    BadSignature,
    WrongTokenId,
    ScopeLabelChanged,
    BadExpiry,
    Status(RawReply),
}

/// Fake server configuration; `Default` is an honest server.
#[derive(Clone)]
pub struct Behavior {
    pub security: u8,
    pub listener_id: [u8; 16],
    pub password: &'static str,
    /// `(memory_kb, time, parallelism)`.
    pub kdf: (u32, u32, u32),
    pub begin_status: Option<RawReply>,
    pub finish: FinishMode,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            security: 0,
            listener_id: [0x11; 16],
            password: "correct horse",
            kdf: (19_456, 2, 1),
            begin_status: None,
            finish: FinishMode::Honest,
        }
    }
}

/// One request as seen by the fake.
#[derive(Clone, Debug)]
pub struct Recorded {
    pub path: String,
    /// Lowercased and sorted.
    pub header_names: Vec<String>,
    pub body: Vec<u8>,
}

/// Strict view of the begin request; decoding it asserts the exact key set.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeginBody {
    pub protocol_version: u8,
    pub user: String,
    pub client_nonce: ByteBuf,
    pub binding_mode: u8,
    pub token: BeginToken,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BeginToken {
    pub label: String,
    pub mode: String,
    pub channels: String,
    pub databases: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FinishBody {
    protocol_version: u8,
    handshake_id: ByteBuf,
    client_proof: ByteBuf,
}

#[derive(Serialize)]
struct ChallengeOut<'a> {
    protocol_version: u8,
    binding_mode: u8,
    security: u8,
    listener_id: &'a Bytes,
    handshake_id: &'a Bytes,
    salt: &'a Bytes,
    kdf: &'a str,
    memory_kb: u32,
    time: u32,
    parallelism: u32,
    argon2_version: u8,
    server_nonce: &'a Bytes,
}

#[derive(Serialize)]
struct SuccessOut<'a> {
    protocol_version: u8,
    server_signature: &'a Bytes,
    token: String,
    id: &'a Bytes,
    label: String,
    owner: &'a str,
    mode: &'a str,
    channels: &'a str,
    databases: Option<&'a [String]>,
    created_at_ns: u64,
    expires_at_ns: u64,
}

struct Pending {
    user: String,
    client_nonce: [u8; 32],
    token: BeginToken,
}

struct State {
    behavior: Behavior,
    accepts: AtomicUsize,
    records: Mutex<Vec<Recorded>>,
    pending: Mutex<Option<Pending>>,
}

pub struct FakeLogin {
    pub addr: SocketAddr,
    state: Arc<State>,
    task: JoinHandle<()>,
}

impl Drop for FakeLogin {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeLogin {
    pub async fn start(behavior: Behavior) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let state = Arc::new(State {
            behavior,
            accepts: AtomicUsize::new(0),
            records: Mutex::new(Vec::new()),
            pending: Mutex::new(None),
        });
        let accept_state = Arc::clone(&state);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                accept_state.accepts.fetch_add(1, Ordering::AcqRel);
                tokio::spawn(serve(stream, Arc::clone(&accept_state)));
            }
        });
        Self { addr, state, task }
    }

    pub fn accepts(&self) -> usize {
        self.state.accepts.load(Ordering::Acquire)
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.state.records.lock().unwrap().clone()
    }

    pub fn endpoint(&self) -> TokenEndpoint {
        TokenEndpoint::plain_loopback(self.addr).unwrap()
    }
}

async fn serve(mut stream: TcpStream, state: Arc<State>) {
    let mut buf: Vec<u8> = Vec::new();
    let Some(head_end) = read_head(&mut stream, &mut buf).await else {
        return;
    };
    let raw_head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let total = head_end + content_length(&raw_head);
    while buf.len() < total {
        if !fill(&mut stream, &mut buf).await {
            return;
        }
    }
    let body = buf[head_end..total].to_vec();
    let mut lines = raw_head.lines();
    let path = lines
        .next()
        .unwrap_or_default()
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned();
    let mut header_names: Vec<String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, _)| name.trim().to_ascii_lowercase())
        .collect();
    header_names.sort_unstable();
    state.records.lock().unwrap().push(Recorded {
        path: path.clone(),
        header_names,
        body: body.clone(),
    });
    let reply = match path.as_str() {
        BEGIN_PATH => handle_begin(&state, &body),
        FINISH_PATH => handle_finish(&state, &body),
        _ => RawReply::error(404, &[], "not_found", None),
    };
    let _ = stream.write_all(&reply.encode()).await;
    let _ = stream.shutdown().await;
}

fn handle_begin(state: &State, body: &[u8]) -> RawReply {
    let behavior = &state.behavior;
    if let Some(raw) = &behavior.begin_status {
        return raw.clone();
    }
    let Ok(begin) = rmp_serde::from_slice::<BeginBody>(body) else {
        return RawReply::error(400, &[], "bad_request", None);
    };
    let Ok(client_nonce) = <[u8; 32]>::try_from(begin.client_nonce.as_slice()) else {
        return RawReply::error(400, &[], "bad_request", None);
    };
    if begin.protocol_version != 1 || begin.binding_mode != 3 {
        return RawReply::error(400, &[], "bad_request", None);
    }
    *state.pending.lock().unwrap() = Some(Pending {
        user: begin.user,
        client_nonce,
        token: begin.token,
    });
    let (memory_kb, time, parallelism) = behavior.kdf;
    let challenge = rmp_serde::to_vec_named(&ChallengeOut {
        protocol_version: 1,
        binding_mode: 3,
        security: behavior.security,
        listener_id: Bytes::new(&behavior.listener_id),
        handshake_id: Bytes::new(&HANDSHAKE_ID),
        salt: Bytes::new(&SALT),
        kdf: "argon2id",
        memory_kb,
        time,
        parallelism,
        argon2_version: 0x13,
        server_nonce: Bytes::new(&SERVER_NONCE),
    })
    .unwrap();
    RawReply::new(200, &[], challenge)
}

fn token_string(id: [u8; 16]) -> String {
    format!("shk1.{}.{}", TokenId(id).to_b64(), "A".repeat(43))
}

fn handle_finish(state: &State, body: &[u8]) -> RawReply {
    let behavior = &state.behavior;
    if let FinishMode::Status(raw) = &behavior.finish {
        return raw.clone();
    }
    let Ok(finish) = rmp_serde::from_slice::<FinishBody>(body) else {
        return RawReply::error(400, &[], "bad_request", None);
    };
    let Ok(proof) = <[u8; 32]>::try_from(finish.client_proof.as_slice()) else {
        return RawReply::error(400, &[], "bad_request", None);
    };
    if finish.protocol_version != 1 || finish.handshake_id.as_slice() != HANDSHAKE_ID.as_slice() {
        return RawReply::error(400, &[], "bad_request", None);
    }
    let guard = state.pending.lock().unwrap();
    let Some(pending) = guard.as_ref() else {
        return RawReply::error(400, &[], "bad_request", None);
    };
    let (memory_kb, time, parallelism) = behavior.kdf;
    let databases: Option<Vec<&str>> = pending
        .token
        .databases
        .as_ref()
        .map(|list| list.iter().map(String::as_str).collect());
    let scope_bytes = encode_scope(&HttpLoginScope {
        label: &pending.token.label,
        mode: match pending.token.mode.as_str() {
            "read_write" => HttpLoginMode::ReadWrite,
            _ => HttpLoginMode::ReadOnly,
        },
        channels: match pending.token.channels.as_str() {
            "ws" => HttpLoginChannels::Ws,
            "both" => HttpLoginChannels::Both,
            _ => HttpLoginChannels::Rest,
        },
        databases: databases.as_deref(),
    })
    .expect("encode fake scope");
    let transcript = build_transcript(&HttpLoginTranscriptInput {
        protocol_version: 1,
        binding_mode: 3,
        security: behavior.security,
        listener_id: &behavior.listener_id,
        handshake_id: &HANDSHAKE_ID,
        user: &pending.user,
        client_nonce: &pending.client_nonce,
        server_nonce: &SERVER_NONCE,
        salt: &SALT,
        memory_kb,
        time,
        parallelism,
        argon2_version: 0x13,
        scope_bytes: &scope_bytes,
    })
    .expect("build fake transcript");
    let keys = DerivedKeys::derive(
        behavior.password.as_bytes(),
        &SALT,
        &KdfParams {
            memory_kb,
            time,
            parallelism,
            argon2_version: 0x13,
        },
    )
    .expect("derive fake keys");
    if !verify_http_login_proof(&proof, &keys.stored_key, &transcript) {
        return RawReply::error(401, &[], "authentication_failed", None);
    }
    let mut signature = build_server_signature(&keys.server_key, &transcript);
    if matches!(behavior.finish, FinishMode::BadSignature) {
        signature[0] ^= 1;
    }
    let token_id = if matches!(behavior.finish, FinishMode::WrongTokenId) {
        OTHER_TOKEN_ID
    } else {
        TOKEN_ID
    };
    let label = if matches!(behavior.finish, FinishMode::ScopeLabelChanged) {
        format!("{}x", pending.token.label)
    } else {
        pending.token.label.clone()
    };
    let expires_at_ns = if matches!(behavior.finish, FinishMode::BadExpiry) {
        CREATED_AT_NS
    } else {
        EXPIRES_AT_NS
    };
    let success = rmp_serde::to_vec_named(&SuccessOut {
        protocol_version: 1,
        server_signature: Bytes::new(&signature),
        token: token_string(token_id),
        id: Bytes::new(&TOKEN_ID),
        label,
        owner: &pending.user,
        mode: &pending.token.mode,
        channels: &pending.token.channels,
        databases: pending.token.databases.as_deref(),
        created_at_ns: CREATED_AT_NS,
        expires_at_ns,
    })
    .unwrap();
    RawReply::new(200, &[], success)
}

async fn fill(stream: &mut TcpStream, buf: &mut Vec<u8>) -> bool {
    let mut chunk = [0u8; 4096];
    match stream.read(&mut chunk).await {
        Ok(0) | Err(_) => false,
        Ok(n) => {
            buf.extend_from_slice(&chunk[..n]);
            true
        }
    }
}

async fn read_head(stream: &mut TcpStream, buf: &mut Vec<u8>) -> Option<usize> {
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            return Some(end + 4);
        }
        if !fill(stream, buf).await {
            return None;
        }
    }
}

fn content_length(raw_head: &str) -> usize {
    raw_head
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())?
        })
        .unwrap_or(0)
}
