//! Sidecar metadata codec tests (moved from the inline cfg(test) module in
//! `entry/segment_meta.rs`).

use crate::segment_meta::{decode, encode, META_TOTAL_LEN};

#[test]
fn rv_wal_cc3131a9_sparse_sidecar_with_trailing_bytes_is_rejected() {
    let dir = tempfile::TempDir::new().unwrap();
    let segment = dir.path().join("00000000.wal");
    let meta = segment.with_extension("meta");
    let bytes = encode(42, false);
    std::fs::write(&meta, bytes).unwrap();
    let file = std::fs::OpenOptions::new().write(true).open(&meta).unwrap();
    file.set_len(128 * 1024 * 1024).unwrap();
    assert_eq!(crate::segment_meta::read_blocking(&segment).unwrap(), None);
}

#[test]
fn roundtrip_encode_decode() {
    for v in [0u64, 1, 42, u64::MAX, 1 << 40] {
        let bytes = encode(v, false);
        assert_eq!(decode(&bytes), Some((v, Some(false))), "roundtrip for {v}");
        assert_eq!(decode(&encode(v, true)), Some((v, Some(true))));
    }
}

#[test]
fn decode_rejects_bad_magic() {
    let mut bytes = encode(99, false);
    bytes[0] = b'X';
    assert_eq!(decode(&bytes), None);
}

#[test]
fn decode_rejects_bad_version() {
    // Unknown versions must fall back to replay.
    let mut bytes = encode(99, false);
    bytes[4] = 4;
    assert_eq!(decode(&bytes), None);
}

#[test]
fn decode_rejects_wmt1_magic() {
    // Hand-build a well-formed LEGACY WMT1 sidecar: same layout, only the
    // magic/version differ. It must never decode (never trusted).
    let mut buf = [0u8; 17];
    buf[0..4].copy_from_slice(b"WMT1");
    buf[4] = 1;
    buf[5..13].copy_from_slice(&99u64.to_le_bytes());
    let crc = crc32fast::hash(&buf[0..13]);
    buf[13..17].copy_from_slice(&crc.to_le_bytes());
    assert_eq!(decode(&buf), None);
}

#[test]
fn decode_rejects_bad_crc() {
    let mut bytes = encode(99, false);
    // Flip a byte in the max_version field without fixing the CRC.
    bytes[5] ^= 0xFF;
    assert_eq!(decode(&bytes), None);
}

#[test]
fn decode_rejects_wrong_length() {
    let bytes = encode(99, false);
    assert_eq!(decode(&bytes[..META_TOTAL_LEN - 1]), None, "truncated");
    let mut extended = bytes.to_vec();
    extended.push(0);
    assert_eq!(decode(&extended), None, "trailing garbage");
    assert_eq!(decode(&[]), None, "empty");
}
