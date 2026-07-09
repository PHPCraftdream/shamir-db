//! Category 2 — Hash consistency between inline and heap representations
//! of the SAME bytes, under both `shamir_collections::THasher` (FxHasher,
//! the workspace default for hash-keyed structures) and the stdlib default
//! hasher. This is the exact class of bug task #489 found and fixed in
//! `Value<Key>` (Hash/Eq NaN inconsistency) — a `#[derive(Hash)]` on the
//! `Repr` enum would make an inline 16-byte key hash differently from the
//! same bytes arriving as a heap `Bytes`, silently corrupting staging
//! maps, conflict detection, and caches.

use super::super::KeyBytes;
use shamir_collections::{TFxMap, THasher};
use std::hash::{BuildHasher, Hash, Hasher};

/// Several short byte strings that exercise the inline path, including
/// edge lengths and a realistic 16-byte `RecordId`-shaped key.
const SAMPLES: &[&[u8]] = &[
    &[],
    &[0xFF],
    &[1, 2, 3, 4, 5],
    &[0u8; 16],                        // RecordId-shape
    &[0xAB; super::super::INLINE_CAP], // exactly at the inline boundary
];

#[test]
fn inline_and_heap_hash_identically_under_fxhasher() {
    let bh = THasher::default();
    for &bytes in SAMPLES {
        let inline = KeyBytes::from_slice(bytes);
        let heap = KeyBytes::force_heap_for_test(bytes);
        assert!(
            inline.is_inline(),
            "sanity (len={}): expected inline",
            bytes.len()
        );
        assert!(
            !heap.is_inline(),
            "sanity (len={}): expected forced heap",
            bytes.len()
        );

        let in_hash = bh.hash_one(&inline);
        let hp_hash = bh.hash_one(&heap);
        assert_eq!(
            in_hash, hp_hash,
            "FxHasher divergence for bytes={bytes:?}: inline={in_hash:#x} heap={hp_hash:#x}"
        );
    }
}

#[test]
fn inline_and_heap_hash_identically_under_std_default_hasher() {
    use std::hash::DefaultHasher;
    for &bytes in SAMPLES {
        let inline = KeyBytes::from_slice(bytes);
        let heap = KeyBytes::force_heap_for_test(bytes);

        let mut a = DefaultHasher::new();
        inline.hash(&mut a);
        let mut b = DefaultHasher::new();
        heap.hash(&mut b);
        assert_eq!(
            a.finish(),
            b.finish(),
            "DefaultHasher divergence for bytes={bytes:?}"
        );
    }
}

#[test]
fn inline_and_heap_are_mutually_interchangeable_in_fxhashed_map() {
    // The end-to-end consequence the hash-consistency guard protects
    // against: a `TFxMap<KeyBytes, _>` (the shape of staging/cached key
    // maps) must look up an inline key with a forced-heap probe (and
    // vice versa).
    let mut map: TFxMap<KeyBytes, u32> = TFxMap::default();
    for (i, &bytes) in SAMPLES.iter().enumerate() {
        map.insert(KeyBytes::from_slice(bytes), i as u32);
    }
    for (i, &bytes) in SAMPLES.iter().enumerate() {
        // Probe with the OPPOSITE representation from how it was inserted.
        let probe = KeyBytes::force_heap_for_test(bytes);
        assert_eq!(
            map.get(&probe),
            Some(&(i as u32)),
            "inline-inserted key not found via forced-heap probe: bytes={bytes:?}"
        );
    }

    // And the reverse direction: insert forced-heap, probe inline.
    let mut map2: TFxMap<KeyBytes, u32> = TFxMap::default();
    for (i, &bytes) in SAMPLES.iter().enumerate() {
        map2.insert(KeyBytes::force_heap_for_test(bytes), i as u32);
    }
    for (i, &bytes) in SAMPLES.iter().enumerate() {
        let probe = KeyBytes::from_slice(bytes);
        assert_eq!(
            map2.get(&probe),
            Some(&(i as u32)),
            "forced-heap-inserted key not found via inline probe: bytes={bytes:?}"
        );
    }
}
