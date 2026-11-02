//! Audit-event coverage for token-admin mutations.

use super::common::{bearer, create, Fixture, OTHER};
use crate::boot::config::TokensConfig;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use shamir_connect::server::token::parse_token;
use shamir_query_types::wire::{DbRequest, DbResponse};

#[tokio::test]
async fn token_mutations_emit_secret_free_audit_events_and_correct_counters() {
    let fixture = Fixture::new(TokensConfig::default(), true).await;
    let (first_id, first_token) = bearer(fixture.call(&fixture.other, &create("first")).await);
    assert!(matches!(
        fixture
            .call(
                &fixture.other,
                &DbRequest::RevokeApiToken {
                    id: first_id.clone(),
                    hmac: None,
                },
            )
            .await,
        DbResponse::ApiTokenRevoked
    ));
    let (second_id, second_token) = bearer(fixture.call(&fixture.other, &create("second")).await);
    assert!(matches!(
        fixture
            .call(
                &fixture.other,
                &DbRequest::RevokeUserTokens {
                    user: Some(OTHER.into()),
                    hmac: None,
                },
            )
            .await,
        DbResponse::ApiUserTokensRevoked { count: 1 }
    ));

    let entries = fixture.audit.entries.lock().expect("audit entries").clone();
    assert_eq!(entries.len(), 4);
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.event.as_str())
            .collect::<Vec<_>>(),
        [
            "api_token_created",
            "api_token_revoked",
            "api_token_created",
            "api_token_user_revoked",
        ]
    );

    let first_token_id = parse_token(first_token.as_bytes())
        .expect("first token")
        .id
        .0;
    let second_token_id = parse_token(second_token.as_bytes())
        .expect("second token")
        .id
        .0;
    assert_eq!(URL_SAFE_NO_PAD.encode(first_token_id), first_id);
    assert_eq!(URL_SAFE_NO_PAD.encode(second_token_id), second_id);
    assert_ne!(first_token_id, second_token_id);

    for index in [0, 1, 2] {
        let details: ([u8; 16], [u8; 16]) =
            rmp_serde::from_slice(&entries[index].details_canonical_msgpack)
                .expect("token id and owner details");
        assert_eq!(
            details.0,
            if index < 2 {
                first_token_id
            } else {
                second_token_id
            }
        );
        assert_eq!(details.1, fixture.other.user_id);
    }
    let user_details: ([u8; 16], Vec<[u8; 16]>) =
        rmp_serde::from_slice(&entries[3].details_canonical_msgpack).expect("user-revoked details");
    assert_eq!(user_details, (fixture.other.user_id, vec![second_token_id]));

    for entry in &entries {
        for sensitive in [first_token.as_bytes(), second_token.as_bytes()] {
            assert!(!entry
                .details_canonical_msgpack
                .windows(sensitive.len())
                .any(|window| window == sensitive));
        }
    }
    assert_eq!(fixture.service.counters().created(), 2);
    assert_eq!(fixture.service.counters().revoked(), 2);
}
