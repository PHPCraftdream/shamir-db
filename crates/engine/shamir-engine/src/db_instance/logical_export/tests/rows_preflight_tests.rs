use super::super::rows::preflight;
use shamir_storage::error::DbError;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::value::InnerValue;

const MAX_ROW_BYTES: usize = 16 * 1024 * 1024;
const MAX_CONTAINER_ENTRIES: usize = 65_536;
const MAX_WORKING_BYTES: usize = 64 * 1024 * 1024;
const NODE_WORKING_BYTES: usize = 192;

fn encode_map_header(entries: usize) -> Vec<u8> {
    let mut bytes = vec![0xdf];
    bytes.extend_from_slice(
        &u32::try_from(entries)
            .expect("test map count fits MessagePack map32")
            .to_be_bytes(),
    );
    bytes
}

fn encode_interned_key(id: u64, bytes: &mut Vec<u8>) {
    let (encoded, len) = InternerKey::new(id).as_bytes_buf();
    bytes.extend_from_slice(&[
        0xc4,
        u8::try_from(len).expect("interned key length is one of 1, 2, 4, or 8"),
    ]);
    bytes.extend_from_slice(&encoded[..len]);
}

fn map_with_unique_keys(entries: usize) -> Vec<u8> {
    let mut bytes = encode_map_header(entries);
    for id in 1..=entries {
        encode_interned_key(u64::try_from(id).expect("test key ID fits u64"), &mut bytes);
        bytes.push(0xc0);
    }
    bytes
}

#[test]
fn map_member_limit_accepts_exactly_65_536_entries() {
    let bytes = map_with_unique_keys(MAX_CONTAINER_ENTRIES);
    assert!(bytes.len() < MAX_ROW_BYTES);
    let working_bytes = preflight(&bytes).expect("legal maximum-sized map fits the budget");
    assert!(bytes.len() + working_bytes <= MAX_WORKING_BYTES);
}

#[test]
fn map_member_limit_rejects_65_537_entries() {
    let bytes = map_with_unique_keys(MAX_CONTAINER_ENTRIES + 1);
    assert!(bytes.len() < MAX_ROW_BYTES);
    assert!(matches!(
        preflight(&bytes),
        Err(DbError::Validation(message)) if message == "record container exceeds entry limit"
    ));
}

#[test]
fn duplicate_interned_keys_fail_preflight_before_decoder_overwrites() {
    // Two map members with the same binary-encoded interned key, but different
    // values. The storage decoder collapses these to the last value.
    let bytes = [0x82, 0xc4, 0x01, 0x01, 0xc0, 0xc4, 0x01, 0x01, 0x01];
    let decoded = InnerValue::from_bytes(bytes).expect("storage decoder accepts duplicate keys");
    let InnerValue::Map(fields) = decoded else {
        panic!("expected decoded map");
    };
    assert_eq!(fields.len(), 1);
    assert!(matches!(fields.values().next(), Some(InnerValue::Int(1))));

    assert!(matches!(
        preflight(&bytes),
        Err(DbError::Codec(message)) if message == "duplicate interned record map key"
    ));
}

#[test]
fn nested_values_can_exceed_the_total_node_count_when_within_memory_budget() {
    let entries = 32_769;
    let bytes = map_with_unique_keys(entries);
    assert!(bytes.len() < MAX_ROW_BYTES);
    let working_bytes = preflight(&bytes).expect("map fits working budget");
    assert!(bytes.len() + working_bytes <= MAX_WORKING_BYTES);
}

#[test]
fn nested_values_cannot_exceed_the_64_mib_working_budget() {
    // Six legal 65,536-element lists stay below the 16 MiB source-row ceiling,
    // but their per-node accounting exceeds the 64 MiB working budget.
    let mut bytes = vec![0xdc, 0x00, 0x06];
    let mut total_nodes = 1usize;
    for _ in 0..6 {
        bytes.extend_from_slice(&[0xdd, 0x00, 0x01, 0x00, 0x00]);
        bytes.resize(bytes.len() + MAX_CONTAINER_ENTRIES, 0xc0);
        total_nodes += MAX_CONTAINER_ENTRIES + 1;
    }
    assert!(bytes.len() < MAX_ROW_BYTES);
    assert!(total_nodes * NODE_WORKING_BYTES > MAX_WORKING_BYTES);
    assert!(preflight(&bytes).is_err());
}
