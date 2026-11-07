use crate::migration::shadow_key::ShadowKey;

#[test]
fn round_trip() {
    let k = ShadowKey::new("mig-001", 42);
    let bytes = k.to_bytes();
    assert_eq!(ShadowKey::parse_lsn(&bytes), Some(42));
}

#[test]
fn binary_layout_length_prefixes_id() {
    let bytes = ShadowKey::new("mig-001", 1).to_bytes();
    let mut expected = Vec::new();
    expected.extend_from_slice(b"__shadow_");
    expected.extend_from_slice(&(b"mig-001".len() as u32).to_be_bytes());
    expected.extend_from_slice(b"mig-001");
    expected.extend_from_slice(&1u64.to_be_bytes());
    assert_eq!(bytes.as_ref(), expected.as_slice());
}

#[test]
fn scan_prefix_length_prefixes_id() {
    let prefix = ShadowKey::scan_prefix("mig-001");
    let mut expected = Vec::new();
    expected.extend_from_slice(b"__shadow_");
    expected.extend_from_slice(&(b"mig-001".len() as u32).to_be_bytes());
    expected.extend_from_slice(b"mig-001");
    assert_eq!(prefix.as_ref(), expected.as_slice());
}

#[test]
fn parse_lsn_extracts_be_suffix() {
    let k = ShadowKey::new("x", 0xdead_beef_cafe_babe).to_bytes();
    assert_eq!(ShadowKey::parse_lsn(&k), Some(0xdead_beef_cafe_babe));
}

/// Defect 5 (2026-08-14 cross-crate rush review, shamir-engine group 24):
/// production migration ids are `mig_<table_name>_<ts>_<hex>`
/// (`shamir-db`'s `admin_migration.rs`), and `<table_name>` is a
/// user-controlled identifier that may itself contain `_`. Under the
/// prior delimiter layout (`id || b'_' || lsn`), `scan_prefix("mig_users")`
/// is a literal byte-prefix of every key belonging to a DIFFERENT
/// migration `"mig_users_backup"` (`"__shadow_mig_users_"` prefixes
/// `"__shadow_mig_users_backup_..."`), so one migration's shadow log
/// could bleed into another's at the prefix-scan boundary. The
/// length-prefixed layout must keep these fully disjoint.
#[test]
fn scan_prefix_is_not_a_byte_prefix_of_a_different_ids_keys() {
    let prefix_a = ShadowKey::scan_prefix("mig_users");
    let key_b = ShadowKey::new("mig_users_backup", 1).to_bytes();
    assert!(!key_b.as_ref().starts_with(prefix_a.as_ref()));

    // ... nor the reverse direction.
    let prefix_b = ShadowKey::scan_prefix("mig_users_backup");
    let key_a = ShadowKey::new("mig_users", 1).to_bytes();
    assert!(!key_a.as_ref().starts_with(prefix_b.as_ref()));
}
