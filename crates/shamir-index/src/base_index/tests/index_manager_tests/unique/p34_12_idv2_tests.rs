//! Identity-v2 base-index behaviour tests (design P34-12, stage S2).
//!
//! RED `rv_index_9e472725_composite_int_unique_no_false_duplicate`: under
//! the OLD v3 identity (two rustc-hash streams with one multiplier, one
//! physical slot per 25-byte key) the two composite tuples here collapsed
//! to the same posting key, so the second insert failed `DuplicateKey`.
//! Under IDV2 the canonical bytes differ and BOTH rows are admitted and
//! found.
//!
//! K citation: `Cargo.lock` pins rustc-hash 2.1.2; the 64-bit multiplier is
//! `K = 0xf1357aea2e62a9c5` (`rustc-hash-2.1.2/src/lib.rs:77`). The old
//! `NumericKey::hash_tagged` calls `i.hash(state)` (`shamir-types/.../numeric_key.rs:163-166`);
//! Rust's `i64: Hash` calls `write_i64`, whose default is `write_u64`
//! (`core/hash/mod.rs:417-419`); rustc-hash 2.1.2's `write_u64` feeds the
//! integer directly into `add_to_hash` (`lib.rs:123-125, 151-154`). Thus
//! the old stream over Int leaves is affine in their payloads, with the
//! two-leaf collision witness below verified from those operations.

use bytes::Bytes;
use shamir_storage::error::DbError;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::tests::index_manager_tests::helpers::{create_manager, create_test_value};

/// The 64-bit rustc-hash 2.1.2 multiplier, copied as a literal from the
/// pinned source (`rustc-hash-2.1.2/src/lib.rs:77`).
const K: u64 = 0xf1357aea2e62a9c5;

const FIELD_A: u64 = 401;
const FIELD_B: u64 = 402;
const NAME: u64 = 9100;

fn composite_def() -> IndexDefinition {
    IndexDefinition::new(
        NAME,
        vec![
            IndexInfoItem::new(vec![FIELD_A]),
            IndexInfoItem::new(vec![FIELD_B]),
        ],
    )
}

/// RED rv_index_9e472725: `(0, 0)` and
/// `(1, (0u64.wrapping_sub(K.wrapping_mul(K))) as i64)` share the old v3
/// unique hash key; IDV2 admits both and both are found.
#[tokio::test]
async fn rv_index_9e472725_composite_int_unique_no_false_duplicate() {
    let (_, _, manager) = create_manager();
    manager.create_unique_index(composite_def()).await.unwrap();

    let b_witness = (0u64.wrapping_sub(K.wrapping_mul(K))) as i64;
    let row_a = create_test_value(&[(FIELD_A, InnerValue::Int(0)), (FIELD_B, InnerValue::Int(0))]);
    let row_b = create_test_value(&[
        (FIELD_A, InnerValue::Int(1)),
        (FIELD_B, InnerValue::Int(b_witness)),
    ]);

    let id_a = RecordId::new();
    let id_b = RecordId::new();
    manager
        .validate_unique_for_create(&row_a)
        .await
        .expect("row A must be admitted");
    manager
        .on_record_created_unique(&id_a, &row_a)
        .await
        .unwrap();

    // OLD code: DuplicateKey (both v3 FxHasher streams collapsed these two
    // tuples onto the same 25-byte key). NEW code: admitted.
    manager
        .validate_unique_for_create(&row_b)
        .await
        .expect("row B must be admitted: distinct canonical bytes, no hash slot");
    manager
        .on_record_created_unique(&id_b, &row_b)
        .await
        .unwrap();

    // Both rows are found by their exact lookup values.
    let found_a = manager
        .lookup_by_unique_index(NAME, &[InnerValue::Int(0), InnerValue::Int(0)])
        .await
        .unwrap();
    assert_eq!(found_a, Some(id_a), "row A must be found");
    let found_b = manager
        .lookup_by_unique_index(NAME, &[InnerValue::Int(1), InnerValue::Int(b_witness)])
        .await
        .unwrap();
    assert_eq!(found_b, Some(id_b), "row B must be found");
}

/// Numeric-equal tuples remain duplicates across types — the ONE equality
/// rule is unchanged by v2: `(Int 1, Int 2)` vs `(F64 1.0, Dec 2)`.
#[tokio::test]
async fn rv_index_9e472725_true_duplicate_still_rejected() {
    let (_, _, manager) = create_manager();
    manager.create_unique_index(composite_def()).await.unwrap();

    let row_a = create_test_value(&[(FIELD_A, InnerValue::Int(1)), (FIELD_B, InnerValue::Int(2))]);
    let row_b = create_test_value(&[
        (FIELD_A, InnerValue::F64(1.0)),
        (FIELD_B, InnerValue::Dec(rust_decimal::Decimal::from(2))),
    ]);
    manager
        .on_record_created_unique(&RecordId::new(), &row_a)
        .await
        .unwrap();

    let err = manager
        .validate_unique_for_create(&row_b)
        .await
        .expect_err("numeric-equal tuple must stay a duplicate");
    assert!(
        matches!(err, DbError::DuplicateKey(_)),
        "expected DuplicateKey, got {err:?}"
    );
}

/// Long strings (> 256 canonical bytes) travel in DIGEST form: equal long
/// strings stay `DuplicateKey`, DISTINCT long strings are both admitted
/// (the digest key is only a locator; the value tail decides), and a
/// foreign tail at the probe's digest key is an `Internal` collision, not
/// a `DuplicateKey`.
#[tokio::test]
async fn rv_index_9e472725_digest_form_unique_semantics() {
    let (_, _, manager) = create_manager();
    let def = IndexDefinition::new(NAME, vec![IndexInfoItem::new(vec![FIELD_A])]);
    manager.create_unique_index(def).await.unwrap();

    let long_a = "a".repeat(300);
    let long_a2 = long_a.clone();
    let long_b = {
        let mut s = "a".repeat(300);
        s.replace_range(299..300, "b");
        s
    };

    let row_a = create_test_value(&[(FIELD_A, InnerValue::Str(long_a))]);
    let row_a2 = create_test_value(&[(FIELD_A, InnerValue::Str(long_a2))]);
    let row_b = create_test_value(&[(FIELD_A, InnerValue::Str(long_b.clone()))]);

    manager
        .on_record_created_unique(&RecordId::new(), &row_a)
        .await
        .unwrap();

    // Equal long string -> DuplicateKey (tail equal, digest form).
    let err = manager
        .validate_unique_for_create(&row_a2)
        .await
        .expect_err("equal long strings must stay duplicates");
    assert!(matches!(err, DbError::DuplicateKey(_)), "got {err:?}");

    // Distinct long string -> Ok (BLAKE3 digests differ).
    manager
        .validate_unique_for_create(&row_b)
        .await
        .expect("distinct long string must be admitted");

    // Injected digest posting with a FOREIGN tail at row_b's digest key:
    // fail closed with Internal, never DuplicateKey, never an overwrite.
    let probe_key =
        crate::base_index::index_keys::build_index_key(true, NAME, &[InnerValue::Str(long_b)])
            .expect("probe has identity")
            .to_bytes();
    let owner = RecordId::new();
    let foreign_tail = vec![0xEE; 300];
    let injected = {
        let mut v = Vec::with_capacity(16 + foreign_tail.len());
        v.extend_from_slice(owner.as_bytes());
        v.extend_from_slice(&foreign_tail);
        Bytes::from(v)
    };
    manager
        .info_store
        .set(probe_key.into(), injected)
        .await
        .unwrap();
    let err = manager
        .validate_unique_for_create(&row_b)
        .await
        .expect_err("foreign digest tail must fail closed");
    assert!(
        matches!(err, DbError::Internal(_)),
        "expected Internal digest-collision error, got {err:?}"
    );
}

fn sorted_ids(ids: &[RecordId]) -> Vec<RecordId> {
    let mut sorted = ids.to_vec();
    sorted.sort();
    sorted
}

/// Regular lookup exactness: "a"/"ab" never cross-match; inline and digest
/// values both round-trip; the doctor's entry count matches the number of
/// rows that lookups find.
#[tokio::test]
async fn rv_index_9e472725_regular_lookup_exactness() {
    let (_, _, manager) = create_manager();
    let def = IndexDefinition::new(NAME, vec![IndexInfoItem::new(vec![FIELD_A])]);
    manager.create_index(def).await.unwrap();

    let mut ids_short = Vec::new();
    for _ in 0..3 {
        let row = create_test_value(&[(FIELD_A, InnerValue::Str("a".to_string()))]);
        let id = RecordId::new();
        manager.on_record_created(&id, &row).await.unwrap();
        ids_short.push(id);
    }
    let mut ids_long = Vec::new();
    for _ in 0..2 {
        let row = create_test_value(&[(FIELD_A, InnerValue::Str("z".repeat(300)))]);
        let id = RecordId::new();
        manager.on_record_created(&id, &row).await.unwrap();
        ids_long.push(id);
    }

    // Inline exactness: "a" finds exactly the 3 short rows.
    let got = manager
        .lookup_by_index(NAME, &[InnerValue::Str("a".to_string())])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        sorted_ids(got.as_ref()),
        sorted_ids(&ids_short),
        "inline lookup exact"
    );

    // "ab" (a DIFFERENT inline identity, prefix of no concern) finds
    // nothing — no prefix bleed between self-delimiting identities.
    let got_ab = manager
        .lookup_by_index(NAME, &[InnerValue::Str("ab".to_string())])
        .await
        .unwrap()
        .unwrap();
    assert!(got_ab.is_empty(), "\"ab\" must not match \"a\" postings");

    // Digest form: the long value finds exactly its 2 rows.
    let got_long = manager
        .lookup_by_index(NAME, &[InnerValue::Str("z".repeat(300))])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        sorted_ids(got_long.as_ref()),
        sorted_ids(&ids_long),
        "digest-form lookup exact"
    );

    // A distinct long value sharing the same first byte finds nothing.
    let other = {
        let mut s = "z".repeat(300);
        s.replace_range(299..300, "q");
        s
    };
    let got_other = manager
        .lookup_by_index(NAME, &[InnerValue::Str(other)])
        .await
        .unwrap()
        .unwrap();
    assert!(got_other.is_empty(), "distinct digest value must not match");

    let probe_value = InnerValue::Str("z".repeat(300));
    let probe_key = crate::base_index::index_keys::build_index_key(
        false,
        NAME,
        std::slice::from_ref(&probe_value),
    )
    .expect("probe has identity")
    .to_bytes();
    let mut foreign_key = probe_key.to_vec();
    foreign_key.extend_from_slice(RecordId::new().as_bytes());
    let foreign_key = Bytes::from(foreign_key);
    manager
        .info_store
        .set(foreign_key.clone().into(), Bytes::from(vec![0xEE; 300]))
        .await
        .unwrap();
    for _ in 0..3 {
        let foreign_lookup = manager
            .lookup_by_index(NAME, std::slice::from_ref(&probe_value))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            sorted_ids(foreign_lookup.as_ref()),
            sorted_ids(&ids_long),
            "foreign tail excluded, also bypasses stale cache"
        );
    }

    manager.info_store.remove(foreign_key.into()).await.unwrap();
    // Count fast path parity: the doctor's entry count equals the number
    // of postings the lookups resolved (3 + 2).
    let count = manager.entry_count(NAME, false).await.unwrap();
    assert_eq!(count, 5, "entry count must equal inline+digest postings");
}
