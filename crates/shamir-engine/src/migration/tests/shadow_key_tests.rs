use crate::migration::shadow_key::ShadowKey;

#[test]
fn round_trip() {
    let k = ShadowKey::new("mig-001", 42);
    let bytes = k.to_bytes();
    assert_eq!(ShadowKey::parse_lsn(&bytes), Some(42));
}

#[test]
fn binary_layout_matches_legacy() {
    let bytes = ShadowKey::new("mig-001", 1).to_bytes();
    let mut expected = Vec::new();
    expected.extend_from_slice(b"__shadow_");
    expected.extend_from_slice(b"mig-001");
    expected.push(b'_');
    expected.extend_from_slice(&1u64.to_be_bytes());
    assert_eq!(bytes.as_ref(), expected.as_slice());
}

#[test]
fn scan_prefix_matches_legacy() {
    let prefix = ShadowKey::scan_prefix("mig-001");
    let mut expected = Vec::new();
    expected.extend_from_slice(b"__shadow_");
    expected.extend_from_slice(b"mig-001");
    expected.push(b'_');
    assert_eq!(prefix.as_ref(), expected.as_slice());
}

#[test]
fn parse_lsn_extracts_be_suffix() {
    let k = ShadowKey::new("x", 0xdead_beef_cafe_babe).to_bytes();
    assert_eq!(ShadowKey::parse_lsn(&k), Some(0xdead_beef_cafe_babe));
}
