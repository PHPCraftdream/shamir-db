use crate::server::token::{parse_token, verify, *};
struct Floors(Option<u64>);
impl OwnerFloors for Floors {
    fn floor_if_present(&self, _: &[u8; 16]) -> Option<u64> {
        self.0
    }
}
fn fixture() -> (TokenMirror, Vec<u8>, [u8; 32]) {
    fixture_with_format_version(1)
}
fn fixture_with_format_version(stored_format_version: u8) -> (TokenMirror, Vec<u8>, [u8; 32]) {
    let canonical = b"shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let parsed = parse_token(canonical).unwrap();
    let id = parsed.id;
    let key = [9; 32];
    let mac = compute_mac(&key, 1, 1, &id, &parsed.secret);
    let record = TokenRecord {
        id: id.0,
        owner_user_id: [2; 16],
        label: "x".into(),
        mac,
        key_id: 1,
        format_version: stored_format_version,
        created_at_ns: 10,
        expires_at_ns: 30,
        mode: TokenMode::ReadOnly,
        channels: TokenChannels::Rest,
        databases: None,
        roles: vec![],
        revoked_at_ns: None,
    };
    let mirror = TokenMirror::new();
    mirror.insert(record);
    (mirror, canonical.to_vec(), key)
}
fn ctx<'a>(
    mirror: &'a TokenMirror,
    key: &'a [u8; 32],
    owners: &'a Floors,
    now_ns: u64,
    endpoint: Endpoint,
    key_id: u8,
) -> VerifyCtx<'a> {
    VerifyCtx {
        mirror,
        key,
        key_id,
        owners,
        now_ns,
        endpoint,
    }
}
#[test]
fn exactly_one_mac_for_unknown_id_and_wrong_secret() {
    let (m, wire, key) = fixture();
    let floors = Floors(Some(10));
    verify::reset_mac_count();
    let mut unknown = wire.clone();
    unknown[5] = if unknown[5] == b'A' { b'B' } else { b'A' };
    assert_eq!(
        verify_token(&ctx(&m, &key, &floors, 20, Endpoint::Rest, 1), &unknown).unwrap_err(),
        VerifyFailure::Invalid
    );
    assert_eq!(verify::mac_count(), 1);
    verify::reset_mac_count();
    let mut wrong = wire.clone();
    wrong[5 + 22 + 1] = if wrong[5 + 22 + 1] == b'B' {
        b'A'
    } else {
        b'B'
    };
    assert_eq!(
        verify_token(&ctx(&m, &key, &floors, 20, Endpoint::Rest, 1), &wrong).unwrap_err(),
        VerifyFailure::Invalid
    );
    assert_eq!(verify::mac_count(), 1);
}
#[test]
fn full_ordered_decision_table_and_controls() {
    let (m, wire, key) = fixture();
    let good = Floors(Some(9));
    assert!(verify_token(&ctx(&m, &key, &good, 29, Endpoint::Rest, 1), &wire).is_ok());
    assert_eq!(
        verify_token(&ctx(&m, &key, &good, 30, Endpoint::Ws, 1), &wire).unwrap_err(),
        VerifyFailure::Expired
    );
    assert_eq!(
        verify_token(&ctx(&m, &key, &Floors(None), 30, Endpoint::Rest, 1), &wire).unwrap_err(),
        VerifyFailure::Expired
    );
    assert_eq!(
        verify_token(&ctx(&m, &key, &Floors(Some(9)), 30, Endpoint::Ws, 1), &wire).unwrap_err(),
        VerifyFailure::Expired
    );
    assert_eq!(
        verify_token(&ctx(&m, &key, &Floors(None), 20, Endpoint::Ws, 1), &wire).unwrap_err(),
        VerifyFailure::OwnerInvalid
    );
    assert_eq!(
        verify_token(&ctx(&m, &key, &Floors(None), 20, Endpoint::Rest, 1), &wire).unwrap_err(),
        VerifyFailure::OwnerInvalid
    );
    assert_eq!(
        verify_token(
            &ctx(&m, &key, &Floors(Some(10)), 20, Endpoint::Rest, 1),
            &wire
        )
        .unwrap_err(),
        VerifyFailure::OwnerInvalid
    );
    assert!(verify_token(
        &ctx(&m, &key, &Floors(Some(9)), 20, Endpoint::Rest, 1),
        &wire
    )
    .is_ok());
    let mut wrong_key = key;
    wrong_key[0] ^= 1;
    assert_eq!(
        verify_token(&ctx(&m, &wrong_key, &good, 20, Endpoint::Rest, 1), &wire).unwrap_err(),
        VerifyFailure::Invalid
    );
    let id = m.ids_of_owner(&[2; 16])[0];
    let mut bad_record = m.get(&id).unwrap().record().clone();
    bad_record.mac[0] ^= 1;
    let bad_mirror = TokenMirror::new();
    assert!(bad_mirror.insert(bad_record));
    assert_eq!(
        verify_token(&ctx(&bad_mirror, &key, &good, 20, Endpoint::Rest, 1), &wire).unwrap_err(),
        VerifyFailure::Invalid
    );
    assert_eq!(
        verify_token(&ctx(&m, &key, &good, 20, Endpoint::Ws, 1), &wire).unwrap_err(),
        VerifyFailure::ChannelDenied
    );
    let mut wrong = wire.clone();
    wrong[5 + 22 + 1] = if wrong[5 + 22 + 1] == b'B' {
        b'A'
    } else {
        b'B'
    };
    assert_eq!(
        verify_token(&ctx(&m, &key, &good, 20, Endpoint::Ws, 1), &wrong).unwrap_err(),
        VerifyFailure::Invalid
    );
    assert_eq!(
        verify_token(&ctx(&m, &key, &good, 20, Endpoint::Rest, 2), &wire).unwrap_err(),
        VerifyFailure::Invalid
    );
    assert_eq!(
        verify_token(&ctx(&m, &key, &good, 20, Endpoint::Rest, 1), b"malformed").unwrap_err(),
        VerifyFailure::Invalid
    );
    let id = m.ids_of_owner(&[2; 16])[0];
    assert!(m.revoke(&id, 15));
    assert_eq!(
        verify_token(&ctx(&m, &key, &good, 30, Endpoint::Rest, 1), &wire).unwrap_err(),
        VerifyFailure::Revoked
    );
}
#[test]
fn bad_mac_precedes_revoked_expired_unknown_owner_and_channel() {
    let (m, wire, key) = fixture();
    let id = m.ids_of_owner(&[2; 16])[0];
    m.revoke(&id, 12);
    let bad_owner = Floors(None);
    let mut wrong = wire.clone();
    wrong[5 + 22 + 1] = if wrong[5 + 22 + 1] == b'B' {
        b'A'
    } else {
        b'B'
    };
    assert_eq!(
        verify_token(&ctx(&m, &key, &bad_owner, 99, Endpoint::Ws, 1), &wrong).unwrap_err(),
        VerifyFailure::Invalid
    );
}
#[test]
fn key_id_mismatch_is_invalid_before_hmac_and_revocation_checks() {
    let (m, wire, key) = fixture();
    let id = m.ids_of_owner(&[2; 16])[0];
    m.revoke(&id, 15);
    verify::reset_mac_count();
    assert_eq!(
        verify_token(&ctx(&m, &key, &Floors(None), 99, Endpoint::Ws, 2), &wire).unwrap_err(),
        VerifyFailure::Invalid
    );
    assert_eq!(verify::mac_count(), 0);
}
#[test]
fn wrong_stored_format_version_is_invalid_without_hmac() {
    let (m, wire, key) = fixture_with_format_version(2);
    verify::reset_mac_count();
    assert_eq!(
        verify_token(&ctx(&m, &key, &Floors(None), 1, Endpoint::Rest, 1), &wire).unwrap_err(),
        VerifyFailure::Invalid
    );
    assert_eq!(verify::mac_count(), 0);
}
#[test]
fn revoked_precedes_expiry_owner_and_channel() {
    let (m, wire, key) = fixture();
    let id = m.ids_of_owner(&[2; 16])[0];
    m.revoke(&id, 12);
    assert_eq!(
        verify_token(&ctx(&m, &key, &Floors(None), 99, Endpoint::Ws, 1), &wire).unwrap_err(),
        VerifyFailure::Revoked
    );
}
