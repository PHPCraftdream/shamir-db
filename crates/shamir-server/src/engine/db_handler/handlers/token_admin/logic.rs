//! Token administration implementation and authorization helpers.

use super::audit_context::audit_context;
use crate::db_handler::config::NodeMode;
use crate::db_handler::handler::ShamirDbHandler;
use crate::security::tokens::{CreatedToken, NewToken, TokenCreateError, TokenView};
use crate::security::user_directory::UserDirectoryState;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use shamir_connect::common::types::BindingMode;
use shamir_connect::common::username::NormalizedUsername;
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::session::Session;
use shamir_connect::server::token::{TokenChannels, TokenId, TokenMode};
use shamir_connect::server::SessionOrigin;
use shamir_query_types::hmac as canon;
use shamir_query_types::wire::{
    ApiTokenChannels, ApiTokenMode, ApiTokenView, DbRequest, DbResponse,
};
use std::sync::Arc;

pub(crate) async fn handle(
    handler: &ShamirDbHandler,
    session: &Session,
    request: &DbRequest,
    canonical: Option<&[u8]>,
) -> DbResponse {
    if matches!(&session.origin, SessionOrigin::Token(_)) {
        return error(
            "token_request_denied",
            "token sessions cannot administer tokens",
        );
    }
    match request {
        DbRequest::CreateApiToken { .. } => create(handler, session, request, canonical).await,
        DbRequest::ListApiTokens { for_user, hmac } => {
            list(
                handler,
                session,
                for_user.as_deref(),
                hmac.as_deref(),
                canonical,
            )
            .await
        }
        DbRequest::RevokeApiToken { id, hmac } => {
            revoke(handler, session, id, hmac.as_deref(), canonical).await
        }
        DbRequest::RevokeUserTokens { user, hmac } => {
            revoke_user(
                handler,
                session,
                user.as_deref(),
                hmac.as_deref(),
                canonical,
            )
            .await
        }
        _ => error("invalid_request", "not a token administration request"),
    }
}
async fn create(
    handler: &ShamirDbHandler,
    session: &Session,
    request: &DbRequest,
    canonical: Option<&[u8]>,
) -> DbResponse {
    let DbRequest::CreateApiToken {
        label,
        ttl_secs,
        mode,
        channels,
        databases,
        for_user,
        hmac,
    } = request
    else {
        return error("invalid_request", "not a token creation request");
    };
    if session.binding_mode == BindingMode::TlsNoExport {
        return error(
            "token_browser_session",
            "browser-bound sessions cannot create tokens",
        );
    }
    if handler.node_mode == NodeMode::ReadOnly {
        return error(
            "read_only_replica",
            "token mutation is unavailable on a read-only node",
        );
    }
    let Some(service) = handler.token_service.as_ref() else {
        return error("not_supported", "token administration is unavailable");
    };
    if for_user.is_none() && !handler.allow_self_service {
        return error(
            "token_self_service_disabled",
            "self-service token creation is disabled",
        );
    }
    if for_user.is_some() && !session.permissions.is_superuser {
        return error(
            "permission_denied",
            "creating for another user requires superuser",
        );
    }
    if for_user.is_some() {
        if let Some(r) = verify_hmac(session, hmac.as_deref(), canonical) {
            return r;
        }
    }
    let target = for_user.as_deref().unwrap_or(&session.username);
    let (owner_state, owner_id) = match resolve_user(handler, target).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    if for_user.is_none() && owner_id != session.user_id {
        return error(
            "session_invalidated",
            "session user no longer matches directory",
        );
    }
    if !caller_valid(handler, session) {
        return error("session_invalidated", "session is no longer valid");
    }
    let created = match service
        .create_with_context(
            NewToken {
                owner_user_id: owner_id,
                owner_roles: owner_state.roles.clone(),
                label: label.clone(),
                ttl_secs: *ttl_secs,
                mode: api_token_mode(*mode),
                channels: api_token_channels(*channels),
                databases: databases.clone(),
            },
            audit_context(session),
        )
        .await
    {
        Ok(created) => created,
        Err(err) => return create_error(err),
    };
    finalize_created(handler, session, &owner_state, created).await
}
/// Re-check the session and owner snapshots after issuance; rollback on any invalidation.
pub(in crate::engine::db_handler) async fn finalize_created(
    handler: &ShamirDbHandler,
    session: &Session,
    owner_snapshot: &UserDirectoryState,
    created: CreatedToken,
) -> DbResponse {
    let service = match handler.token_service.as_ref() {
        Some(service) => service,
        None => return rollback_invalidated(handler, session, created).await,
    };
    match recheck_created(handler, session, owner_snapshot, &created).await {
        Ok(true) => created_response(created, &owner_snapshot.username),
        Ok(false) | Err(()) => rollback_invalidated_with(service, session, created).await,
    }
}
/// Recheck caller validity and target owner identity, role, and token floor after issuance.
pub(crate) async fn recheck_created(
    handler: &ShamirDbHandler,
    session: &Session,
    owner_snapshot: &UserDirectoryState,
    created: &CreatedToken,
) -> Result<bool, ()> {
    let Some(directory) = handler
        .admin
        .as_ref()
        .map(|admin| Arc::clone(&admin.user_dir))
    else {
        return Err(());
    };
    let owner_id = owner_snapshot.user_id;
    let fresh_owner = tokio::task::spawn_blocking(move || directory.state_by_user_id(&owner_id))
        .await
        .map_err(|_| ())?;
    let Some(fresh_owner) = fresh_owner else {
        return Ok(false);
    };
    Ok(caller_valid(handler, session)
        && fresh_owner.user_id == owner_snapshot.user_id
        && fresh_owner.roles == owner_snapshot.roles
        && fresh_owner.tickets_invalid_before_ns == owner_snapshot.tickets_invalid_before_ns
        && created.record_view.owner_user_id == owner_snapshot.user_id
        && created.record_view.created_at_ns > fresh_owner.tickets_invalid_before_ns)
}
async fn rollback_invalidated(
    handler: &ShamirDbHandler,
    session: &Session,
    created: CreatedToken,
) -> DbResponse {
    let Some(service) = handler.token_service.as_ref() else {
        return error(
            "internal",
            "created token rollback failed after session invalidation",
        );
    };
    rollback_invalidated_with(service, session, created).await
}
async fn rollback_invalidated_with(
    service: &crate::security::tokens::TokenService,
    session: &Session,
    created: CreatedToken,
) -> DbResponse {
    let at = service
        .now_ns()
        .unwrap_or_else(|| created.record_view.created_at_ns.saturating_add(1));
    if service
        .revoke_with_context(TokenId(created.record_view.id), at, audit_context(session))
        .await
        .is_err()
    {
        return error(
            "internal",
            "created token rollback failed after session invalidation",
        );
    }
    error(
        "session_invalidated",
        "session became invalid during token creation",
    )
}
fn caller_valid(handler: &ShamirDbHandler, session: &Session) -> bool {
    handler
        .admin
        .as_ref()
        .and_then(|admin| admin.user_dir.floor_if_present(&session.user_id))
        .is_some_and(|floor| session.created_at_ns > floor)
}
fn created_response(created: CreatedToken, owner: &str) -> DbResponse {
    let row = created.record_view;
    DbResponse::ApiTokenCreated {
        id: URL_SAFE_NO_PAD.encode(row.id),
        token: shamir_query_types::auth::SecretString::from(created.token.to_string()),
        label: row.label,
        owner: owner.to_owned(),
        mode: api_mode(row.mode),
        channels: api_channels(row.channels),
        databases: row.databases,
        created_at_ns: row.created_at_ns,
        expires_at_ns: row.expires_at_ns,
    }
}
async fn list(
    handler: &ShamirDbHandler,
    session: &Session,
    target: Option<&str>,
    hmac: Option<&str>,
    canonical: Option<&[u8]>,
) -> DbResponse {
    let Some(service) = handler.token_service.as_ref() else {
        return error("not_supported", "token administration is unavailable");
    };
    let (state, owner) = match authorized_target(handler, session, target, hmac, canonical).await {
        Ok(v) => v,
        Err(r) => return r,
    };
    match service.list(owner).await {
        Ok(rows) => DbResponse::ApiTokensListed {
            tokens: rows
                .into_iter()
                .map(|row| api_view(row, &state.username))
                .collect(),
        },
        Err(_) => error("internal", "token listing failed"),
    }
}
async fn revoke(
    handler: &ShamirDbHandler,
    session: &Session,
    encoded: &str,
    hmac: Option<&str>,
    canonical: Option<&[u8]>,
) -> DbResponse {
    if handler.node_mode == NodeMode::ReadOnly {
        return error(
            "read_only_replica",
            "token mutation is unavailable on a read-only node",
        );
    }
    let Some(service) = handler.token_service.as_ref() else {
        return error("not_supported", "token administration is unavailable");
    };
    if encoded.len() != 22 {
        return error("token_not_found", "token not found");
    }
    let Ok(bytes) = URL_SAFE_NO_PAD.decode(encoded) else {
        return error("token_not_found", "token not found");
    };
    let Ok(id) = <[u8; 16]>::try_from(bytes.as_slice()) else {
        return error("token_not_found", "token not found");
    };
    if URL_SAFE_NO_PAD.encode(id) != encoded {
        return error("token_not_found", "token not found");
    }
    let view = match service.lookup(id).await {
        Ok(Some(view)) => view,
        Ok(None) => return error("token_not_found", "token not found"),
        Err(_) => return error("internal", "token lookup failed"),
    };
    if view.owner_user_id != session.user_id {
        if !session.permissions.is_superuser {
            return error("token_not_found", "token not found");
        }
        if let Some(r) = verify_hmac(session, hmac, canonical) {
            return r;
        }
    }
    match service.now_ns() {
        Some(now) => match service
            .revoke_with_context(TokenId(id), now, audit_context(session))
            .await
        {
            Ok(_) => DbResponse::ApiTokenRevoked,
            Err(TokenCreateError::NotFound) => error("token_not_found", "token not found"),
            Err(TokenCreateError::ReadOnlyNode) => error(
                "read_only_replica",
                "token mutation is unavailable on a read-only node",
            ),
            Err(_) => error("internal", "token revocation failed"),
        },
        None => error("internal", "clock unavailable"),
    }
}
async fn revoke_user(
    handler: &ShamirDbHandler,
    session: &Session,
    target: Option<&str>,
    hmac: Option<&str>,
    canonical: Option<&[u8]>,
) -> DbResponse {
    if handler.node_mode == NodeMode::ReadOnly {
        return error(
            "read_only_replica",
            "token mutation is unavailable on a read-only node",
        );
    }
    let Some(service) = handler.token_service.as_ref() else {
        return error("not_supported", "token administration is unavailable");
    };
    let owner = if let Some(name) = target {
        let normalized = match NormalizedUsername::from_raw(name) {
            Ok(value) => value,
            Err(_) => return error("invalid_username", "invalid target username"),
        };
        if normalized.as_str() == session.username {
            session.user_id
        } else {
            if !session.permissions.is_superuser {
                return error(
                    "permission_denied",
                    "targeting another user requires superuser",
                );
            }
            if let Some(response) = verify_hmac(session, hmac, canonical) {
                return response;
            }
            match resolve_user(handler, normalized.as_str()).await {
                Ok((_, id)) => id,
                Err(response) => return response,
            }
        }
    } else {
        session.user_id
    };
    match service.now_ns() {
        Some(now) => match service
            .revoke_user_with_context(owner, now, audit_context(session))
            .await
        {
            Ok(count) => DbResponse::ApiUserTokensRevoked {
                count: count as u64,
            },
            Err(TokenCreateError::ReadOnlyNode) => error(
                "read_only_replica",
                "token mutation is unavailable on a read-only node",
            ),
            Err(_) => error("internal", "token revocation failed"),
        },
        None => error("internal", "clock unavailable"),
    }
}
async fn authorized_target(
    handler: &ShamirDbHandler,
    session: &Session,
    target: Option<&str>,
    hmac: Option<&str>,
    canonical: Option<&[u8]>,
) -> Result<(UserDirectoryState, [u8; 16]), DbResponse> {
    let Some(name) = target else {
        return resolve_user_id(handler, session.user_id)
            .await
            .map(|state| (state, session.user_id));
    };
    if !session.permissions.is_superuser {
        return Err(error(
            "permission_denied",
            "targeting another user requires superuser",
        ));
    }
    if let Some(response) = verify_hmac(session, hmac, canonical) {
        return Err(response);
    }
    resolve_user(handler, name).await
}
async fn resolve_user(
    handler: &ShamirDbHandler,
    username: &str,
) -> Result<(UserDirectoryState, [u8; 16]), DbResponse> {
    let normalized = NormalizedUsername::from_raw(username)
        .map_err(|_| error("invalid_username", "invalid target username"))?;
    let Some(directory) = handler
        .admin
        .as_ref()
        .map(|admin| Arc::clone(&admin.user_dir))
    else {
        return Err(error("not_supported", "user directory is unavailable"));
    };
    let username = normalized.as_str().to_owned();
    let found = tokio::task::spawn_blocking(move || {
        let id = directory.user_id(&username)?;
        let state = directory.state_by_user_id(&id)?;
        Some((state, id))
    })
    .await
    .map_err(|_| error("internal", "user directory lookup failed"))?;
    found.ok_or_else(|| error("not_found", "target user not found"))
}
async fn resolve_user_id(
    handler: &ShamirDbHandler,
    user_id: [u8; 16],
) -> Result<UserDirectoryState, DbResponse> {
    let Some(directory) = handler
        .admin
        .as_ref()
        .map(|admin| Arc::clone(&admin.user_dir))
    else {
        return Err(error("not_supported", "user directory is unavailable"));
    };
    let state = tokio::task::spawn_blocking(move || directory.state_by_user_id(&user_id))
        .await
        .map_err(|_| error("internal", "user directory lookup failed"))?;
    state.ok_or_else(|| error("session_invalidated", "user no longer exists"))
}
fn verify_hmac(
    session: &Session,
    hmac: Option<&str>,
    canonical: Option<&[u8]>,
) -> Option<DbResponse> {
    let Some(bytes) = canonical else {
        return Some(error("hmac_required", "canonical request unavailable"));
    };
    let Some(tag) = hmac else {
        return Some(error("hmac_required", "confirmation HMAC required"));
    };
    if !canon::verify_tag_hex(&session.hmac_key(), bytes, tag) {
        return Some(error("hmac_mismatch", "confirmation HMAC did not match"));
    }
    None
}
fn api_token_mode(mode: Option<ApiTokenMode>) -> TokenMode {
    match mode.unwrap_or(ApiTokenMode::ReadOnly) {
        ApiTokenMode::ReadOnly => TokenMode::ReadOnly,
        ApiTokenMode::ReadWrite => TokenMode::ReadWrite,
    }
}
fn api_token_channels(channels: Option<ApiTokenChannels>) -> TokenChannels {
    match channels.unwrap_or(ApiTokenChannels::Both) {
        ApiTokenChannels::Rest => TokenChannels::Rest,
        ApiTokenChannels::Ws => TokenChannels::Ws,
        ApiTokenChannels::Both => TokenChannels::Both,
    }
}
fn api_mode(mode: TokenMode) -> ApiTokenMode {
    match mode {
        TokenMode::ReadOnly => ApiTokenMode::ReadOnly,
        TokenMode::ReadWrite => ApiTokenMode::ReadWrite,
    }
}
fn api_channels(channels: TokenChannels) -> ApiTokenChannels {
    match channels {
        TokenChannels::Rest => ApiTokenChannels::Rest,
        TokenChannels::Ws => ApiTokenChannels::Ws,
        TokenChannels::Both => ApiTokenChannels::Both,
    }
}
fn api_view(row: TokenView, owner: &str) -> ApiTokenView {
    ApiTokenView {
        id: URL_SAFE_NO_PAD.encode(row.id),
        label: row.label,
        owner: owner.to_owned(),
        mode: api_mode(row.mode),
        channels: api_channels(row.channels),
        databases: row.databases,
        created_at_ns: row.created_at_ns,
        expires_at_ns: row.expires_at_ns,
        revoked_at_ns: row.revoked_at_ns,
    }
}
fn create_error(err: TokenCreateError) -> DbResponse {
    match err {
        TokenCreateError::TokenTtlInvalid => {
            error("token_ttl_invalid", "token lifetime is invalid")
        }
        TokenCreateError::LabelExists => error(
            "token_label_exists",
            "an active token already uses this label",
        ),
        TokenCreateError::LimitReached => {
            error("token_limit_reached", "active token limit reached")
        }
        TokenCreateError::InvalidInput => error("invalid_request", "token fields are invalid"),
        TokenCreateError::ReadOnlyNode => error(
            "read_only_replica",
            "token mutation is unavailable on a read-only node",
        ),
        TokenCreateError::NotFound => error("token_not_found", "token not found"),
        TokenCreateError::Store(_) => error("internal", "token persistence failed"),
        TokenCreateError::IdCollision => error("internal", "token identifier allocation failed"),
        TokenCreateError::Entropy => error("internal", "token generation failed"),
        TokenCreateError::Clock => error("internal", "clock unavailable"),
    }
}
fn error(code: &str, message: &str) -> DbResponse {
    DbResponse::Error {
        code: code.to_owned(),
        message: message.to_owned(),
    }
}
