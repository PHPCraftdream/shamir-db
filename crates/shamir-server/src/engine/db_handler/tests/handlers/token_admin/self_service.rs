//! Self-service token issuance, listing and revocation regressions.

use super::common::{bearer, create, Fixture, NOW};
use crate::boot::config::TokensConfig;
use shamir_connect::common::types::BindingMode;
use shamir_connect::server::token::Endpoint;
use shamir_query_types::wire::{DbRequest, DbResponse};

#[tokio::test]
async fn self_create_defaults_once_lists_no_secret_and_revoke_invalidates_bearer() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let created = f.call(&f.other, &create("automation")).await;
    let created_bytes = rmp_serde::to_vec_named(&created).expect("serialize created response");
    let created_dbg = format!("{created:?}");
    let (id, bearer) = bearer(created);
    assert!(!created_dbg.contains(&bearer));
    assert_eq!(
        created_bytes
            .windows(bearer.len())
            .filter(|window| *window == bearer.as_bytes())
            .count(),
        1
    );
    assert!(f
        .service
        .verify(bearer.as_bytes(), Endpoint::Rest, NOW)
        .is_ok());
    let response = f
        .call(
            &f.other,
            &DbRequest::ListApiTokens {
                for_user: None,
                hmac: None,
            },
        )
        .await;
    let DbResponse::ApiTokensListed { tokens } = response else {
        panic!("expected list")
    };
    assert_eq!(tokens.len(), 1);
    assert_eq!(
        tokens[0].mode,
        shamir_query_types::wire::ApiTokenMode::ReadOnly
    );
    assert_eq!(
        tokens[0].channels,
        shamir_query_types::wire::ApiTokenChannels::Both
    );
    assert!(!format!("{:?}", tokens[0]).contains(&bearer));
    assert!(matches!(
        f.call(&f.other, &DbRequest::RevokeApiToken { id, hmac: None })
            .await,
        DbResponse::ApiTokenRevoked
    ));
    assert!(f
        .service
        .verify(bearer.as_bytes(), Endpoint::Rest, NOW)
        .is_err());
}

#[tokio::test]
async fn self_create_enforces_labels_ttl_default_maximum_and_per_user_limit() {
    let f = Fixture::new(
        TokensConfig {
            max_per_user: 3,
            default_ttl_days: 2,
            max_ttl_days: 3,
            ..TokensConfig::default()
        },
        true,
    )
    .await;
    let DbResponse::ApiTokenCreated {
        created_at_ns,
        expires_at_ns,
        mode,
        channels,
        ..
    } = f.call(&f.other, &create("same-label")).await
    else {
        panic!("created")
    };
    assert_eq!(mode, shamir_query_types::wire::ApiTokenMode::ReadOnly);
    assert_eq!(channels, shamir_query_types::wire::ApiTokenChannels::Both);
    assert_eq!(
        expires_at_ns - created_at_ns,
        2 * 24 * 60 * 60 * 1_000_000_000
    );
    Fixture::assert_error(
        f.call(&f.other, &create("same-label")).await,
        "token_label_exists",
    );
    let mut zero = create("zero");
    if let DbRequest::CreateApiToken { ttl_secs, .. } = &mut zero {
        *ttl_secs = Some(0);
    }
    Fixture::assert_error(f.call(&f.other, &zero).await, "token_ttl_invalid");
    let mut beyond = create("beyond");
    if let DbRequest::CreateApiToken { ttl_secs, .. } = &mut beyond {
        *ttl_secs = Some(4 * 24 * 60 * 60);
    }
    Fixture::assert_error(f.call(&f.other, &beyond).await, "token_ttl_invalid");
    let mut max_ttl = create("max-ttl");
    if let DbRequest::CreateApiToken { ttl_secs, .. } = &mut max_ttl {
        *ttl_secs = Some(3 * 24 * 60 * 60);
    }
    assert!(matches!(
        f.call(&f.other, &max_ttl).await,
        DbResponse::ApiTokenCreated { .. }
    ));
    let _ = bearer(f.call(&f.other, &create("third")).await);
    Fixture::assert_error(
        f.call(&f.other, &create("over-limit")).await,
        "token_limit_reached",
    );
}

#[tokio::test]
async fn disabled_self_service_only_blocks_creation() {
    let f = Fixture::new(TokensConfig::default(), false).await;
    let mut delegated = create("admin-created");
    if let DbRequest::CreateApiToken { for_user, .. } = &mut delegated {
        *for_user = Some(super::common::OTHER.into());
    }
    Fixture::sign(&f.admin, &mut delegated);
    let (id, bearer) = bearer(f.call(&f.admin, &delegated).await);
    assert!(f
        .service
        .verify(bearer.as_bytes(), Endpoint::Rest, NOW)
        .is_ok());
    Fixture::assert_error(
        f.call(&f.other, &create("disabled")).await,
        "token_self_service_disabled",
    );
    assert!(matches!(
        f.call(
            &f.other,
            &DbRequest::ListApiTokens {
                for_user: None,
                hmac: None
            }
        )
        .await,
        DbResponse::ApiTokensListed { .. }
    ));
    assert!(matches!(
        f.call(&f.other, &DbRequest::RevokeApiToken { id, hmac: None })
            .await,
        DbResponse::ApiTokenRevoked
    ));
    assert!(f
        .service
        .verify(bearer.as_bytes(), Endpoint::Rest, NOW)
        .is_err());
}

#[tokio::test]
async fn browser_binding_blocks_creation_but_not_read_or_revoke() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let browser = super::common::session(
        f.other.user_id,
        super::common::OTHER,
        false,
        [8; 32],
        BindingMode::TlsNoExport,
    );
    Fixture::assert_error(
        f.call(&browser, &create("browser")).await,
        "token_browser_session",
    );
    let (id, _) = bearer(f.call(&f.other, &create("native")).await);
    assert!(matches!(
        f.call(
            &browser,
            &DbRequest::ListApiTokens {
                for_user: None,
                hmac: None
            }
        )
        .await,
        DbResponse::ApiTokensListed { .. }
    ));
    assert!(matches!(
        f.call(&browser, &DbRequest::RevokeApiToken { id, hmac: None })
            .await,
        DbResponse::ApiTokenRevoked
    ));
}
