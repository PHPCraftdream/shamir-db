//! FINISH proof validation, policy rechecks and durable issuance.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use shamir_connect::common::crypto::constant_time_eq;
use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::scram::ClientProof;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::http_login::{build_server_signature, verify_http_login_proof};
use shamir_connect::server::lockout::LockoutStore;
use shamir_connect::server::token::TokenId;
use zeroize::Zeroizing;

use super::super::admission::Source;
use super::super::metrics::{observe_finish, record_result, LoginResult};
use super::super::pending::PendingId;
use super::super::wire::{
    response, success, FinishRequest, LoginResponse, Success, TokenScope, TokenScopeMode,
};
use super::LoginServiceContext;
use crate::security::tokens::{NewToken, TokenCreateError};

const PAD_BASE: Duration = Duration::from_millis(50);

/// Parse, claim, verify and durably issue a token for a valid proof.
pub async fn finish(context: &LoginServiceContext, source: Source, body: &[u8]) -> LoginResponse {
    let received = (context.monotonic_now)();
    let started = received;
    let jitter = sample_jitter();
    let value = match jitter {
        Some(_) => finish_inner(context, source, body).await,
        None => busy(),
    };
    tokio::time::sleep_until(received + PAD_BASE + jitter.unwrap_or(Duration::ZERO)).await;
    let result = match value.status() {
        StatusCode::OK => LoginResult::Issued,
        StatusCode::FORBIDDEN => LoginResult::PolicyDenied,
        StatusCode::SERVICE_UNAVAILABLE => LoginResult::ServerBusy,
        StatusCode::BAD_REQUEST => LoginResult::InvalidRequest,
        StatusCode::TOO_MANY_REQUESTS => LoginResult::RateLimited,
        _ => LoginResult::AuthenticationFailed,
    };
    record_result("finish", result);
    observe_finish(started.elapsed().as_secs_f64());
    if result != LoginResult::Issued {
        if let Some(audit) = &context.audit {
            audit.failure(result);
        }
    }
    value
}

async fn finish_inner(context: &LoginServiceContext, source: Source, body: &[u8]) -> LoginResponse {
    let request = match FinishRequest::parse(body) {
        Ok(value) => value,
        Err(_) => {
            return response(StatusCode::BAD_REQUEST.as_u16(), "invalid_request", None)
                .expect("fixed response")
        }
    };
    let mut claim = match context
        .pending
        .claim(
            PendingId(request.handshake_id),
            source,
            context.listener_id,
            context.security_class,
        )
        .await
    {
        Ok(value) => value,
        Err(_) => return failed(),
    };
    if !claim.is_live() {
        return failed();
    }
    let Some(entry) = claim.take() else {
        return failed();
    };
    let now = match (context.unix_clock)() {
        Some(value) => value,
        None => return busy(),
    };
    let user_len = usize::from(u16::from_be_bytes([
        entry.transcript[71],
        entry.transcript[72],
    ]));
    let scope = match 166usize
        .checked_add(user_len)
        .and_then(|offset| entry.transcript.get(offset..))
        .and_then(|bytes| TokenScope::from_scope_bytes(bytes).ok())
    {
        Some(value) => value,
        None => return failed(),
    };
    let username = match entry
        .transcript
        .get(73..73 + user_len)
        .and_then(|bytes| core::str::from_utf8(bytes).ok())
    {
        Some(value) => value.to_owned(),
        None => return failed(),
    };
    let pair = context.pair_key(&username, source.address());
    let status = context.lockout.pair_status(pair, now);
    if status.locked_until_ns.is_some()
        || status.backoff_until_ns.is_some()
        || status
            .last_fail_at_ns
            .is_some_and(|last| last > entry.begin_time_ns)
    {
        return failed();
    }
    let proof: ClientProof = request.client_proof;
    let stored = StoredKey(*entry.stored_key);
    if !verify_http_login_proof(&proof, &stored, &entry.transcript)
        || !entry.eligible
        || entry.user_id.is_none()
    {
        context.lockout.register_failure(pair, now);
        metrics::gauge!("shamir_login_http_pair_slots")
            .set(context.lockout.failure_pair_count() as f64);
        return failed();
    }
    if !context.self_service {
        return denied("token_self_service_disabled");
    }
    if context.read_only {
        return denied("login_read_only_replica");
    }
    if scope.mode == TokenScopeMode::ReadWrite && !context.login_allow_read_write {
        return denied("login_write_forbidden");
    }
    let user_id = entry.user_id.expect("verified account");
    let Some(before) = snapshot(context, user_id, &username).await else {
        return failed();
    };
    if before.privileged {
        return denied("login_account_denied");
    }
    if !before.eligible {
        return failed();
    }
    if !constant_time_eq(&before.stored_key[..], &entry.stored_key[..])
        || before.floor != entry.tickets_invalid_before_ns
    {
        return failed();
    }
    let Some(ttl) = context.ttl_seconds() else {
        return busy();
    };
    if !claim.is_live() {
        return failed();
    }
    let created = match context
        .tokens
        .create(NewToken {
            owner_user_id: user_id,
            owner_roles: before.roles.clone(),
            label: scope.label.to_owned(),
            ttl_secs: Some(ttl),
            mode: scope.token_mode(),
            channels: scope.token_channels(),
            databases: scope
                .databases
                .as_ref()
                .map(|values| values.iter().map(|value| (*value).to_owned()).collect()),
        })
        .await
    {
        Ok(value) => value,
        Err(error) => return map_create_error(error),
    };
    if !claim.is_live() {
        return compensate(context, created, now).await;
    }
    if created.record_view.created_at_ns <= before.floor {
        return compensate(context, created, now).await;
    }
    let Some(after) = snapshot(context, user_id, &username).await else {
        return compensate(context, created, now).await;
    };
    if !claim.is_live()
        || !constant_time_eq(&after.stored_key[..], &before.stored_key[..])
        || after.floor != before.floor
        || after.roles != before.roles
        || after.privileged
        || !after.eligible
    {
        return compensate(context, created, now).await;
    }
    let signature = build_server_signature(&entry.server_key, &entry.transcript);
    match success(&Success {
        server_signature: &signature,
        token: created.token.as_str(),
        view: &created.record_view,
        owner: &after.username,
    }) {
        Ok(body) => {
            context.lockout.reset_on_success(pair);
            metrics::gauge!("shamir_login_http_pair_slots")
                .set(context.lockout.failure_pair_count() as f64);
            if let Some(audit) = &context.audit {
                audit.issued(created.record_view.id);
            }
            LoginResponse::new(StatusCode::OK, body)
        }
        Err(_) => compensate(context, created, now).await,
    }
}

struct UserSnapshot {
    username: String,
    roles: Vec<String>,
    stored_key: Zeroizing<[u8; 32]>,
    floor: u64,
    privileged: bool,
    eligible: bool,
}

async fn snapshot(
    context: &LoginServiceContext,
    id: [u8; 16],
    expected_name: &str,
) -> Option<UserSnapshot> {
    let users = Arc::clone(&context.users);
    let expected_name = expected_name.to_owned();
    let hardening = context.hardening;
    let default_kdf = context.default_kdf;
    let span = tracing::Span::current();
    tokio::task::spawn_blocking(move || {
        let _entered = span.enter();
        let state = users.state_by_user_id(&id)?;
        if state.username != expected_name {
            return None;
        }
        let record = UserDirectory::lookup_by_name(users.as_ref(), &expected_name)?;
        let id_by_name = UserDirectory::user_id(users.as_ref(), &expected_name)?;
        if id_by_name != id || record.tickets_invalid_before_ns != state.tickets_invalid_before_ns {
            return None;
        }
        Some(UserSnapshot {
            username: state.username,
            roles: state.roles,
            stored_key: Zeroizing::new(record.stored_key.0),
            floor: state.tickets_invalid_before_ns,
            privileged: state.superuser || state.replicator,
            eligible: record.kdf_params.validate_client_limits().is_ok()
                && (!hardening || record.kdf_params == default_kdf),
        })
    })
    .await
    .ok()
    .flatten()
}

async fn compensate(
    context: &LoginServiceContext,
    created: crate::security::tokens::CreatedToken,
    now: u64,
) -> LoginResponse {
    match context
        .tokens
        .revoke(TokenId(created.record_view.id), now)
        .await
    {
        Ok(_) => failed(),
        Err(_) => {
            if let Some(audit) = &context.audit {
                audit.compensation_failed(created.record_view.id);
            }
            tracing::error!(token_id = ?created.record_view.id, "critical HTTP login token compensation failed");
            busy()
        }
    }
}

fn map_create_error(error: TokenCreateError) -> LoginResponse {
    match error {
        TokenCreateError::LabelExists => denied("token_label_exists"),
        TokenCreateError::LimitReached => denied("token_limit_reached"),
        TokenCreateError::ReadOnlyNode => denied("login_read_only_replica"),
        _ => busy(),
    }
}

fn failed() -> LoginResponse {
    response(
        StatusCode::UNAUTHORIZED.as_u16(),
        "authentication_failed",
        None,
    )
    .expect("fixed response")
}
fn denied(code: &str) -> LoginResponse {
    response(StatusCode::FORBIDDEN.as_u16(), code, None).expect("fixed response")
}
fn busy() -> LoginResponse {
    response(
        StatusCode::SERVICE_UNAVAILABLE.as_u16(),
        "server_busy",
        Some(1),
    )
    .expect("fixed response")
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
