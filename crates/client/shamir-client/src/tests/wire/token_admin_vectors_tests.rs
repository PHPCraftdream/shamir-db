use shamir_connect::common::crypto::derive_session_hmac_key;
use shamir_query_builder::token_admin::{
    create_api_token, list_api_tokens, revoke_api_token, revoke_user_tokens,
};
use shamir_query_types::hmac::{canonical_db_request, verify_tag_hex};
use shamir_query_types::wire::token_admin::{ApiTokenChannels, ApiTokenMode};
use shamir_query_types::wire::DbRequest;

use crate::client::admin_request_tag;

const FIXTURE: &str =
    include_str!("../../../../../query/shamir-query-types/test-vectors/admin_hmac_v2.json");
const VECTOR_TAGS: [&str; 4] = ["0020", "0021", "0022", "0023"];

fn fixture_value(tag: &str, field: &str) -> String {
    let marker = format!("\"op_tag\": \"{tag}\"");
    let start = FIXTURE.find(&marker).expect("fixture vector") + marker.len();
    let end = FIXTURE[start..]
        .find("\"op_tag\":")
        .map_or(FIXTURE.len(), |next| start + next);
    let vector = &FIXTURE[start..end];
    let key = format!("\"{field}\": \"");
    let value_start = vector.find(&key).expect("fixture field") + key.len();
    let value_end = vector[value_start..]
        .find('\"')
        .expect("fixture string end")
        + value_start;
    vector[value_start..value_end].to_owned()
}

fn request(tag: &str, signed: bool) -> DbRequest {
    match (tag, signed) {
        ("0020", false) => create_api_token("café-token")
            .ttl_secs(9_007_199_254_740_993)
            .mode(ApiTokenMode::ReadWrite)
            .channels(ApiTokenChannels::Both)
            .databases(["café-db", "db"])
            .for_user("bob")
            .build(),
        ("0020", true) => create_api_token("café-token")
            .ttl_secs(9_007_199_254_740_993)
            .mode(ApiTokenMode::ReadWrite)
            .channels(ApiTokenChannels::Both)
            .databases(["café-db", "db"])
            .for_user("bob")
            .hmac("ff")
            .build(),
        ("0021", false) => list_api_tokens().for_user("bob").build(),
        ("0021", true) => list_api_tokens().for_user("bob").hmac("ff").build(),
        ("0022", false) => revoke_api_token("AAECAwQFBgcICQoLDA0ODw").build(),
        ("0022", true) => revoke_api_token("AAECAwQFBgcICQoLDA0ODw")
            .hmac("ff")
            .build(),
        ("0023", false) => revoke_user_tokens().build(),
        ("0023", true) => revoke_user_tokens().hmac("ff").build(),
        _ => unreachable!(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[test]
fn builder_requests_match_shared_canonical_and_tag_vectors() {
    let session_id: [u8; 32] = std::array::from_fn(|index| index as u8);
    let key = derive_session_hmac_key(&session_id);

    for tag in VECTOR_TAGS {
        let request = request(tag, false);
        let canonical = canonical_db_request(&request).expect("canonical request");
        let canonical_hex = hex(&canonical);
        let expected_canonical = fixture_value(tag, "canonical_hex");
        assert_eq!(canonical_hex, expected_canonical, "canonical vector {tag}");

        let signature = admin_request_tag(&session_id, &request).expect("request tag");
        let expected_tag = fixture_value(tag, "tag_hex");
        assert_eq!(signature, expected_tag, "tag vector {tag}");
        assert!(verify_tag_hex(&key, &canonical, &signature));
    }
}

#[test]
fn hmac_field_is_excluded_from_all_four_canonical_vectors() {
    for tag in VECTOR_TAGS {
        let unsigned = request(tag, false);
        let with_hmac = request(tag, true);
        let session_id: [u8; 32] = std::array::from_fn(|index| index as u8);
        assert_eq!(
            admin_request_tag(&session_id, &unsigned).expect("unsigned tag"),
            admin_request_tag(&session_id, &with_hmac).expect("signed tag"),
            "HMAC change must not alter canonical tagging for vector {tag}"
        );
        assert_eq!(
            canonical_db_request(&unsigned).expect("unsigned canonical"),
            canonical_db_request(&with_hmac).expect("signed canonical"),
            "HMAC change must not alter canonical bytes for vector {tag}"
        );
    }
}

#[test]
fn tag_verification_rejects_wrong_key_modified_canonical_and_bad_tags() {
    let session_id: [u8; 32] = std::array::from_fn(|index| index as u8);
    let key = derive_session_hmac_key(&session_id);
    let request = request("0020", false);
    let canonical = canonical_db_request(&request).expect("canonical request");
    let tag = fixture_value("0020", "tag_hex");

    let wrong_key = derive_session_hmac_key(&[0xff; 32]);
    assert!(!verify_tag_hex(&wrong_key, &canonical, &tag));

    let mut modified_canonical = canonical.clone();
    modified_canonical[0] ^= 1;
    assert!(!verify_tag_hex(&key, &modified_canonical, &tag));

    let mut modified_tag = tag.clone().into_bytes();
    modified_tag[0] = if modified_tag[0] == b'0' { b'1' } else { b'0' };
    assert!(!verify_tag_hex(
        &key,
        &canonical,
        std::str::from_utf8(&modified_tag).unwrap()
    ));
    assert!(!verify_tag_hex(&key, &canonical, &tag[..tag.len() - 1]));
    assert!(!verify_tag_hex(&key, &canonical, ""));
}

#[test]
fn fixture_extractor_bounds_nested_vector_fields_by_next_op_tag() {
    for (tag, expected) in [
        ("0020", "create_api_token"),
        ("0021", "list_api_tokens"),
        ("0022", "revoke_api_token"),
        ("0023", "revoke_user_tokens"),
    ] {
        assert!(FIXTURE.contains(&format!("\"op_tag\": \"{tag}\"")));
        let op = format!("\"op\": \"{expected}\"");
        let marker = format!("\"op_tag\": \"{tag}\"");
        let start = FIXTURE.find(&marker).expect("fixture vector") + marker.len();
        let end = FIXTURE[start..]
            .find("\"op_tag\":")
            .map_or(FIXTURE.len(), |next| start + next);
        assert!(FIXTURE[start..end].contains(&op));
        assert!(!fixture_value(tag, "canonical_hex").is_empty());
        assert!(!fixture_value(tag, "tag_hex").is_empty());
    }
}
