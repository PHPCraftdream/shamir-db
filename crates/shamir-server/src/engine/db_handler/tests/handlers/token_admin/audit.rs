//! Audit-event coverage for token-admin mutations.

use super::common::{bearer, create, session, Fixture, OTHER, OWNER};
use crate::boot::config::TokensConfig;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use shamir_connect::common::types::{BindingMode, TransportKind};
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
    for entry in &entries {
        assert_eq!(entry.user, fixture.other.username);
        assert_eq!(entry.ip_subnet, "");
        assert_eq!(entry.transport, "tcp");
        assert_eq!(entry.session_id_prefix, [8; 8]);
    }

    assert_eq!(fixture.service.counters().created(), 2);
    assert_eq!(fixture.service.counters().revoked(), 2);
}

#[tokio::test]
async fn audit_records_delegated_actor_transport_and_eight_byte_session_prefix() {
    let fixture = Fixture::new(TokensConfig::default(), true).await;
    let sid = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24,
        25, 26, 27, 28, 29, 30, 31,
    ];
    let mut actor = session(
        fixture.admin.user_id,
        OWNER,
        true,
        sid,
        BindingMode::TlsExporter,
    );
    actor.transport_kind = TransportKind::WebSocket;
    let mut request = create("delegated-audit-create");
    if let DbRequest::CreateApiToken { for_user, .. } = &mut request {
        *for_user = Some(OTHER.into());
    }
    Fixture::sign(&actor, &mut request);
    let (created_id, _) = bearer(fixture.call(&actor, &request).await);
    let mut revoke = DbRequest::RevokeApiToken {
        id: created_id.clone(),
        hmac: None,
    };
    Fixture::sign(&actor, &mut revoke);
    assert!(matches!(
        fixture.call(&actor, &revoke).await,
        DbResponse::ApiTokenRevoked
    ));

    let mut second_request = create("delegated-audit-user-revoke");
    if let DbRequest::CreateApiToken { for_user, .. } = &mut second_request {
        *for_user = Some(OTHER.into());
    }
    Fixture::sign(&actor, &mut second_request);
    let (second_id, _) = bearer(fixture.call(&actor, &second_request).await);
    let mut revoke_user = DbRequest::RevokeUserTokens {
        user: Some(OTHER.into()),
        hmac: None,
    };
    let mut unix_actor = session(
        fixture.admin.user_id,
        OWNER,
        true,
        sid,
        BindingMode::TlsExporter,
    );
    unix_actor.transport_kind = TransportKind::Unix;
    Fixture::sign(&unix_actor, &mut revoke_user);
    assert!(matches!(
        fixture.call(&unix_actor, &revoke_user).await,
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
            "api_token_user_revoked"
        ]
    );
    let first_details: ([u8; 16], [u8; 16]) =
        rmp_serde::from_slice(&entries[0].details_canonical_msgpack).expect("create details");
    let revoke_details: ([u8; 16], [u8; 16]) =
        rmp_serde::from_slice(&entries[1].details_canonical_msgpack).expect("revoke details");
    let second_details: ([u8; 16], [u8; 16]) =
        rmp_serde::from_slice(&entries[2].details_canonical_msgpack)
            .expect("second create details");
    let user_revoke_details: ([u8; 16], Vec<[u8; 16]>) =
        rmp_serde::from_slice(&entries[3].details_canonical_msgpack).expect("user-revoke details");
    for details in [first_details, revoke_details, second_details] {
        assert_eq!(details.1, fixture.other.user_id);
    }
    assert_eq!(first_details.0, revoke_details.0);
    assert_eq!(
        user_revoke_details,
        (fixture.other.user_id, vec![second_details.0])
    );
    assert_eq!(URL_SAFE_NO_PAD.encode(first_details.0), created_id);
    assert_eq!(URL_SAFE_NO_PAD.encode(second_details.0), second_id);
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(entry.user, actor.username);
        assert_ne!(entry.user, fixture.other.username);
        assert_eq!(entry.ip_subnet, "");
        assert_eq!(entry.transport, if index == 3 { "unix" } else { "ws" });
        assert_eq!(entry.session_id_prefix, [0, 1, 2, 3, 4, 5, 6, 7]);
    }
}
