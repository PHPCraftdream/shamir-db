use zeroize::Zeroizing;

use shamir_query_types::wire::{DbRequest, DbResponse};

use crate::client::Client;
use crate::error::ClientError;

/// Compute the session-bound signature for an API-token administration request.
fn admin_request_tag(session_id: &[u8; 32], request: &DbRequest) -> Result<String, ClientError> {
    let canonical = shamir_query_types::hmac::canonical_db_request(request)
        .map_err(|_| ClientError::Protocol("canonical request failed".into()))?;
    let key = shamir_connect::common::crypto::derive_session_hmac_key(session_id);
    Ok(shamir_query_types::hmac::compute_tag_hex(&key, &canonical))
}

#[cfg(test)]
pub(crate) fn admin_request_tag_for_test(
    session_id: &[u8; 32],
    request: &DbRequest,
) -> Result<String, ClientError> {
    admin_request_tag(session_id, request)
}

impl Client {
    /// Create an API token; see TOKEN_API section 12. Requests are always signed; the server remains authoritative for authorization and validation.
    ///
    /// Section-12 error codes surfaced as `ClientError::Db` include:
    /// `permission_denied`, `token_not_found`, `token_browser_session`,
    /// `token_request_denied`, `token_self_service_disabled`, `read_only_replica`,
    /// `hmac_required`, `hmac_mismatch`, `session_invalidated`, `invalid_username`,
    /// `not_found`, `not_supported`, `internal`, `token_label_exists`,
    /// `token_limit_reached`, and `token_ttl_invalid`.
    pub async fn create_api_token(
        &self,
        request: shamir_query_builder::token_admin::CreateApiToken,
    ) -> Result<shamir_query_types::wire::token_admin::CreatedApiToken, ClientError> {
        self.require_scram()?;
        let mut builder = request;
        let unsigned = builder.clone().build();
        builder = builder.hmac(admin_request_tag(&self.session_id, &unsigned)?);
        let req = builder.build();
        let created = match self.roundtrip(&req).await? {
            DbResponse::ApiTokenCreated(created) => created,
            _ => {
                return Err(ClientError::Protocol(
                    "unexpected API token creation response".into(),
                ));
            }
        };
        let token = created.token.reveal().as_bytes();
        if token.len() != 71
            || !token.starts_with(b"shk1.")
            || token.get(27) != Some(&b'.')
            || token.get(5..27) != Some(created.id.as_bytes())
        {
            return Err(ClientError::Protocol(
                "malformed api token in creation response".into(),
            ));
        }
        Ok(created)
    }

    /// List API tokens; see [`Self::create_api_token`] for the section-12 error codes and signing behavior.
    pub async fn list_api_tokens(
        &self,
        for_user: Option<&str>,
    ) -> Result<Vec<shamir_query_types::wire::token_admin::ApiTokenView>, ClientError> {
        self.require_scram()?;
        let mut builder = shamir_query_builder::token_admin::list_api_tokens();
        if let Some(user) = for_user {
            builder = builder.for_user(user);
        }
        let unsigned = builder.clone().build();
        builder = builder.hmac(admin_request_tag(&self.session_id, &unsigned)?);
        match self.roundtrip(&builder.build()).await? {
            DbResponse::ApiTokensListed { tokens } => Ok(tokens),
            _ => Err(ClientError::Protocol(
                "unexpected API token list response".into(),
            )),
        }
    }

    /// Revoke an API token; see [`Self::create_api_token`] for the section-12 error codes and signing behavior.
    pub async fn revoke_api_token(&self, id: &str) -> Result<(), ClientError> {
        self.require_scram()?;
        let mut builder = shamir_query_builder::token_admin::revoke_api_token(id);
        let unsigned = builder.clone().build();
        builder = builder.hmac(admin_request_tag(&self.session_id, &unsigned)?);
        match self.roundtrip(&builder.build()).await? {
            DbResponse::ApiTokenRevoked => Ok(()),
            _ => Err(ClientError::Protocol(
                "unexpected API token revoke response".into(),
            )),
        }
    }

    /// Revoke a user's API tokens; see [`Self::create_api_token`] for the section-12 error codes and signing behavior.
    pub async fn revoke_user_tokens(&self, user: Option<&str>) -> Result<u64, ClientError> {
        self.require_scram()?;
        let mut builder = shamir_query_builder::token_admin::revoke_user_tokens();
        if let Some(user) = user {
            builder = builder.user(user);
        }
        let unsigned = builder.clone().build();
        builder = builder.hmac(admin_request_tag(&self.session_id, &unsigned)?);
        match self.roundtrip(&builder.build()).await? {
            DbResponse::ApiUserTokensRevoked { count } => Ok(count),
            _ => Err(ClientError::Protocol(
                "unexpected user token revoke response".into(),
            )),
        }
    }
}

impl Client {
    /// Create a SCRAM-authenticatable user. Requires the current
    /// session to be a superuser (server enforces). Returns the
    /// stable 16-byte `user_id`.
    ///
    /// The HMAC confirmation tag required by the server (task #604) is
    /// computed automatically from `self.session_id` — callers don't need
    /// to know about the HMAC plumbing.
    pub async fn create_scram_user(
        &self,
        name: &str,
        password: Zeroizing<String>,
        roles: Vec<String>,
    ) -> Result<Vec<u8>, ClientError> {
        self.require_scram()?;
        let mut req = DbRequest::CreateScramUser {
            name: name.to_string(),
            password: password.as_str().to_owned().into(),
            roles,
            hmac: None,
        };
        let tag = {
            let key = shamir_connect::common::crypto::derive_session_hmac_key(&self.session_id);
            let canonical =
                shamir_query_types::hmac::canonical_db_request(&req).map_err(|error| {
                    ClientError::Protocol(format!("canonical request failed: {error:?}"))
                })?;
            shamir_query_types::hmac::compute_tag_hex(&key, &canonical)
        };
        if let DbRequest::CreateScramUser { hmac, .. } = &mut req {
            *hmac = Some(tag);
        }
        let result = self.roundtrip(&req).await;
        // Wipe the cleartext password copy placed into the request
        // immediately after the roundtrip, rather than waiting for `req`'s
        // natural end-of-function drop. (The caller's `password` is
        // `Zeroizing` and wipes on its own drop.) `SecretString` already
        // zeroizes its buffer on `Drop` under the `crypto` feature, so
        // dropping `req` here achieves the same "wipe ASAP" intent that the
        // old manual `password.zeroize()` field-reach used to.
        drop(req);
        match result? {
            DbResponse::UserCreated { user_id, .. } => Ok(user_id),
            other => Err(ClientError::Protocol(format!(
                "expected UserCreated, got {other:?}"
            ))),
        }
    }

    /// Grant or revoke the `replicator` role on an existing SCRAM user
    /// (task #921). The `replicator` pseudo-role is reserved — it cannot be
    /// attached through [`create_scram_user`]'s generic `roles` array; the
    /// server explicitly rejects it there and requires this dedicated
    /// [`DbRequest::SetReplicator`] wire op instead (mirrors
    /// `SetSuperuser`'s shape and unconditional-HMAC gate exactly).
    ///
    /// Requires the current session to belong to a superuser (server enforces,
    /// checked before the HMAC gate). The confirmation tag is computed from
    /// `self.session_id` — callers don't need to know about the HMAC plumbing.
    pub async fn set_replicator(&self, user: &str, on: bool) -> Result<(), ClientError> {
        self.require_scram()?;
        let mut req = DbRequest::SetReplicator {
            user: user.to_string(),
            on,
            hmac: None,
        };
        let tag = {
            let key = shamir_connect::common::crypto::derive_session_hmac_key(&self.session_id);
            let canonical =
                shamir_query_types::hmac::canonical_db_request(&req).map_err(|error| {
                    ClientError::Protocol(format!("canonical request failed: {error:?}"))
                })?;
            shamir_query_types::hmac::compute_tag_hex(&key, &canonical)
        };
        if let DbRequest::SetReplicator { hmac, .. } = &mut req {
            *hmac = Some(tag);
        }
        match self.roundtrip(&req).await? {
            DbResponse::ReplicatorSet { .. } => Ok(()),
            other => Err(ClientError::Protocol(format!(
                "expected ReplicatorSet, got {other:?}"
            ))),
        }
    }
}

impl Client {
    /// Read-only operator status observation (admin-shell PR3, P1).
    ///
    /// Named `operator_status`: `Client::status` is already taken by the
    /// schema-migration status read (`receipts/schema_migration/status.rs`)
    /// and two inherent impls cannot repeat a method name.
    ///
    /// Superuser-class, SCRAM-session only: a token session is refused
    /// client-side before any request is sent (`require_scram`; the
    /// server perimeter also denies it with `token_request_denied`).
    /// The snapshot carries what the server observed at boot plus live
    /// telemetry; the replication gap is a merged, non-authoritative
    /// aggregate (`gap_versions_merged`) and the per-repository age is
    /// always `unknown`.
    pub async fn operator_status(
        &self,
    ) -> Result<shamir_query_types::wire::db_message::OperatorStatus, ClientError> {
        self.require_scram()?;
        let req = shamir_query_builder::wire::admin::status();
        match self.roundtrip(&req).await? {
            DbResponse::Status { status } => Ok(status),
            _ => Err(ClientError::Protocol("expected Status response".into())),
        }
    }
}
