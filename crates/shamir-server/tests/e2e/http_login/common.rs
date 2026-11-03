use serde::{Deserialize, Serialize};
use shamir_connect::common::crypto::{constant_time_eq, hmac_sha256};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::http_login::{
    build_transcript, encode_scope, HttpLoginChannels, HttpLoginMode, HttpLoginScope,
    HttpLoginTranscriptInput,
};
use shamir_connect::server::token::{parse_token, TokenChannels, TokenMode};
use shamir_server::user_directory::FjallUserDirectory;
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

pub(super) use crate::http_rest::common::Harness;

pub const USERNAME: &str = "http-login-user";
pub const PASSWORD: &[u8] = b"http-login-test-password";

pub(super) async fn launch() -> Harness {
    launch_with_options(false, false).await
}

pub(super) async fn launch_with_options(tls: bool, allow_read_write: bool) -> Harness {
    let mut settings = shamir_server::config::HttpListenerSettings {
        tls: Some(tls),
        ..Default::default()
    };
    settings.login.enabled = true;
    Harness::launch_with_hooks(
        "127.0.0.1:0",
        settings,
        true,
        |config| config.tokens.login_allow_read_write = allow_read_write,
        |path| {
            seed_user(path);
        },
    )
    .await
}

pub fn seed_user(data_dir: &Path) -> ([u8; 16], Vec<String>) {
    seed_account(data_dir, USERNAME, PASSWORD)
}

pub fn seed_account(data_dir: &Path, username: &str, password: &[u8]) -> ([u8; 16], Vec<String>) {
    let directory = FjallUserDirectory::open(data_dir.join("users")).expect("user directory");
    let salt = [0x42; 16];
    let params = KdfParams {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    };
    let keys = DerivedKeys::derive(password, &salt, &params).expect("derive seeded user keys");
    let id = directory
        .insert(
            username.into(),
            shamir_connect::server::user_record::UserRecord {
                salt,
                stored_key: keys.stored_key,
                server_key: keys.server_key,
                kdf_params: params,
                tickets_invalid_before_ns: 0,
            },
        )
        .expect("insert seeded user");
    (id, Vec::new())
}

pub async fn post(addr: std::net::SocketAddr, path: &str, body: &[u8]) -> (u16, String, Vec<u8>) {
    tokio::time::timeout(Duration::from_secs(15), post_inner(addr, path, body))
        .await
        .expect("HTTP request timed out")
}

async fn post_inner(addr: std::net::SocketAddr, path: &str, body: &[u8]) -> (u16, String, Vec<u8>) {
    assert!(path.starts_with('/') && !path.contains(['\r', '\n', '\0']));
    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect HTTP listener");
    let head = format!("POST {path} HTTP/1.1\r\nHost: localhost:{}\r\nContent-Type: application/msgpack\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", addr.port(), body.len());
    stream
        .write_all(head.as_bytes())
        .await
        .expect("write HTTP headers");
    stream.write_all(body).await.expect("write HTTP body");
    let mut response = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        assert!(response.len() < 8192, "HTTP headers too large");
        let n = stream.read(&mut chunk).await.expect("read HTTP response");
        assert!(n > 0, "HTTP response ended before headers");
        response.extend_from_slice(&chunk[..n]);
        if let Some(end) = response.windows(4).position(|w| w == b"\r\n\r\n") {
            break end;
        }
    };
    let headers = String::from_utf8_lossy(&response[..header_end]).into_owned();
    let status = headers
        .split_whitespace()
        .nth(1)
        .expect("HTTP status")
        .parse()
        .expect("numeric status");
    let content_length = headers
        .lines()
        .skip(1)
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length").then(|| {
                value
                    .trim()
                    .parse::<usize>()
                    .expect("numeric Content-Length")
            })
        })
        .expect("response Content-Length");
    assert!(content_length <= 4096, "HTTP response body too large");
    let body_start = header_end + 4;
    let body_end = body_start
        .checked_add(content_length)
        .expect("response length overflow");
    while response.len() < body_end {
        let n = stream.read(&mut chunk).await.expect("read HTTP body");
        assert!(n > 0, "HTTP response body ended early");
        response.extend_from_slice(&chunk[..n]);
        assert!(
            response.len() <= body_end,
            "unexpected trailing HTTP response data"
        );
    }
    (status, headers, response[body_start..body_end].to_vec())
}

#[derive(Serialize)]
pub(super) struct LoginScope<'a> {
    label: &'a str,
    mode: &'static str,
    channels: &'static str,
    databases: Option<Vec<String>>,
}

#[derive(Serialize)]
struct Begin<'a> {
    protocol_version: u8,
    user: &'a str,
    client_nonce: &'a serde_bytes::Bytes,
    binding_mode: u8,
    token: LoginScope<'a>,
}

pub fn begin_body(label: &str, nonce: &[u8; 32]) -> Vec<u8> {
    begin_body_for(USERNAME, label, "read_only", nonce)
}

pub fn begin_body_for(user: &str, label: &str, mode: &'static str, nonce: &[u8; 32]) -> Vec<u8> {
    rmp_serde::to_vec_named(&Begin {
        protocol_version: 1,
        user,
        client_nonce: serde_bytes::Bytes::new(nonce),
        binding_mode: 3,
        token: LoginScope {
            label,
            mode,
            channels: "rest",
            databases: None,
        },
    })
    .expect("encode login begin")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginChallenge {
    pub protocol_version: u8,
    pub binding_mode: u8,
    pub security: u8,
    pub listener_id: serde_bytes::ByteBuf,
    pub handshake_id: serde_bytes::ByteBuf,
    pub salt: serde_bytes::ByteBuf,
    pub kdf: String,
    pub memory_kb: u32,
    pub time: u32,
    pub parallelism: u32,
    pub argon2_version: u8,
    pub server_nonce: serde_bytes::ByteBuf,
}

#[derive(Serialize)]
struct Finish<'a> {
    protocol_version: u8,
    handshake_id: &'a serde_bytes::Bytes,
    client_proof: &'a serde_bytes::Bytes,
}

pub fn finish_body(handshake_id: &[u8; 32], proof: &[u8; 32]) -> Vec<u8> {
    rmp_serde::to_vec_named(&Finish {
        protocol_version: 1,
        handshake_id: serde_bytes::Bytes::new(handshake_id),
        client_proof: serde_bytes::Bytes::new(proof),
    })
    .expect("encode login finish")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginSuccess {
    pub protocol_version: u8,
    pub server_signature: serde_bytes::ByteBuf,
    pub token: String,
    pub id: serde_bytes::ByteBuf,
    pub label: String,
    pub owner: String,
    pub mode: String,
    pub channels: String,
    pub databases: Option<Vec<String>>,
    pub created_at_ns: u64,
    pub expires_at_ns: u64,
}

pub async fn login(
    addr: std::net::SocketAddr,
    label: &str,
    password: &[u8],
) -> Result<LoginSuccess, (u16, String, Vec<u8>)> {
    login_as(addr, USERNAME, label, password).await
}

pub async fn login_as(
    addr: std::net::SocketAddr,
    username: &str,
    label: &str,
    password: &[u8],
) -> Result<LoginSuccess, (u16, String, Vec<u8>)> {
    let nonce = [9u8; 32];
    let (status, headers, body) = post(
        addr,
        "/shamir/v1/auth/begin",
        &begin_body_for(username, label, "read_only", &nonce),
    )
    .await;
    if status != 200 {
        return Err((status, headers, body));
    }
    let challenge: LoginChallenge = rmp_serde::from_slice(&body).expect("decode login challenge");
    assert_eq!(challenge.security, 0);
    assert_eq!(
        (
            challenge.protocol_version,
            challenge.binding_mode,
            challenge.kdf.as_str()
        ),
        (1, 3, "argon2id")
    );
    let handshake_id: [u8; 32] = challenge
        .handshake_id
        .as_ref()
        .try_into()
        .expect("handshake ID length");
    let transcript = transcript_for(username, label, "read_only", &nonce, &challenge);
    let keys = DerivedKeys::derive(
        password,
        &challenge.salt,
        &KdfParams {
            memory_kb: challenge.memory_kb,
            time: challenge.time,
            parallelism: challenge.parallelism,
            argon2_version: challenge.argon2_version,
        },
    )
    .expect("derive login keys");
    let proof = proof_for(username, label, "read_only", password, &challenge);
    let (status, headers, body) = post(
        addr,
        "/shamir/v1/auth/finish",
        &finish_body(&handshake_id, &proof),
    )
    .await;
    if status != 200 {
        return Err((status, headers, body));
    }
    let success: LoginSuccess = rmp_serde::from_slice(&body).expect("decode login success");
    let expected = hmac_sha256(&keys.server_key[..], &transcript);
    assert!(
        constant_time_eq(&success.server_signature, &expected),
        "server signature mismatch"
    );
    let parsed = parse_token(success.token.as_bytes()).expect("parse returned token");
    assert_eq!(
        parsed.id.0.as_slice(),
        success.id.as_ref(),
        "token ID mismatch"
    );
    assert_eq!(success.owner, username);
    assert_eq!(success.databases, None);
    assert!(success.expires_at_ns > success.created_at_ns);
    assert_eq!(
        (
            success.protocol_version,
            success.mode.as_str(),
            success.channels.as_str()
        ),
        (1, "read_only", "rest")
    );
    Ok(success)
}

pub fn proof_for(
    user: &str,
    label: &str,
    mode: &str,
    password: &[u8],
    challenge: &LoginChallenge,
) -> [u8; 32] {
    let nonce = [9u8; 32];
    let transcript = transcript_for(user, label, mode, &nonce, challenge);
    let keys = DerivedKeys::derive(
        password,
        &challenge.salt,
        &KdfParams {
            memory_kb: challenge.memory_kb,
            time: challenge.time,
            parallelism: challenge.parallelism,
            argon2_version: challenge.argon2_version,
        },
    )
    .expect("derive login keys");
    let signature = hmac_sha256(&keys.stored_key.0, &transcript);
    core::array::from_fn(|index| keys.client_key[index] ^ signature[index])
}

fn transcript_for(
    user: &str,
    label: &str,
    mode: &str,
    nonce: &[u8; 32],
    challenge: &LoginChallenge,
) -> Vec<u8> {
    let salt: [u8; 16] = challenge.salt.as_ref().try_into().expect("salt length");
    let listener_id: [u8; 16] = challenge
        .listener_id
        .as_ref()
        .try_into()
        .expect("listener ID length");
    let handshake_id: [u8; 32] = challenge
        .handshake_id
        .as_ref()
        .try_into()
        .expect("handshake ID length");
    let server_nonce: [u8; 32] = challenge
        .server_nonce
        .as_ref()
        .try_into()
        .expect("server nonce length");
    let scope = encode_scope(&HttpLoginScope {
        label,
        mode: if mode == "read_write" {
            HttpLoginMode::ReadWrite
        } else {
            HttpLoginMode::ReadOnly
        },
        channels: HttpLoginChannels::Rest,
        databases: None,
    })
    .expect("encode transcript scope");
    build_transcript(&HttpLoginTranscriptInput {
        protocol_version: 1,
        binding_mode: 3,
        security: challenge.security,
        listener_id: &listener_id,
        handshake_id: &handshake_id,
        user,
        client_nonce: nonce,
        server_nonce: &server_nonce,
        salt: &salt,
        memory_kb: challenge.memory_kb,
        time: challenge.time,
        parallelism: challenge.parallelism,
        argon2_version: challenge.argon2_version,
        scope_bytes: &scope,
    })
    .expect("build login transcript")
}

pub(super) fn token_scope(label: &str, mode: TokenMode, channels: TokenChannels) -> LoginScope<'_> {
    LoginScope {
        label,
        mode: match mode {
            TokenMode::ReadOnly => "read_only",
            TokenMode::ReadWrite => "read_write",
        },
        channels: match channels {
            TokenChannels::Rest => "rest",
            TokenChannels::Ws => "ws",
            TokenChannels::Both => "both",
        },
        databases: None,
    }
}
