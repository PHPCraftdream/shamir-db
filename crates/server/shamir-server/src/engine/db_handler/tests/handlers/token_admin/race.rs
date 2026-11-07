//! Deterministic regressions for post-issuance token finalization races.

use super::common::{Fixture, NOW, OTHER, OWNER};
use crate::boot::config::TokensConfig;
use crate::engine::db_handler::handlers::token_admin::finalize_created;
use crate::security::tokens::{CreatedToken, NewToken, TokenAuditContext};
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::token::{TokenChannels, TokenId, TokenMode};
use shamir_query_types::wire::DbResponse;

async fn create_for(f: &Fixture, label: &str) -> CreatedToken {
    let state = f
        .directory
        .state_by_user_id(&f.other.user_id)
        .expect("target state");
    f.service
        .create_with_context(
            NewToken {
                owner_user_id: state.user_id,
                owner_roles: state.roles.clone(),
                label: label.into(),
                ttl_secs: None,
                mode: TokenMode::ReadOnly,
                channels: TokenChannels::Both,
                databases: None,
            },
            TokenAuditContext::default(),
        )
        .await
        .expect("token create")
}

async fn assert_revoked(f: &Fixture, id: [u8; 16]) {
    let retained = f
        .service
        .lookup(id)
        .await
        .expect("lookup")
        .expect("retained record");
    assert_eq!(retained.revoked_at_ns, Some(NOW));
    assert!(retained
        .revoked_at_ns
        .is_some_and(|revoked_at| revoked_at >= retained.created_at_ns));
    assert!(f
        .service
        .mirror()
        .get(&TokenId(id))
        .is_some_and(|entry| entry.is_revoked()));
}

#[tokio::test]
async fn target_floor_advanced_after_creation_invalidates_and_revokes() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let snapshot = f
        .directory
        .state_by_user_id(&f.other.user_id)
        .expect("target state");
    let created = create_for(&f, "floor-race").await;
    let id = created.record_view.id;
    f.directory
        .bump_tickets_invalid(OTHER, NOW - 1)
        .expect("bump target floor");
    assert!(
        matches!(finalize_created(&f.handler, &f.other, &snapshot, created).await,
        DbResponse::Error { code, .. } if code == "session_invalidated")
    );
    assert_revoked(&f, id).await;
}

#[tokio::test]
async fn caller_floor_advanced_invalidates_delegated_creation() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let snapshot = f
        .directory
        .state_by_user_id(&f.other.user_id)
        .expect("target state");
    let created = create_for(&f, "caller-floor-race").await;
    let id = created.record_view.id;
    f.directory
        .bump_tickets_invalid(OWNER, NOW - 1)
        .expect("bump caller floor");
    assert!(
        matches!(finalize_created(&f.handler, &f.admin, &snapshot, created).await,
        DbResponse::Error { code, .. } if code == "session_invalidated")
    );
    assert_revoked(&f, id).await;
}

#[tokio::test]
async fn changed_target_roles_invalidate_delegated_creation() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let snapshot = f
        .directory
        .state_by_user_id(&f.other.user_id)
        .expect("target state");
    let created = create_for(&f, "role-race").await;
    let id = created.record_view.id;
    f.directory
        .grant_role(OTHER, "changed", NOW - 1)
        .expect("change target roles");
    assert!(
        matches!(finalize_created(&f.handler, &f.admin, &snapshot, created).await,
        DbResponse::Error { code, .. } if code == "session_invalidated")
    );
    assert_revoked(&f, id).await;
}
