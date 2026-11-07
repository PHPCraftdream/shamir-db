use crate::http_rest::common::{Harness, ADMIN_PASSWORD};
use crate::token_matrix::support::RawScram;
use shamir_query_builder::token_admin;
use shamir_query_types::auth::SecretString;
use shamir_query_types::hmac::{canonical_db_request, compute_tag_hex, derive_session_hmac_key};
use shamir_query_types::wire::{DbRequest, DbResponse};

fn create_for_user(label: &str, username: &str, hmac: Option<String>) -> DbRequest {
    DbRequest::CreateApiToken {
        label: label.into(),
        ttl_secs: Some(3600),
        mode: None,
        channels: None,
        databases: None,
        for_user: Some(username.into()),
        hmac,
    }
}

fn sign_create(session: &RawScram, label: &str, username: &str) -> DbRequest {
    let unsigned = create_for_user(label, username, None);
    let key = derive_session_hmac_key(&session.session_id());
    let tag = compute_tag_hex(
        &key,
        &canonical_db_request(&unsigned).expect("canonical create token"),
    );
    create_for_user(label, username, Some(tag))
}

fn decoded(bytes: &[u8]) -> DbResponse {
    rmp_serde::from_slice(bytes).expect("decode raw DbResponse bytes")
}

fn count_occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    assert!(!needle.is_empty());
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

async fn shutdown(harness: Harness) {
    let report = tokio::time::timeout(
        std::time::Duration::from_secs(45),
        harness.handle.shutdown(),
    )
    .await
    .expect("server shutdown timeout");
    assert!(
        report.errors().is_empty(),
        "shutdown errors: {:?}",
        report.errors()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn for_user_requires_hmac_and_signed_control_uses_same_raw_scram_fixture() {
    let harness = Harness::launch_with_native_seed().await;
    let mut admin = RawScram::connect(&harness, "admin", ADMIN_PASSWORD).await;
    let create_target = DbRequest::CreateScramUser {
        name: "wire-target".into(),
        password: SecretString::from("wire-target-password".to_owned()),
        roles: vec![],
        hmac: None,
    };
    let create_target_tag = compute_tag_hex(
        &derive_session_hmac_key(&admin.session_id()),
        &canonical_db_request(&create_target).expect("canonical target creation"),
    );
    let create_target = DbRequest::CreateScramUser {
        name: "wire-target".into(),
        password: SecretString::from("wire-target-password".to_owned()),
        roles: vec![],
        hmac: Some(create_target_tag),
    };
    let target_response = decoded(&admin.request_raw(&create_target).await);
    assert!(matches!(
        target_response,
        DbResponse::UserCreated { ref name, .. } if name == "wire-target"
    ));
    let unsigned = decoded(
        &admin
            .request_raw(&create_for_user("unsigned-control", "wire-target", None))
            .await,
    );
    assert!(matches!(unsigned, DbResponse::Error { ref code, .. } if code == "hmac_required"));

    let signed_request = sign_create(&admin, "signed-control", "wire-target");
    let signed_raw = admin.request_raw(&signed_request).await;
    let signed = decoded(&signed_raw);
    assert!(
        matches!(signed, DbResponse::ApiTokenCreated(ref token) if token.owner == "wire-target"),
        "signed control failed: {signed:?}"
    );
    drop(admin);
    shutdown(harness).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn create_token_wire_reveals_secret_once_and_wire_list_omits_it() {
    let harness = Harness::launch_with_native_seed().await;
    let mut admin = RawScram::connect(&harness, "admin", ADMIN_PASSWORD).await;
    let created_raw = admin
        .request_raw(&token_admin::create_api_token("wire-once").build())
        .await;
    let created: DbResponse = decoded(&created_raw);
    let DbResponse::ApiTokenCreated(token) = created else {
        panic!("create failed: {created:?}");
    };
    let token_text = token.token.reveal();
    let token_bytes = token_text.as_bytes();
    assert_eq!(
        count_occurrences(&created_raw, token_bytes),
        1,
        "token must appear once on response wire"
    );

    let listed_raw = admin
        .request_raw(&DbRequest::ListApiTokens {
            for_user: None,
            hmac: None,
        })
        .await;
    let secret = token_text
        .rsplit('.')
        .next()
        .expect("secret segment")
        .as_bytes();
    assert_eq!(
        count_occurrences(&listed_raw, token_bytes),
        0,
        "list wire leaked full token"
    );
    assert_eq!(
        count_occurrences(&listed_raw, secret),
        0,
        "list wire leaked token secret"
    );
    let listed = decoded(&listed_raw);
    assert!(matches!(
        listed,
        DbResponse::ApiTokensListed { tokens }
            if tokens.len() == 1 && tokens[0].id == token.id
    ));
    drop(admin);
    shutdown(harness).await;
}
