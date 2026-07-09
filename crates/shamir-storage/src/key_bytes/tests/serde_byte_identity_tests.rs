//! Category 3 — Serde byte-identity vs `bytes::Bytes`.
//!
//! `KeyBytes`'s serde encoding MUST be byte-for-byte identical to the
//! encoding the WAL uses for `Bytes` today (plan doc §5.3): the
//! `serde_bytes_bytes` helper in `shamir_wal::wal_entry_v2`, which is
//!   serialize:   serde_bytes::Bytes::new(b.as_ref()).serialize(s)
//!   deserialize: serde_bytes::ByteBuf::deserialize(d) -> Bytes::from(vec)
//!
//! We cannot import that private module from `shamir-wal`, so we mirror
//! it locally as the reference encoder and confirm:
//!   (a) `bincode::serialize(&KeyBytes::from_slice(b))`
//!       == `bincode::serialize(&reference_bytes(b))` for representative
//!       lengths spanning the inline/heap boundary;
//!   (b) the same parity under `rmp-serde` (the client-wire encoder used
//!       in `shamir-client`; not currently used for storage keys, but the
//!       plan doc §5.3 requires the encoding be ready for any later
//!       step that crosses that boundary);
//!   (c) round-trip: `deserialize(serialize(x)) == x` for both reprs.
//!
//! `bincode` is the WAL's actual on-disk encoding (`wal_entry_v2.rs:218`,
//! `bincode::serialize_into`); `rmp-serde` is included defensively per
//! the plan doc.

use super::super::KeyBytes;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Local mirror of `shamir_wal::wal_entry_v2::serde_bytes_bytes` —
/// encodes `Bytes` exactly the way the WAL does today. We wrap a thin
/// newtype around `Bytes` so we can attach this impl without the
/// orphan-rule collision of impl'ing `Serialize` for `Bytes` directly
/// (which `Bytes` itself does not provide).
#[derive(Serialize, Deserialize)]
struct RefBytes(#[serde(with = "ref_bytes_bytes")] Bytes);

mod ref_bytes_bytes {
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(b: &Bytes, s: S) -> Result<S::Ok, S::Error> {
        serde_bytes::Bytes::new(b.as_ref()).serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Bytes, D::Error> {
        let bb = serde_bytes::ByteBuf::deserialize(d)?;
        Ok(Bytes::from(bb.into_vec()))
    }
}

fn ref_bytes(b: &[u8]) -> RefBytes {
    RefBytes(Bytes::copy_from_slice(b))
}

/// Representative lengths spanning both inline (0, 15, 23) and heap
/// (24, 31, 41) paths. 23 = `INLINE_CAP`; 24 forces heap.
const SAMPLE_LENS: &[usize] = &[
    0,
    15,
    super::super::INLINE_CAP,
    super::super::INLINE_CAP + 1,
    41,
];

fn sample_bytes(len: usize) -> Vec<u8> {
    // Distinct non-zero pattern so a length-mismatch or truncation in
    // the encoding is visible in the diff.
    (0..len).map(|i| ((i * 7 + 1) & 0xFF) as u8).collect()
}

#[test]
fn bincode_encoding_matches_bytes_byte_for_byte() {
    for &len in SAMPLE_LENS {
        let bytes = sample_bytes(len);
        let kb = KeyBytes::from_slice(&bytes);
        let kb_enc = bincode::serialize(&kb).expect("bincode serialize KeyBytes");
        let ref_enc = bincode::serialize(&ref_bytes(&bytes)).expect("bincode serialize RefBytes");
        assert_eq!(
            kb_enc, ref_enc,
            "bincode byte mismatch at len={len}: KeyBytes={kb_enc:?} Bytes={ref_enc:?}"
        );
    }
}

#[test]
fn rmp_serde_encoding_matches_bytes_byte_for_byte() {
    for &len in SAMPLE_LENS {
        let bytes = sample_bytes(len);
        let kb = KeyBytes::from_slice(&bytes);
        let kb_enc = rmp_serde::to_vec(&kb).expect("rmp serialize KeyBytes");
        let ref_enc = rmp_serde::to_vec(&ref_bytes(&bytes)).expect("rmp serialize RefBytes");
        assert_eq!(
            kb_enc, ref_enc,
            "rmp-serde byte mismatch at len={len}: KeyBytes={kb_enc:?} Bytes={ref_enc:?}"
        );
    }
}

#[test]
fn bincode_roundtrip_preserves_value_for_both_reprs() {
    for &len in SAMPLE_LENS {
        let bytes = sample_bytes(len);
        let original = KeyBytes::from_slice(&bytes);
        let enc = bincode::serialize(&original).expect("serialize");
        let back: KeyBytes = bincode::deserialize(&enc).expect("deserialize");
        assert_eq!(
            back.as_slice(),
            original.as_slice(),
            "roundtrip mismatch at len={len}"
        );
        assert_eq!(back, original, "Eq mismatch after roundtrip at len={len}");
    }
}

#[test]
fn rmp_serde_roundtrip_preserves_value_for_both_reprs() {
    for &len in SAMPLE_LENS {
        let bytes = sample_bytes(len);
        let original = KeyBytes::from_slice(&bytes);
        let enc = rmp_serde::to_vec(&original).expect("serialize");
        let back: KeyBytes = rmp_serde::from_slice(&enc).expect("deserialize");
        assert_eq!(back, original, "rmp roundtrip Eq mismatch at len={len}");
    }
}

#[test]
fn keybytes_decodes_what_bytes_encoded_and_vice_versa() {
    // Cross-decode: bytes serialized as `RefBytes` must deserialize as
    // `KeyBytes`, and vice versa. This is the strict on-disk/wire
    // compatibility guarantee a later alias flip relies on.
    for &len in SAMPLE_LENS {
        let bytes = sample_bytes(len);

        let ref_enc = bincode::serialize(&ref_bytes(&bytes)).unwrap();
        let from_ref: KeyBytes = bincode::deserialize(&ref_enc).unwrap();
        assert_eq!(from_ref.as_slice(), &bytes[..], "Bytes->KeyBytes len={len}");

        let kb = KeyBytes::from_slice(&bytes);
        let kb_enc = bincode::serialize(&kb).unwrap();
        let from_kb: RefBytes = bincode::deserialize(&kb_enc).unwrap();
        assert_eq!(from_kb.0.as_ref(), &bytes[..], "KeyBytes->Bytes len={len}");
    }
}
