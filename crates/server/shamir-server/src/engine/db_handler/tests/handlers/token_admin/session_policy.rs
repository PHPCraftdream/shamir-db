//! Session-origin, user targeting, and read-only-node policy regressions.

use super::common::{bearer, create, Fixture, OTHER};
use crate::boot::config::TokensConfig;
use shamir_connect::server::token::Endpoint;
use shamir_query_types::wire::{DbRequest, DbResponse};

#[tokio::test]
async fn token_origin_is_denied_for_all_four_administration_requests() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let (id, token) = bearer(f.call(&f.other, &create("seed")).await);
    let verified = f
        .service
        .verify(token.as_bytes(), Endpoint::Rest, super::common::NOW)
        .unwrap();
    let token_session = shamir_connect::server::session::Session::for_token(
        &verified,
        OTHER.into(),
        super::common::NOW,
        [45; 32],
    );
    let requests = [
        create("blocked"),
        DbRequest::ListApiTokens {
            for_user: None,
            hmac: None,
        },
        DbRequest::RevokeApiToken { id, hmac: None },
        DbRequest::RevokeUserTokens {
            user: None,
            hmac: None,
        },
    ];
    for request in &requests {
        Fixture::assert_error(
            f.call(&token_session, request).await,
            "token_request_denied",
        );
    }
}

#[tokio::test]
async fn foreign_token_revoke_by_non_superuser_is_token_not_found() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let (id, _) = bearer(f.call(&f.other, &create("foreign")).await);
    Fixture::assert_error(
        f.call(
            &f.owner,
            &DbRequest::RevokeApiToken {
                id: id.clone(),
                hmac: None,
            },
        )
        .await,
        "token_not_found",
    );
    let unknown = DbRequest::RevokeApiToken {
        id: "AAAAAAAAAAAAAAAAAAAAAA".into(),
        hmac: None,
    };
    Fixture::assert_error(f.call(&f.owner, &unknown).await, "token_not_found");
}

#[tokio::test]
async fn self_revoke_user_with_explicit_and_omitted_owner_needs_no_hmac() {
    let f = Fixture::new(TokensConfig::default(), true).await;
    let _ = bearer(f.call(&f.other, &create("one")).await);
    let _ = bearer(f.call(&f.other, &create("two")).await);
    assert!(matches!(
        f.call(
            &f.other,
            &DbRequest::RevokeUserTokens {
                user: None,
                hmac: None
            }
        )
        .await,
        DbResponse::ApiUserTokensRevoked { count: 2 }
    ));
    let _ = bearer(f.call(&f.other, &create("three")).await);
    assert!(matches!(
        f.call(
            &f.other,
            &DbRequest::RevokeUserTokens {
                user: Some(OTHER.into()),
                hmac: None
            }
        )
        .await,
        DbResponse::ApiUserTokensRevoked { count: 1 }
    ));
}

#[tokio::test]
async fn read_only_node_rejects_mutations_but_allows_list() {
    let f = Fixture::new(
        TokensConfig {
            max_per_user: 10,
            ..TokensConfig::default()
        },
        true,
    )
    .await;
    let (id, _) = bearer(f.call(&f.other, &create("before-ro")).await);
    Fixture::assert_error(
        f.call_with(&f.read_only_handler, &f.other, &create("read-only"))
            .await,
        "read_only_replica",
    );
    Fixture::assert_error(
        f.call_with(
            &f.read_only_handler,
            &f.other,
            &DbRequest::RevokeApiToken {
                id: id.clone(),
                hmac: None,
            },
        )
        .await,
        "read_only_replica",
    );
    Fixture::assert_error(
        f.call_with(
            &f.read_only_handler,
            &f.other,
            &DbRequest::RevokeUserTokens {
                user: None,
                hmac: None,
            },
        )
        .await,
        "read_only_replica",
    );
    assert!(matches!(
        f.call_with(
            &f.read_only_handler,
            &f.other,
            &DbRequest::ListApiTokens {
                for_user: None,
                hmac: None
            }
        )
        .await,
        DbResponse::ApiTokensListed { .. }
    ));
}
