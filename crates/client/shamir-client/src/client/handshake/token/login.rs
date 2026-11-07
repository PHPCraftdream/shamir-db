use bytes::Bytes;
use shamir_connect::common::crypto::{constant_time_eq, hmac_sha256, try_random_array};
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::scram::DerivedKeys;
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::http_login::{
    build_server_signature, build_transcript, encode_scope, HttpLoginChannels, HttpLoginMode,
    HttpLoginScope, HttpLoginTranscriptInput,
};
use shamir_connect::server::token::parse_token;
use shamir_connect::Error as ConnectError;
use zeroize::{Zeroize, Zeroizing};

use super::login_options::LoginOptions;
use super::login_outcome::LoginOutcome;
use super::login_scope::{LoginChannels, LoginMode};
use super::login_wire::{decode_challenge, decode_finish, encode_begin, encode_finish};
use crate::client::transport::token::{map_login_failure, TokenScheme, TokenTransport};
use crate::client::ApiToken;
use crate::error::ClientError;

const BEGIN_PATH: &str = "/shamir/v1/auth/begin";
const FINISH_PATH: &str = "/shamir/v1/auth/finish";
const MAX_REPLY_BYTES: usize = 8192;
const NONCE_ATTEMPTS: usize = 3;

type KeyPair = (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>);

fn entropy_error() -> ClientError {
    ClientError::handshake("login: entropy unavailable")
}

fn fresh_nonce() -> Result<[u8; 32], ClientError> {
    for _ in 0..NONCE_ATTEMPTS {
        let nonce = try_random_array::<32>().map_err(|_| entropy_error())?;
        if nonce.iter().any(|byte| *byte != 0) {
            return Ok(nonce);
        }
    }
    Err(entropy_error())
}

fn wire_mode(mode: LoginMode) -> HttpLoginMode {
    match mode {
        LoginMode::ReadOnly => HttpLoginMode::ReadOnly,
        LoginMode::ReadWrite => HttpLoginMode::ReadWrite,
    }
}

fn wire_channels(channels: LoginChannels) -> HttpLoginChannels {
    match channels {
        LoginChannels::Rest => HttpLoginChannels::Rest,
        LoginChannels::Ws => HttpLoginChannels::Ws,
        LoginChannels::Both => HttpLoginChannels::Both,
    }
}

fn reply_shape(
    content_type_msgpack: bool,
    has_content_encoding: bool,
    len: usize,
) -> Result<(), ClientError> {
    if content_type_msgpack && !has_content_encoding && len <= MAX_REPLY_BYTES {
        Ok(())
    } else {
        Err(ClientError::Protocol("unexpected login response".into()))
    }
}

fn wipe_body(body: Bytes) {
    if let Ok(mut buf) = body.try_into_mut() {
        buf[..].zeroize();
    }
}

fn derive_proof(
    password: Zeroizing<String>,
    salt: [u8; 16],
    params: KdfParams,
    transcript: Vec<u8>,
) -> Result<KeyPair, ConnectError> {
    let mut keys = DerivedKeys::derive(password.as_bytes(), &salt, &params)?;
    drop(password);
    let signature = Zeroizing::new(hmac_sha256(&keys.stored_key.0, &transcript));
    keys.stored_key.0.zeroize();
    let proof = Zeroizing::new(core::array::from_fn(|i| keys.client_key[i] ^ signature[i]));
    Ok((proof, keys.server_key))
}

/// Signs in with user name and password over the server HTTP listener and
/// returns an ordinary reusable API token.
pub async fn login(opts: LoginOptions) -> Result<LoginOutcome, ClientError> {
    let LoginOptions {
        endpoint,
        username,
        password,
        mut scope,
        listener_id,
        connect_timeout,
        request_timeout,
    } = opts;

    let expected_security: u8 = match endpoint.scheme() {
        TokenScheme::Https(_) => 1,
        TokenScheme::PlainLoopback => 0,
    };

    let user = NormalizedUsername::from_raw(&username)
        .map_err(|e| ClientError::InvalidUsername(e.to_string()))?;
    if let Some(databases) = scope.databases.as_mut() {
        databases.sort();
        databases.dedup();
    }
    let scope_bytes = {
        let database_refs: Option<Vec<&str>> = scope
            .databases
            .as_ref()
            .map(|databases| databases.iter().map(String::as_str).collect());
        encode_scope(&HttpLoginScope {
            label: &scope.label,
            mode: wire_mode(scope.mode),
            channels: wire_channels(scope.channels),
            databases: database_refs.as_deref(),
        })
        .map_err(|msg| ClientError::handshake(format!("login: invalid scope: {msg}")))?
    };

    let nonce = fresh_nonce()?;

    let transport = TokenTransport::anonymous(&endpoint, connect_timeout, request_timeout)?;
    let begin_body = encode_begin(user.as_str(), &nonce, &scope)?;
    let reply = transport
        .post_fresh(BEGIN_PATH, Bytes::from(begin_body), MAX_REPLY_BYTES)
        .await?;
    if reply.status != 200 {
        return Err(map_login_failure(&reply));
    }
    reply_shape(
        reply.content_type_msgpack,
        reply.has_content_encoding,
        reply.body.len(),
    )?;
    let challenge = decode_challenge(&reply.body)?;

    if challenge.security != expected_security {
        return Err(ClientError::handshake("login: security class mismatch"));
    }
    if listener_id.is_some_and(|pinned| pinned != challenge.listener_id) {
        return Err(ClientError::handshake("login: listener id mismatch"));
    }

    let transcript = build_transcript(&HttpLoginTranscriptInput {
        protocol_version: 1,
        binding_mode: 3,
        security: challenge.security,
        listener_id: &challenge.listener_id,
        handshake_id: &challenge.handshake_id,
        user: user.as_str(),
        client_nonce: &nonce,
        server_nonce: &challenge.server_nonce,
        salt: &challenge.salt,
        memory_kb: challenge.memory_kb,
        time: challenge.time,
        parallelism: challenge.parallelism,
        argon2_version: challenge.argon2_version,
        scope_bytes: &scope_bytes,
    })
    .map_err(|e| ClientError::handshake(format!("login: challenge rejected: {e}")))?;

    let params = KdfParams {
        memory_kb: challenge.memory_kb,
        time: challenge.time,
        parallelism: challenge.parallelism,
        argon2_version: challenge.argon2_version,
    };
    let salt = challenge.salt;
    let blocking_transcript = transcript.clone();
    let (proof, server_key) = tokio::task::spawn_blocking(move || {
        derive_proof(password, salt, params, blocking_transcript)
    })
    .await
    .map_err(|e| ClientError::handshake_with("login: key derivation task", e))?
    .map_err(|e| ClientError::handshake_with("login: key derivation failed", e))?;

    let finish_body = Zeroizing::new(encode_finish(&challenge.handshake_id, &proof)?);
    let reply = transport
        .post_fresh(FINISH_PATH, Bytes::from_owner(finish_body), MAX_REPLY_BYTES)
        .await?;
    if reply.status != 200 {
        return Err(map_login_failure(&reply));
    }
    let shape = reply_shape(
        reply.content_type_msgpack,
        reply.has_content_encoding,
        reply.body.len(),
    );
    let body = reply.body;
    let decoded = shape.and_then(|()| decode_finish(&body));
    wipe_body(body);
    let mut success = decoded?;

    let expected_signature = build_server_signature(&server_key, &transcript);
    if !constant_time_eq(&success.server_signature, &expected_signature) {
        return Err(ClientError::handshake("login: server signature mismatch"));
    }
    let parsed = parse_token(success.token.as_bytes()).map_err(|_| ClientError::InvalidToken)?;
    // The token exists on the server from here on; its id is not secret, so errors carry it for revocation.
    let issued = parsed.id.to_b64();
    if parsed.id.0 != success.id {
        return Err(ClientError::handshake(format!(
            "login: token id mismatch (issued token {issued})"
        )));
    }
    if success.owner != user.as_str()
        || success.label != scope.label
        || success.mode != scope.mode
        || success.channels != scope.channels
        || success.databases != scope.databases
    {
        return Err(ClientError::handshake(format!(
            "login: scope mismatch (issued token {issued})"
        )));
    }
    if success.expires_at_ns <= success.created_at_ns {
        return Err(ClientError::handshake(format!(
            "login: invalid token expiry (issued token {issued})"
        )));
    }

    let token = ApiToken::new(std::mem::take(&mut *success.token))?;
    Ok(LoginOutcome {
        token,
        id: success.id,
        owner: success.owner,
        scope,
        created_at_ns: success.created_at_ns,
        expires_at_ns: success.expires_at_ns,
    })
}
