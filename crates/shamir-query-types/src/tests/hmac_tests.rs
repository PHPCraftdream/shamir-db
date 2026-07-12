use crate::admin::{PurgeScope, ResourceRef, Retention};
use crate::hmac::{
    canonical_chgrp, canonical_chmod, canonical_chown, canonical_commit_migration,
    canonical_create_role, canonical_create_user, canonical_drop_db, canonical_drop_index,
    canonical_drop_repo, canonical_drop_role, canonical_drop_table, canonical_drop_user,
    canonical_grant_role, canonical_purge_history, canonical_purge_scope, canonical_resource_ref,
    canonical_retention, canonical_revoke_role, canonical_rollback_migration,
    canonical_set_retention, canonical_start_migration, compute_tag_hex, derive_session_hmac_key,
    hex_decode, hex_encode, verify_tag_hex,
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

// ---------------------------------------------------------------------------
// New canonical_* helpers (task #542 — extended destructive-op coverage)
// ---------------------------------------------------------------------------

#[test]
fn canonical_grant_revoke_role_are_null_separated() {
    assert_eq!(
        canonical_grant_role("superuser", "alice"),
        b"grant_role\0superuser\0alice"
    );
    assert_eq!(
        canonical_revoke_role("superuser", "alice"),
        b"revoke_role\0superuser\0alice"
    );
}

#[test]
fn canonical_create_user_never_includes_password() {
    // Canonical input is username-only — no password field exists to leak.
    assert_eq!(canonical_create_user("bob"), b"create_user\0bob");
}

#[test]
fn canonical_create_role_is_name_only() {
    assert_eq!(canonical_create_role("viewer"), b"create_role\0viewer");
}

#[test]
fn canonical_resource_ref_matches_resource_path_display_shape() {
    assert_eq!(
        canonical_resource_ref(&ResourceRef::Database {
            database: "mydb".to_string()
        }),
        "db://mydb"
    );
    assert_eq!(
        canonical_resource_ref(&ResourceRef::Store {
            store: ["mydb".to_string(), "main".to_string()]
        }),
        "db://mydb/main"
    );
    assert_eq!(
        canonical_resource_ref(&ResourceRef::Table {
            table: ["mydb".to_string(), "main".to_string(), "users".to_string()]
        }),
        "db://mydb/main/users"
    );
    assert_eq!(
        canonical_resource_ref(&ResourceRef::Function {
            function: "my_fn".to_string()
        }),
        "fn://my_fn"
    );
    assert_eq!(
        canonical_resource_ref(&ResourceRef::FunctionFolder {
            function_folder: vec!["reports".to_string(), "daily".to_string()]
        }),
        "fn://reports/daily/"
    );
    assert_eq!(
        canonical_resource_ref(&ResourceRef::FunctionNamespace {
            function_namespace: true
        }),
        "fn://"
    );
}

#[test]
fn canonical_chmod_chown_chgrp_are_null_separated() {
    let table = ResourceRef::Table {
        table: ["db".to_string(), "main".to_string(), "users".to_string()],
    };
    assert_eq!(
        canonical_chmod(&table, 0o700),
        b"chmod\0db://db/main/users\0448"
    );
    assert_eq!(canonical_chown(&table, 7), b"chown\0db://db/main/users\07");
    assert_eq!(
        canonical_chgrp(&table, Some(3)),
        b"chgrp\0db://db/main/users\03"
    );
    // Clearing the group uses the "null" sentinel — never collides with a
    // valid decimal u64 group id.
    assert_eq!(
        canonical_chgrp(&table, None),
        b"chgrp\0db://db/main/users\0null"
    );
}

#[test]
fn canonical_retention_renders_all_three_knobs_or_none_sentinel() {
    assert_eq!(canonical_retention(&Retention::default()), "none,none,none");
    assert_eq!(
        canonical_retention(&Retention {
            max_age_secs: Some(86_400),
            max_count: Some(5),
            min_count: Some(1),
        }),
        "86400,5,1"
    );
    assert_eq!(
        canonical_retention(&Retention {
            max_age_secs: None,
            max_count: Some(0),
            min_count: None,
        }),
        "none,0,none"
    );
}

#[test]
fn canonical_set_retention_is_null_separated() {
    let retention = Retention {
        max_count: Some(5),
        ..Default::default()
    };
    assert_eq!(
        canonical_set_retention("mydb", "main", "users", &retention),
        b"set_retention\0mydb\0main\0users\0none,5,none"
    );
}

#[test]
fn canonical_purge_scope_renders_both_variants() {
    assert_eq!(
        canonical_purge_scope(&PurgeScope::OlderThan {
            timestamp: 1_600_000_000_000
        }),
        "older_than:1600000000000"
    );
    assert_eq!(
        canonical_purge_scope(&PurgeScope::OlderThanAge { age_secs: 86_400 }),
        "older_than_age:86400"
    );
}

#[test]
fn canonical_purge_history_is_null_separated() {
    assert_eq!(
        canonical_purge_history(
            "mydb",
            "main",
            "users",
            &PurgeScope::OlderThanAge { age_secs: 86_400 }
        ),
        b"purge_history\0mydb\0main\0users\0older_than_age:86400"
    );
}

#[test]
fn new_canonical_inputs_roundtrip_through_hmac() {
    let key = derive_session_hmac_key(&[1u8; 32]);
    let canonical = canonical_grant_role("superuser", "alice");
    let tag = compute_tag_hex(&key, &canonical);
    assert!(verify_tag_hex(&key, &canonical, &tag));
    // A tag computed for "alice" must not verify for "mallory".
    let wrong = canonical_grant_role("superuser", "mallory");
    assert!(!verify_tag_hex(&key, &wrong, &tag));
}
