use crate::hmac::{
    canonical_commit_migration, canonical_drop_db, canonical_drop_index, canonical_drop_repo,
    canonical_drop_role, canonical_drop_table, canonical_drop_user, canonical_rollback_migration,
    canonical_start_migration, compute_tag_hex, derive_session_hmac_key, hex_decode, hex_encode,
    verify_tag_hex,
};

#[test]
fn key_is_deterministic_and_domain_separated() {
    let sid = [7u8; 32];
    let k1 = derive_session_hmac_key(&sid);
    let k2 = derive_session_hmac_key(&sid);
    assert_eq!(k1, k2);
    // Different session → different key.
    let mut other = sid;
    other[0] ^= 0xFF;
    assert_ne!(derive_session_hmac_key(&other), k1);
    // Domain-separated: raw session_id != derived key.
    assert_ne!(k1, sid);
}

#[test]
fn canonical_inputs_are_null_separated() {
    assert_eq!(canonical_drop_db("mydb"), b"drop_db\0mydb");
    assert_eq!(
        canonical_drop_repo("mydb", "cold"),
        b"drop_repo\0mydb\0cold"
    );
    assert_eq!(
        canonical_drop_table("mydb", "main", "users"),
        b"drop_table\0mydb\0main\0users"
    );
    assert_eq!(
        canonical_drop_index("mydb", "main", "users", "by_email", false),
        b"drop_index\0mydb\0main\0users\0by_email\x000"
    );
    assert_eq!(
        canonical_drop_index("mydb", "main", "users", "by_email", true),
        b"drop_index\0mydb\0main\0users\0by_email\x001"
    );
    assert_eq!(canonical_drop_user("bob"), b"drop_user\0bob");
    assert_eq!(canonical_drop_role("admin"), b"drop_role\0admin");

    assert_eq!(
        canonical_start_migration("mydb", "main", "users", "cold", "redb"),
        b"start_migration\0mydb\0main\0users\0cold\0redb"
    );
    assert_eq!(
        canonical_commit_migration("mydb", "mig-001"),
        b"commit_migration\0mydb\0mig-001"
    );
    assert_eq!(
        canonical_rollback_migration("mydb", "mig-001"),
        b"rollback_migration\0mydb\0mig-001"
    );
}

#[test]
fn compute_then_verify_roundtrip() {
    let key = derive_session_hmac_key(&[1u8; 32]);
    let canonical = canonical_drop_table("db", "main", "users");
    let tag = compute_tag_hex(&key, &canonical);
    assert!(verify_tag_hex(&key, &canonical, &tag));
}

#[test]
fn verify_rejects_wrong_input() {
    let key = derive_session_hmac_key(&[1u8; 32]);
    let canonical = canonical_drop_table("db", "main", "users");
    let tag = compute_tag_hex(&key, &canonical);
    // Same key but different op-bytes — must NOT verify.
    let wrong = canonical_drop_table("db", "main", "OTHER");
    assert!(!verify_tag_hex(&key, &wrong, &tag));
}

#[test]
fn verify_rejects_wrong_key() {
    let key_a = derive_session_hmac_key(&[1u8; 32]);
    let key_b = derive_session_hmac_key(&[2u8; 32]);
    let canonical = canonical_drop_table("db", "main", "users");
    let tag = compute_tag_hex(&key_a, &canonical);
    assert!(!verify_tag_hex(&key_b, &canonical, &tag));
}

#[test]
fn verify_rejects_malformed_hex() {
    let key = derive_session_hmac_key(&[1u8; 32]);
    let canonical = canonical_drop_db("x");
    assert!(!verify_tag_hex(&key, &canonical, ""));
    assert!(!verify_tag_hex(&key, &canonical, "not-hex-at-all"));
    assert!(!verify_tag_hex(&key, &canonical, "deadbee")); // odd length
}

#[test]
fn hex_codec_roundtrip() {
    let bytes = [0u8, 1, 15, 16, 255, 0xab];
    let enc = hex_encode(&bytes);
    assert_eq!(enc, "00010f10ffab");
    assert_eq!(hex_decode(&enc).unwrap(), bytes);
    assert_eq!(hex_decode("DEADBEEF").unwrap(), [0xde, 0xad, 0xbe, 0xef]);
}
