//! BEGIN challenge creation with fake-verifier parity and body-receipt padding.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use shamir_connect::common::fake_blob::FakeBlob;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::config::ServerSecrets;
use shamir_connect::server::http_login::{build_transcript, HttpLoginTranscriptInput};
use shamir_connect::server::lockout::LockoutStore;
use shamir_connect::server::user_record::UserRecord;
use tokio::time::Instant;
use zeroize::Zeroizing;

use super::super::admission::Source;
use super::super::metrics::{observe_begin, record_result, LoginResult};
use super::super::pending::{PendingEntry, PendingError, PendingId};
use super::super::wire::{
    challenge, response, BeginRequest, Challenge, LoginResponse, TokenScopeMode,
};
use super::LoginServiceContext;
use crate::security::user_directory::FjallUserDirectory;

type VerifierSnapshot = (Option<UserRecord>, Option<[u8; 16]>, u64, FakeBlob);

const PAD_BASE: Duration = Duration::from_millis(50);

/// Handle a BEGIN body, padding every response from body receipt.
pub async fn begin(context: &LoginServiceContext, source: Source, body: &[u8]) -> LoginResponse {
    let received = (context.monotonic_now)();
    let request = match BeginRequest::parse(body) {
        Ok(request) => request,
        Err(_) => {
            let jitter = sample_jitter();
            return finish(
                context,
                received,
                jitter.map_or_else(busy, |_| invalid()),
                jitter,
            )
            .await;
        }
    };
    if let Some(denial) = policy_denial(context, &request) {
        let jitter = sample_jitter();
        let denial = jitter.map_or_else(busy, |_| denial);
        return finish(context, received, denial, jitter).await;
    }
    let jitter = sample_jitter();
    let mut generate_id = unique_id;
    let value = match jitter {
        Some(_) => begin_inner(context, source, request, &mut generate_id).await,
        None => busy(),
    };
    finish(context, received, value, jitter).await
}

async fn finish(
    context: &LoginServiceContext,
    received: Instant,
    value: LoginResponse,
    jitter: Option<Duration>,
) -> LoginResponse {
    tokio::time::sleep_until(received + PAD_BASE + jitter.unwrap_or(Duration::ZERO)).await;
    observe_begin(received.elapsed().as_secs_f64());
    let result = match value.status() {
        StatusCode::OK => LoginResult::Challenge,
        StatusCode::BAD_REQUEST => LoginResult::InvalidRequest,
        StatusCode::FORBIDDEN => LoginResult::PolicyDenied,
        StatusCode::TOO_MANY_REQUESTS => LoginResult::RateLimited,
        StatusCode::SERVICE_UNAVAILABLE => LoginResult::ServerBusy,
        _ => LoginResult::AuthenticationFailed,
    };
    record_result("begin", result);
    if value.status() != StatusCode::OK {
        if let Some(audit) = &context.audit {
            audit.failure(result);
        }
    }
    value
}

fn policy_denial(
    context: &LoginServiceContext,
    request: &BeginRequest<'_>,
) -> Option<LoginResponse> {
    if request.scope.mode == TokenScopeMode::ReadWrite && !context.login_allow_read_write {
        return Some(denied("login_write_forbidden"));
    }
    if !context.self_service {
        return Some(denied("token_self_service_disabled"));
    }
    if context.read_only {
        return Some(denied("login_read_only_replica"));
    }
    None
}

async fn begin_inner<F>(
    context: &LoginServiceContext,
    source: Source,
    request: BeginRequest<'_>,
    generate_id: &mut F,
) -> LoginResponse
where
    F: FnMut() -> Option<[u8; 32]>,
{
    let username = match NormalizedUsername::from_raw(request.user) {
        Ok(value) if value.as_str().as_bytes() == request.user.as_bytes() => value,
        _ => return invalid(),
    };
    let now = match (context.unix_clock)() {
        Some(value) => value,
        None => return busy(),
    };
    let pair = context.pair_key(username.as_str(), source.address());
    let pair_status = context.lockout.pair_status(pair, now);
    if let Some(until) = pair_status
        .locked_until_ns
        .into_iter()
        .chain(pair_status.backoff_until_ns)
        .max()
    {
        let retry = until
            .saturating_sub(now)
            .div_ceil(1_000_000_000)
            .clamp(1, 3_600) as u32;
        return response(
            StatusCode::TOO_MANY_REQUESTS.as_u16(),
            "rate_limited",
            Some(retry),
        )
        .expect("fixed response");
    }
    let (real, user_id, threshold, fake) = match load_snapshot(
        Arc::clone(&context.users),
        username.clone(),
        Arc::clone(&context.secrets),
        tracing::Span::current(),
    )
    .await
    {
        Ok(value) => value,
        Err(()) => return busy(),
    };
    let threshold = real
        .as_ref()
        .map_or(threshold, |record| record.tickets_invalid_before_ns);
    let eligible = real.as_ref().is_some_and(|record| {
        user_id.is_some()
            && tuple_eligible(&record.kdf_params, &context.default_kdf, context.hardening)
    });
    let (salt, stored_key, server_key, kdf) = if eligible {
        let record = real.as_ref().expect("eligible account snapshot");
        (
            record.salt,
            record.stored_key.0,
            Zeroizing::new(*record.server_key),
            record.kdf_params,
        )
    } else {
        (
            fake.salt,
            fake.stored_key.0,
            fake.server_key,
            context.default_kdf,
        )
    };
    let nonce = match server_nonce(request.client_nonce) {
        Some(value) => value,
        None => return busy(),
    };
    let scope = match request.scope.encode_scope() {
        Ok(value) => value,
        Err(_) => return invalid(),
    };
    for _ in 0..3 {
        let Some(id) = generate_id() else {
            return busy();
        };
        let transcript = match build_transcript(&HttpLoginTranscriptInput {
            protocol_version: 1,
            binding_mode: 3,
            security: context.security_class,
            listener_id: &context.listener_id,
            handshake_id: &id,
            user: request.user,
            client_nonce: &request.client_nonce,
            server_nonce: &nonce,
            salt: &salt,
            memory_kb: kdf.memory_kb,
            time: kdf.time,
            parallelism: kdf.parallelism,
            argon2_version: kdf.argon2_version,
            scope_bytes: &scope,
        }) {
            Ok(value) => value,
            Err(_) => return invalid(),
        };
        let pins = match context.admission.pin_source(source).await {
            Some(pins) => pins,
            // The unique pinned-bucket budget is exhausted: fail closed.
            None => return busy(),
        };
        match context
            .pending
            .insert_pinned(
                PendingId(id),
                PendingEntry {
                    transcript: Zeroizing::new(transcript),
                    user_id,
                    stored_key: Zeroizing::new(stored_key),
                    server_key: Zeroizing::new(*server_key),
                    tickets_invalid_before_ns: threshold,
                    begin_time_ns: now,
                    eligible,
                    listener_id: context.listener_id,
                    security_class: context.security_class,
                    source,
                },
                pins,
            )
            .await
        {
            Ok(()) => {
                return match challenge(&Challenge {
                    security: context.security_class,
                    listener_id: &context.listener_id,
                    handshake_id: &id,
                    salt: &salt,
                    memory_kb: kdf.memory_kb,
                    time: kdf.time,
                    parallelism: kdf.parallelism,
                    server_nonce: &nonce,
                }) {
                    Ok(body) => LoginResponse::new(StatusCode::OK, body),
                    Err(_) => busy(),
                };
            }
            Err(PendingError::Collision) => continue,
            Err(PendingError::Full) => {
                return response(
                    StatusCode::TOO_MANY_REQUESTS.as_u16(),
                    "rate_limited",
                    Some(1),
                )
                .expect("fixed response");
            }
            Err(_) => return busy(),
        }
    }
    busy()
}

async fn load_snapshot(
    users: Arc<FjallUserDirectory>,
    username: NormalizedUsername,
    secrets: Arc<ServerSecrets>,
    span: tracing::Span,
) -> Result<VerifierSnapshot, ()> {
    let dispatcher = tracing::dispatcher::get_default(Clone::clone);
    tokio::task::spawn_blocking(move || -> Result<VerifierSnapshot, ()> {
        tracing::dispatcher::with_default(&dispatcher, || {
            let _entered = span.enter();
            let real = users.lookup_by_name(username.as_str());
            let user_id = users.user_id(username.as_str());
            let floor = user_id
                .and_then(|id| users.floor_if_present(&id))
                .unwrap_or(0);
            let fake = FakeBlob::derive(&secrets.server_secret, &username).map_err(|_| ())?;
            Ok((real, user_id, floor, fake))
        })
    })
    .await
    .unwrap_or(Err(()))
}

fn unique_id() -> Option<[u8; 32]> {
    shamir_connect::common::crypto::try_random_array::<32>().ok()
}

#[cfg(test)]
pub(crate) async fn begin_with_id_generator<F>(
    context: &LoginServiceContext,
    source: Source,
    body: &[u8],
    mut generate_id: F,
) -> LoginResponse
where
    F: FnMut() -> Option<[u8; 32]>,
{
    let request = match BeginRequest::parse(body) {
        Ok(request) => request,
        Err(_) => return invalid(),
    };
    if let Some(denial) = policy_denial(context, &request) {
        return denial;
    }
    begin_inner(context, source, request, &mut generate_id).await
}

fn server_nonce(client_nonce: [u8; 32]) -> Option<[u8; 32]> {
    for _ in 0..3 {
        let nonce = shamir_connect::common::crypto::try_random_array::<32>().ok()?;
        if nonce != client_nonce && nonce.iter().any(|byte| *byte != 0) {
            return Some(nonce);
        }
    }
    None
}

fn tuple_eligible(actual: &KdfParams, default: &KdfParams, hardening: bool) -> bool {
    actual.validate_client_limits().is_ok() && (!hardening || actual == default)
}

fn sample_jitter() -> Option<Duration> {
    loop {
        let bytes = shamir_connect::common::crypto::try_random_array::<2>().ok()?;
        let sample = u16::from_be_bytes(bytes);
        if sample < 65_520 {
            return Some(Duration::from_millis(u64::from(sample % 26)));
        }
    }
}

fn invalid() -> LoginResponse {
    response(StatusCode::BAD_REQUEST.as_u16(), "invalid_request", None).expect("fixed response")
}
fn busy() -> LoginResponse {
    response(
        StatusCode::SERVICE_UNAVAILABLE.as_u16(),
        "server_busy",
        Some(1),
    )
    .expect("fixed response")
}
fn denied(code: &str) -> LoginResponse {
    response(StatusCode::FORBIDDEN.as_u16(), code, None).expect("fixed response")
}
