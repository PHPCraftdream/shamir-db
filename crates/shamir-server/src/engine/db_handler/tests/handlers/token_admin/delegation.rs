//! Superuser delegation and foreign-token authorization regressions.

use super::common::{bearer, create, Fixture, OTHER};
use crate::boot::config::TokensConfig;
use shamir_connect::server::token::Endpoint;
use shamir_query_types::wire::{DbRequest, DbResponse};

#[tokio::test]
async fn non_superuser_cannot_create_or_list_for_another_user() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let mut create_other = create("delegated");
    if let DbRequest::CreateApiToken { for_user, .. } = &mut create_other {
        *for_user = Some(OTHER.into());
    }
    Fixture::assert_error(f.call(&f.other, &create_other).await, "permission_denied");
    let list_owner = DbRequest::ListApiTokens {
        for_user: Some(super::common::OWNER.into()),
        hmac: None,
    };
    Fixture::assert_error(f.call(&f.other, &list_owner).await, "permission_denied");
    let list_other = DbRequest::ListApiTokens {
        for_user: Some(OTHER.into()),
        hmac: None,
    };
    Fixture::assert_error(f.call(&f.other, &list_other).await, "permission_denied");
}

#[tokio::test]
async fn superuser_self_revoke_needs_no_hmac_and_invalidates_bearer() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let (id, token_bearer) = bearer(f.call(&f.admin, &create("admin-self")).await);
    assert!(f
        .service
        .verify(token_bearer.as_bytes(), Endpoint::Rest, super::common::NOW)
        .is_ok());
    assert!(matches!(
        f.call(&f.admin, &DbRequest::RevokeApiToken { id, hmac: None })
            .await,
        DbResponse::ApiTokenRevoked
    ));
    assert!(f
        .service
        .verify(token_bearer.as_bytes(), Endpoint::Rest, super::common::NOW)
        .is_err());
}

#[tokio::test]
async fn superuser_self_user_revoke_needs_no_hmac_and_invalidates_bearer() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let (_, token_bearer) = bearer(f.call(&f.admin, &create("admin-owned")).await);
    assert!(f
        .service
        .verify(token_bearer.as_bytes(), Endpoint::Rest, super::common::NOW)
        .is_ok());
    assert!(matches!(
        f.call(
            &f.admin,
            &DbRequest::RevokeUserTokens {
                user: None,
                hmac: None,
            }
        )
        .await,
        DbResponse::ApiUserTokensRevoked { count: 1 }
    ));
    assert!(f
        .service
        .verify(token_bearer.as_bytes(), Endpoint::Rest, super::common::NOW)
        .is_err());
}

#[tokio::test]
async fn superuser_delegated_create_for_unknown_user_returns_not_found() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let mut request = create("unknown-owner");
    if let DbRequest::CreateApiToken { for_user, .. } = &mut request {
        *for_user = Some("unknown-user".into());
    }
    Fixture::sign(&f.admin, &mut request);
    Fixture::assert_error(f.call(&f.admin, &request).await, "not_found");
    assert!(f
        .service
        .list(f.owner.user_id)
        .await
        .expect("owner list")
        .is_empty());
    assert_eq!(
        f.service
            .active_count(super::common::NOW)
            .await
            .expect("active count"),
        0
    );
}

#[tokio::test]
async fn superuser_delegated_list_for_unknown_user_returns_not_found() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let mut request = DbRequest::ListApiTokens {
        for_user: Some("unknown-user".into()),
        hmac: None,
    };
    Fixture::sign(&f.admin, &mut request);
    Fixture::assert_error(f.call(&f.admin, &request).await, "not_found");
}

#[tokio::test]
async fn superuser_delegated_create_requires_valid_confirmation_hmac() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let mut request = create("delegate");
    if let DbRequest::CreateApiToken { for_user, .. } = &mut request {
        *for_user = Some(OTHER.into());
    }
    Fixture::assert_error(f.call(&f.admin, &request).await, "hmac_required");
    assert!(f
        .service
        .list(f.other.user_id)
        .await
        .expect("list after missing HMAC")
        .is_empty());
    assert_eq!(
        f.service
            .active_count(super::common::NOW)
            .await
            .expect("count after missing HMAC"),
        0
    );
    let mut wrong = request.clone();
    if let DbRequest::CreateApiToken { hmac, .. } = &mut wrong {
        *hmac = Some("00".repeat(32));
    }
    Fixture::assert_error(f.call(&f.admin, &wrong).await, "hmac_mismatch");
    assert!(f
        .service
        .list(f.other.user_id)
        .await
        .expect("list after wrong HMAC")
        .is_empty());
    assert_eq!(
        f.service
            .active_count(super::common::NOW)
            .await
            .expect("count after wrong HMAC"),
        0
    );
    Fixture::sign(&f.admin, &mut request);
    let (owner, label, secret) = match f.call(&f.admin, &request).await {
        DbResponse::ApiTokenCreated {
            owner,
            label,
            token,
            ..
        } => (owner, label, token),
        response => panic!("delegated create: {response:?}"),
    };
    assert_eq!(owner, OTHER);
    assert_eq!(label, "delegate");
    assert!(format!("{secret:?}").contains("SecretString(***)"));
}

#[tokio::test]
async fn superuser_delegated_list_requires_confirmation_hmac() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let (id, _) = bearer(f.call(&f.other, &create("owned")).await);
    let mut request = DbRequest::ListApiTokens {
        for_user: Some(OTHER.into()),
        hmac: None,
    };
    Fixture::assert_error(f.call(&f.admin, &request).await, "hmac_required");
    let mut wrong = request.clone();
    if let DbRequest::ListApiTokens { hmac, .. } = &mut wrong {
        *hmac = Some("ff".repeat(32));
    }
    Fixture::assert_error(f.call(&f.admin, &wrong).await, "hmac_mismatch");
    Fixture::sign(&f.admin, &mut request);
    let DbResponse::ApiTokensListed { tokens } = f.call(&f.admin, &request).await else {
        panic!("delegated list")
    };
    assert_eq!(tokens.len(), 1);
    assert_eq!(tokens[0].id, id);
}

#[tokio::test]
async fn revoked_label_can_be_reused_with_new_id_and_bearer() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let (old_id, old_bearer) = bearer(f.call(&f.other, &create("reusable")).await);
    assert!(matches!(
        f.call(
            &f.other,
            &DbRequest::RevokeApiToken {
                id: old_id.clone(),
                hmac: None,
            }
        )
        .await,
        DbResponse::ApiTokenRevoked
    ));
    assert!(f
        .service
        .verify(old_bearer.as_bytes(), Endpoint::Rest, super::common::NOW)
        .is_err());
    let (new_id, new_bearer) = bearer(f.call(&f.other, &create("reusable")).await);
    assert_ne!(new_id, old_id);
    assert!(f
        .service
        .verify(new_bearer.as_bytes(), Endpoint::Rest, super::common::NOW)
        .is_ok());
    assert!(f
        .service
        .verify(old_bearer.as_bytes(), Endpoint::Rest, super::common::NOW)
        .is_err());
}

#[tokio::test]
async fn foreign_revoke_hides_existence_from_non_superuser_and_requires_admin_hmac() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let (foreign_id, token_bearer) = bearer(f.call(&f.other, &create("foreign")).await);
    Fixture::assert_error(
        f.call(
            &f.owner,
            &DbRequest::RevokeApiToken {
                id: foreign_id.clone(),
                hmac: None,
            },
        )
        .await,
        "token_not_found",
    );
    let mut request = DbRequest::RevokeApiToken {
        id: foreign_id.clone(),
        hmac: None,
    };
    Fixture::assert_error(f.call(&f.admin, &request).await, "hmac_required");
    let mut wrong = request.clone();
    if let DbRequest::RevokeApiToken { hmac, .. } = &mut wrong {
        *hmac = Some("11".repeat(32));
    }
    Fixture::assert_error(f.call(&f.admin, &wrong).await, "hmac_mismatch");
    Fixture::sign(&f.admin, &mut request);
    assert!(matches!(
        f.call(&f.admin, &request).await,
        DbResponse::ApiTokenRevoked
    ));
    assert!(f
        .service
        .verify(token_bearer.as_bytes(), Endpoint::Rest, super::common::NOW)
        .is_err());
}

#[tokio::test]
async fn foreign_revoke_user_requires_superuser_and_hmac() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let (_, token_bearer) = bearer(f.call(&f.other, &create("to-revoke")).await);
    assert!(f
        .service
        .verify(token_bearer.as_bytes(), Endpoint::Rest, super::common::NOW)
        .is_ok());
    Fixture::assert_error(
        f.call(
            &f.owner,
            &DbRequest::RevokeUserTokens {
                user: Some(OTHER.into()),
                hmac: None,
            },
        )
        .await,
        "permission_denied",
    );
    let mut request = DbRequest::RevokeUserTokens {
        user: Some(OTHER.into()),
        hmac: None,
    };
    Fixture::assert_error(f.call(&f.admin, &request).await, "hmac_required");
    let mut wrong = request.clone();
    if let DbRequest::RevokeUserTokens { hmac, .. } = &mut wrong {
        *hmac = Some("22".repeat(32));
    }
    Fixture::assert_error(f.call(&f.admin, &wrong).await, "hmac_mismatch");
    Fixture::sign(&f.admin, &mut request);
    assert!(matches!(
        f.call(&f.admin, &request).await,
        DbResponse::ApiUserTokensRevoked { count: 1 }
    ));
    assert!(f
        .service
        .verify(token_bearer.as_bytes(), Endpoint::Rest, super::common::NOW)
        .is_err());
}
