use crate::server::token::{Endpoint, TokenChannels, TokenMode, TokenRecord};

fn sample() -> TokenRecord {
    TokenRecord {
        id: [1; 16],
        owner_user_id: [2; 16],
        label: "svc".into(),
        mac: [3; 32],
        key_id: 1,
        format_version: 1,
        created_at_ns: 1,
        expires_at_ns: 2,
        mode: TokenMode::ReadOnly,
        channels: TokenChannels::Both,
        databases: Some(vec!["db".into()]),
        roles: vec!["reader".into()],
        revoked_at_ns: None,
    }
}

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle)
}

#[test]
fn named_msgpack_round_trip_and_unknown_field_rejection() {
    let record = sample();
    let bytes = rmp_serde::to_vec_named(&record).unwrap();
    let decoded: TokenRecord = rmp_serde::from_slice(&bytes).unwrap();
    assert_eq!(decoded, record);

    // Named records are fixmaps. Add one full key/value pair and increment
    // the map count, rather than relying on trailing bytes being rejected.
    let mut with_unknown = bytes;
    assert_eq!(with_unknown[0] & 0xf0, 0x80);
    with_unknown[0] += 1;
    with_unknown.extend_from_slice(&[0xa8, b's', b'u', b'r', b'p', b'r', b'i', b's', b'e', 1]);
    assert!(rmp_serde::from_slice::<TokenRecord>(&with_unknown).is_err());
}

#[test]
fn byte_fields_use_msgpack_binary_types_and_expected_lengths() {
    let bytes = rmp_serde::to_vec_named(&sample()).unwrap();
    for (value, length) in [
        ([1_u8; 16].as_slice(), 16_u8),
        ([2_u8; 16].as_slice(), 16),
        ([3_u8; 32].as_slice(), 32),
    ] {
        let mut encoded = vec![0xc4, length]; // bin8 + byte length
        encoded.extend_from_slice(value);
        assert!(
            contains(&bytes, &encoded),
            "missing bin field of length {length}"
        );
    }
}

#[test]
fn enum_variants_use_the_normative_wire_strings() {
    for (mode, wire) in [
        (TokenMode::ReadOnly, "read_only"),
        (TokenMode::ReadWrite, "read_write"),
    ] {
        let mut record = sample();
        record.mode = mode;
        let bytes = rmp_serde::to_vec_named(&record).unwrap();
        let mut encoded = vec![0xa0 | wire.len() as u8];
        encoded.extend_from_slice(wire.as_bytes());
        assert!(contains(&bytes, &encoded));
        assert_eq!(
            rmp_serde::from_slice::<TokenRecord>(&bytes).unwrap(),
            record
        );
    }
    for (channels, wire) in [
        (TokenChannels::Rest, "rest"),
        (TokenChannels::Ws, "ws"),
        (TokenChannels::Both, "both"),
    ] {
        let mut record = sample();
        record.channels = channels;
        let bytes = rmp_serde::to_vec_named(&record).unwrap();
        let mut encoded = vec![0xa0 | wire.len() as u8];
        encoded.extend_from_slice(wire.as_bytes());
        assert!(contains(&bytes, &encoded));
        assert_eq!(
            rmp_serde::from_slice::<TokenRecord>(&bytes).unwrap(),
            record
        );
    }
}

#[test]
fn optional_database_and_revocation_values_round_trip() {
    for databases in [
        None,
        Some(Vec::new()),
        Some(vec!["alpha".into(), "β".into()]),
    ] {
        for revoked_at_ns in [None, Some(0), Some(42)] {
            let mut record = sample();
            record.databases = databases.clone();
            record.revoked_at_ns = revoked_at_ns;
            let bytes = rmp_serde::to_vec_named(&record).unwrap();
            assert_eq!(
                rmp_serde::from_slice::<TokenRecord>(&bytes).unwrap(),
                record
            );
        }
    }
}

#[test]
fn shape_boundaries_are_structural_and_use_utf8_byte_lengths() {
    let mut r = sample();
    assert!(r.validate_shape().is_ok());
    r.expires_at_ns = r.created_at_ns - 1;
    assert!(r.validate_shape().is_err());
    r.expires_at_ns = r.created_at_ns;
    assert!(r.validate_shape().is_err());
    r.expires_at_ns = r.created_at_ns + 1;
    assert!(r.validate_shape().is_ok());

    r.label = "".into();
    assert!(r.validate_shape().is_err());
    r.label = "x".into();
    assert!(r.validate_shape().is_ok()); // one-byte label
    r.label = "x".repeat(64);
    assert!(r.validate_shape().is_ok());
    r.label.push('x');
    assert!(r.validate_shape().is_err());
    r.label = "é".repeat(32); // 64 UTF-8 bytes
    assert!(r.validate_shape().is_ok());
    r.label.push('é'); // 66 UTF-8 bytes
    assert!(r.validate_shape().is_err());

    r = sample();
    r.databases = None;
    assert!(r.validate_shape().is_ok()); // None
    r.databases = Some(vec!["".into()]);
    assert!(r.validate_shape().is_err());
    r.databases = Some(vec!["d".into()]);
    assert!(r.validate_shape().is_ok()); // one-byte database name
    r.databases = Some(Vec::new());
    assert!(r.validate_shape().is_ok()); // empty list
    r.databases = Some(vec!["d".into(); 64]);
    assert!(r.validate_shape().is_ok());
    r.databases.as_mut().unwrap().push("d".into());
    assert!(r.validate_shape().is_err());
    r.databases = Some(vec![String::new()]);
    assert!(r.validate_shape().is_err());
    r.databases = Some(vec!["d".repeat(128)]);
    assert!(r.validate_shape().is_ok());
    r.databases = Some(vec!["d".repeat(129)]);
    assert!(r.validate_shape().is_err());
    r.databases = Some(vec!["é".repeat(64)]); // 128 UTF-8 bytes
    assert!(r.validate_shape().is_ok());
    r.databases = Some(vec!["é".repeat(65)]); // 130 UTF-8 bytes
    assert!(r.validate_shape().is_err());

    r = sample();
    r.roles.clear();
    assert!(r.validate_shape().is_ok());
    r.roles = vec!["role".into(); 64];
    assert!(r.validate_shape().is_ok());
    r.roles.push("role".into());
    assert!(r.validate_shape().is_err());
}

#[test]
fn debug_redacts_binary_identifiers_and_mac() {
    let record = sample();
    let debug = format!("{record:?}");
    assert!(!debug.contains("[1, 1, 1"));
    assert!(!debug.contains("[2, 2, 2"));
    assert!(!debug.contains("[3, 3, 3"));
    assert!(debug.contains("<REDACTED>"));
}

#[test]
fn channels_allows_truth_table() {
    for (channel, rest, ws) in [
        (TokenChannels::Rest, true, false),
        (TokenChannels::Ws, false, true),
        (TokenChannels::Both, true, true),
    ] {
        assert_eq!(channel.allows(Endpoint::Rest), rest);
        assert_eq!(channel.allows(Endpoint::Ws), ws);
    }
}
