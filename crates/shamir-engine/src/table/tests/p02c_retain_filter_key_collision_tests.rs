//! Regression test for sub-bug 2c's retain-filter key-collision safety.
//!
//! The retain filter in `tx/pre_commit.rs`
//! (`rederive_base_index_ops_post_stage`) identifies base_index posting ops
//! by a physical-layout heuristic: key length exactly 41 or 25 bytes **and**
//! first byte <= 1.  This test locks in the **current safe key-length /
//! prefix facts for every existing backend** so the test breaks loudly at CI
//! time the moment any backend's key format changes to collide — and serves
//! as the obvious "extend me when you add a backend" example referenced by
//! the contract comment above the filter.
//!
//! Backends that never produce posting ops (vector — its state lives in an
//! in-memory HNSW graph, not the posting-op pipeline) are intentionally
//! absent.

use shamir_types::types::record_id::RecordId;

use crate::index::index_record_key::IndexRecordKey;
use crate::index2::{build_posting_key, type_tag};

/// Mirrors the retain filter's danger-zone logic from
/// `pre_commit.rs::rederive_base_index_ops_post_stage` (the `retain`
/// closure): a posting key is treated as a base_index candidate iff its
/// length is exactly 41 or 25 bytes AND its first byte is <= 1.
fn is_base_index_retain_candidate(key: &[u8]) -> bool {
    (key.len() == 41 || key.len() == 25) && key.first().is_some_and(|&b| b <= 1)
}

/// SORTED_TAG as defined in
/// `shamir_index::base_index::sorted_index_definition`.
/// It is `pub(crate)` there so we duplicate the value here with a comment
/// pointing to its source.
const SORTED_TAG: u8 = 0x80;

/// Lock in the key-length / first-byte facts for every existing backend that
/// produces posting ops, proving none outside base_index falls into the
/// retain filter's danger zone.
#[test]
fn retain_filter_key_collision_safety() {
    let rid = RecordId::new();

    // ══════════════════════════════════════════════════════════════════════
    // base_index regular — the filter's TARGET (MUST be in the danger zone).
    // Format: IndexRecordKey(25 bytes) || RecordId(16 bytes) = 41 bytes.
    // IndexRecordKey::to_bytes = [is_unique=0:1][name_interned:8LE][h1:8LE][h2:8LE].
    // ══════════════════════════════════════════════════════════════════════
    let regular_irk = IndexRecordKey::new(false, 42).with_hash(0xDEAD_BEEF_CAFE, 0x1234_5678);
    let mut regular_key = regular_irk.to_bytes().to_vec();
    regular_key.extend_from_slice(rid.as_bytes());
    assert_eq!(
        regular_key.len(),
        41,
        "regular posting key must be 41 bytes"
    );
    assert_eq!(
        regular_key[0], 0,
        "regular key first byte is is_unique=false=0"
    );
    assert!(
        is_base_index_retain_candidate(&regular_key),
        "base_index regular posting key MUST be in the danger zone — \
         it is the filter's target"
    );

    // ══════════════════════════════════════════════════════════════════════
    // base_index unique — the filter's TARGET (MUST be in the danger zone).
    // Format: IndexRecordKey(25 bytes) only (no RecordId suffix — unique
    // keys map to a single record).
    // ══════════════════════════════════════════════════════════════════════
    let unique_irk = IndexRecordKey::new(true, 42).with_hash(0xDEAD_BEEF_CAFE, 0x1234_5678);
    let unique_key = unique_irk.to_bytes().to_vec();
    assert_eq!(unique_key.len(), 25, "unique posting key must be 25 bytes");
    assert_eq!(
        unique_key[0], 1,
        "unique key first byte is is_unique=true=1"
    );
    assert!(
        is_base_index_retain_candidate(&unique_key),
        "base_index unique posting key MUST be in the danger zone — \
         it is the filter's target"
    );

    // ══════════════════════════════════════════════════════════════════════
    // sorted — safe via FIRST-BYTE guard (not length).
    // Format: [SORTED_TAG=0x80:1][name_interned:8BE][encoded_value:var][rid:16].
    // We deliberately construct a key of EXACTLY 25 bytes (empty
    // encoded_value) to prove the first-byte guard — not the length — is
    // what saves sorted from misidentification.
    // ══════════════════════════════════════════════════════════════════════
    let sorted_25_key = {
        let mut buf = Vec::with_capacity(25);
        buf.push(SORTED_TAG); // 0x80 — always > 1
        buf.extend_from_slice(&42u64.to_be_bytes());
        // No encoded_value (0 bytes) — forces the 25-byte boundary length
        buf.extend_from_slice(rid.as_bytes());
        buf
    };
    assert_eq!(sorted_25_key.len(), 25);
    assert!(
        !is_base_index_retain_candidate(&sorted_25_key),
        "sorted posting key must NOT be in the danger zone — \
         first byte 0x80 > 1 saves it even at the 25-byte boundary length"
    );

    // ══════════════════════════════════════════════════════════════════════
    // FTS — safe via LENGTH guard (not first byte).
    // Format: build_posting_key = [index_id:4LE][type_tag:1][token_hash:8LE][rid:16] = 29 bytes.
    // With index_id=1, byte[0] = 0x01 (low byte of the u32 LE id) which IS
    // <= 1 — but the 29-byte length saves it.
    // ══════════════════════════════════════════════════════════════════════
    let fts_key = build_posting_key(1, type_tag::FTS, &0u64.to_le_bytes(), &rid);
    assert_eq!(fts_key.len(), 29, "FTS posting key must be 29 bytes");
    assert_eq!(fts_key[0], 1, "FTS key[0] is low byte of index_id=1");
    assert!(
        !is_base_index_retain_candidate(&fts_key),
        "FTS posting key must NOT be in the danger zone — \
         29-byte length saves it even though key[0]=1"
    );

    // ══════════════════════════════════════════════════════════════════════
    // FTS-ranked — same key format as FTS (29 bytes, type_tag::FTS).
    // Different index_id (2) for coverage; byte[0]=2 > 1, so it's doubly safe.
    // ══════════════════════════════════════════════════════════════════════
    let fts_ranked_key = build_posting_key(2, type_tag::FTS, &0u64.to_le_bytes(), &rid);
    assert_eq!(fts_ranked_key.len(), 29);
    assert!(
        !is_base_index_retain_candidate(&fts_ranked_key),
        "FTS-ranked posting key must NOT be in the danger zone"
    );

    // ══════════════════════════════════════════════════════════════════════
    // functional — safe via LENGTH guard (not first byte).
    // Format: [index_id:4LE][type_tag:1][value_hash:16][rid:16] = 37 bytes.
    // With index_id=1, byte[0] = 0x01 which IS <= 1 — but the 37-byte length
    // saves it.
    // ══════════════════════════════════════════════════════════════════════
    let func_key = build_posting_key(1, type_tag::FUNCTIONAL, &[0xAB; 16], &rid);
    assert_eq!(
        func_key.len(),
        37,
        "functional posting key must be 37 bytes"
    );
    assert_eq!(
        func_key[0], 1,
        "functional key[0] is low byte of index_id=1"
    );
    assert!(
        !is_base_index_retain_candidate(&func_key),
        "functional posting key must NOT be in the danger zone — \
         37-byte length saves it even though key[0]=1"
    );
}
