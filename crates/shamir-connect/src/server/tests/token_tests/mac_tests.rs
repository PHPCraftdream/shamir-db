use crate::server::token::{compute_mac, mac_eq, parse_token, TokenId};
use hmac::Mac;

#[test]
fn independent_documented_layout_and_component_mutations() {
    let key = [8u8; 32];
    let canonical = b"shk1.AAAAAAAAAAAAAAAAAAAAAA.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let parsed = parse_token(canonical).unwrap();
    let actual = compute_mac(&key, 7, 1, &parsed.id, &parsed.secret);
    let mut independent = hmac::Hmac::<sha2::Sha256>::new_from_slice(&key).unwrap();
    independent.update(b"shamir-api-token");
    independent.update(&[1, 7]);
    independent.update(&parsed.id.0);
    independent.update(parsed.secret.as_bytes());
    let expected: [u8; 32] = independent.finalize().into_bytes().into();
    assert_eq!(actual, expected);
    let mut changed_key = key;
    changed_key[0] ^= 1;
    assert_ne!(
        actual,
        compute_mac(&changed_key, 7, 1, &parsed.id, &parsed.secret)
    );
    assert_ne!(actual, compute_mac(&key, 8, 1, &parsed.id, &parsed.secret));
    assert_ne!(actual, compute_mac(&key, 7, 2, &parsed.id, &parsed.secret));
    assert_ne!(
        actual,
        compute_mac(&key, 7, 1, &TokenId([5; 16]), &parsed.secret)
    );
    let mut changed_wire = canonical.to_vec();
    changed_wire[28] = b'B';
    let changed_secret = parse_token(&changed_wire).unwrap().secret;
    assert_ne!(actual, compute_mac(&key, 7, 1, &parsed.id, &changed_secret));
}

#[test]
fn constant_time_compare_rejects_one_bit_difference_in_every_position() {
    let a = [0u8; 32];
    assert!(mac_eq(&a, &a));
    for position in 0..32 {
        let mut b = a;
        b[position] = 1;
        assert!(!mac_eq(&a, &b), "byte position {position}");
    }
}
