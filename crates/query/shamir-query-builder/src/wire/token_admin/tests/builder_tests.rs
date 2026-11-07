use crate::wire::token_admin::{
    create_api_token, list_api_tokens, revoke_api_token, revoke_user_tokens,
};
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};
use shamir_query_types::wire::DbRequest;

#[test]
fn api_token_builders_match_literal_wire_requests() {
    // Hand-built literals are the wire-format oracle for these builders.
    let create = create_api_token("abc").build();
    let expected = DbRequest::CreateApiToken {
        label: "abc".into(),
        ttl_secs: None,
        mode: None,
        channels: None,
        databases: None,
        for_user: None,
        hmac: None,
    };
    let golden = vec![
        0x88, 0xa2, b'o', b'p', 0xb0, b'c', b'r', b'e', b'a', b't', b'e', b'_', b'a', b'p', b'i',
        b'_', b't', b'o', b'k', b'e', b'n', 0xa5, b'l', b'a', b'b', b'e', b'l', 0xa3, b'a', b'b',
        b'c', 0xa8, b't', b't', b'l', b'_', b's', b'e', b'c', b's', 0xc0, 0xa4, b'm', b'o', b'd',
        b'e', 0xc0, 0xa8, b'c', b'h', b'a', b'n', b'n', b'e', b'l', b's', 0xc0, 0xa9, b'd', b'a',
        b't', b'a', b'b', b'a', b's', b'e', b's', 0xc0, 0xa8, b'f', b'o', b'r', b'_', b'u', b's',
        b'e', b'r', 0xc0, 0xa4, b'h', b'm', b'a', b'c', 0xc0,
    ];
    assert_eq!(rmp_serde::to_vec_named(&create).unwrap(), golden);
    assert_eq!(
        rmp_serde::to_vec_named(&create).unwrap(),
        rmp_serde::to_vec_named(&expected).unwrap()
    );

    let create = create_api_token("x")
        .ttl_secs(9)
        .mode(ApiTokenMode::ReadWrite)
        .channels(ApiTokenChannels::Both)
        .databases(["db"])
        .for_user(" raw ")
        .hmac("tag")
        .build();
    let expected = DbRequest::CreateApiToken {
        label: "x".into(),
        ttl_secs: Some(9),
        mode: Some(ApiTokenMode::ReadWrite),
        channels: Some(ApiTokenChannels::Both),
        databases: Some(vec!["db".into()]),
        for_user: Some(" raw ".into()),
        hmac: Some("tag".into()),
    };
    assert_eq!(
        rmp_serde::to_vec_named(&create).unwrap(),
        rmp_serde::to_vec_named(&expected).unwrap()
    );

    let cases = [
        (
            list_api_tokens().for_user(" raw ").hmac("tag").build(),
            DbRequest::ListApiTokens {
                for_user: Some(" raw ".into()),
                hmac: Some("tag".into()),
            },
        ),
        (
            revoke_api_token("id").hmac("tag").build(),
            DbRequest::RevokeApiToken {
                id: "id".into(),
                hmac: Some("tag".into()),
            },
        ),
        (
            revoke_user_tokens().user(" raw ").hmac("tag").build(),
            DbRequest::RevokeUserTokens {
                user: Some(" raw ".into()),
                hmac: Some("tag".into()),
            },
        ),
    ];
    for (actual, expected) in cases {
        assert_eq!(
            rmp_serde::to_vec_named(&actual).unwrap(),
            rmp_serde::to_vec_named(&expected).unwrap()
        );
    }
}

#[test]
fn create_api_token_some_empty_databases_differs_from_none() {
    let all = create_api_token("x").build();
    let none = create_api_token("x")
        .databases(Vec::<String>::new())
        .build();
    assert!(matches!(
        &all,
        DbRequest::CreateApiToken {
            databases: None,
            ..
        }
    ));
    assert!(matches!(
        &none,
        DbRequest::CreateApiToken {
            databases: Some(databases),
            ..
        } if databases.is_empty()
    ));
    assert_ne!(
        rmp_serde::to_vec_named(&all).unwrap(),
        rmp_serde::to_vec_named(&none).unwrap()
    );
}

#[test]
fn token_admin_builders_follow_shared_hmac_vector_fields() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../../shamir-query-types/test-vectors/admin_hmac_v2.json"
    ))
    .unwrap();
    let vectors = fixture["vectors"].as_array().unwrap();
    let vector = |op: &str| vectors.iter().find(|v| v["op"] == op).unwrap();

    let fields = &vector("create_api_token")["fields"];
    let mode = match fields["mode"].as_str().unwrap() {
        "read_only" => ApiTokenMode::ReadOnly,
        "read_write" => ApiTokenMode::ReadWrite,
        value => panic!("unexpected mode: {value}"),
    };
    let channels = match fields["channels"].as_str().unwrap() {
        "rest" => ApiTokenChannels::Rest,
        "ws" => ApiTokenChannels::Ws,
        "both" => ApiTokenChannels::Both,
        value => panic!("unexpected channels: {value}"),
    };
    let create = create_api_token(fields["label"].as_str().unwrap())
        .ttl_secs(fields["ttl_secs"].as_str().unwrap().parse().unwrap())
        .mode(mode)
        .channels(channels)
        .databases(
            fields["databases"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap()),
        )
        .for_user(fields["for_user"].as_str().unwrap())
        .build();
    assert!(matches!(
        create,
        DbRequest::CreateApiToken {
            ref label,
            ttl_secs: Some(9007199254740993),
            mode: Some(ApiTokenMode::ReadWrite),
            channels: Some(ApiTokenChannels::Both),
            ref databases,
            for_user: Some(ref user),
            ..
        } if label == "café-token"
            && databases.as_ref().unwrap() == &["café-db", "db"]
            && user == "bob"
    ));

    let fields = &vector("list_api_tokens")["fields"];
    let list = list_api_tokens()
        .for_user(fields["for_user"].as_str().unwrap())
        .build();
    assert!(matches!(
        list,
        DbRequest::ListApiTokens {
            for_user: Some(ref user),
            ..
        } if user == "bob"
    ));

    let fields = &vector("revoke_api_token")["fields"];
    let revoke = revoke_api_token(fields["id"].as_str().unwrap()).build();
    assert!(matches!(
        revoke,
        DbRequest::RevokeApiToken { ref id, .. } if id == "AAECAwQFBgcICQoLDA0ODw"
    ));

    let fields = &vector("revoke_user_tokens")["fields"];
    let revoke = if fields["user"].is_null() {
        revoke_user_tokens().build()
    } else {
        revoke_user_tokens()
            .user(fields["user"].as_str().unwrap())
            .build()
    };
    assert!(
        matches!(&revoke, DbRequest::RevokeUserTokens { user: None, .. })
            && fields["user"].is_null()
    );
}
