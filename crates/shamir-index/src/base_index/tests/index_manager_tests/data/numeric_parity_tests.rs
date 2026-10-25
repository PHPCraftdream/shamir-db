//! RV-FUNCLIB-5FA455F7 (D1-S1): index probe parity with the scan filter
//! for numeric equality — canonical keys (Int↔F64 parity, -0.0 → 0,
//! NaN unindexed). Eq parity: regular + unique + functional. IN union:
//! regular + functional only (the unique manager has no IN lookup).

use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::tests::index_manager_tests::helpers::{create_manager, create_test_value};
use shamir_storage::error::DbError;
use shamir_types::record_view::scalar_ref_cmp;
use shamir_types::record_view::ScalarRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::cmp::Ordering;
use std::collections::BTreeSet;

const CORPUS: &[InnerValue] = &[
    InnerValue::Int(0),
    InnerValue::F64(0.0),
    InnerValue::F64(-0.0),
    InnerValue::Int(1),
    InnerValue::F64(1.0),
    InnerValue::F64(1.5),
    InnerValue::F64(f64::NAN),
    InnerValue::Int(i64::MAX),
    InnerValue::Int(i64::MIN),
    InnerValue::Int(9007199254740993),       // 2^53 + 1
    InnerValue::Int(9007199254740992),       // 2^53
    InnerValue::F64(9007199254740992.0),     // 2^53, exactly representable
    InnerValue::F64(1.7976931348623157e308), // f64::MAX
];

fn as_scalar(v: &InnerValue) -> ScalarRef<'static> {
    match v {
        InnerValue::Int(i) => ScalarRef::Int(*i),
        InnerValue::F64(f) => ScalarRef::F64(*f),
        _ => unreachable!("corpus holds only numeric leaves"),
    }
}

async fn setup_corpus() -> (
    crate::base_index::index_manager::IndexManager,
    Vec<RecordId>,
) {
    let (data_store, _, manager) = create_manager();
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();
    let mut ids = Vec::with_capacity(CORPUS.len());
    for leaf in CORPUS {
        let value = create_test_value(&[(1, leaf.clone())]);
        let record_id = RecordId::new();
        data_store
            .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
            .await
            .unwrap();
        manager.on_record_created(&record_id, &value).await.unwrap();
        ids.push(record_id);
    }
    (manager, ids)
}

#[tokio::test]
async fn rv_funclib_5fa455f7_index_probe_parity_with_scan_filter() {
    let (manager, ids) = setup_corpus().await;
    for (probe_i, probe) in CORPUS.iter().enumerate() {
        let expected: Vec<RecordId> = CORPUS
            .iter()
            .zip(&ids)
            .filter(|(leaf, _)| scalar_ref_cmp(as_scalar(probe), leaf) == Some(Ordering::Equal))
            .map(|(_, id)| *id)
            .collect();
        let found = manager
            .lookup_by_index(1001, std::slice::from_ref(probe))
            .await
            .unwrap()
            .unwrap_or_default();
        for id in &expected {
            assert!(
                found.contains(id),
                "probe #{probe_i} must find its equal record (parity with scan filter)"
            );
        }
        assert_eq!(
            found.len(),
            expected.len(),
            "probe #{probe_i} found extra records beyond the scan-filter oracle"
        );
    }
}

#[tokio::test]
async fn rv_funclib_5fa455f7_neg_zero_and_zero_probes_find_each_other() {
    let (manager, ids) = setup_corpus().await;
    let zero_leaves: Vec<RecordId> = CORPUS
        .iter()
        .zip(&ids)
        .filter(|(leaf, _)| {
            matches!(leaf, InnerValue::Int(0)) || matches!(leaf, InnerValue::F64(f) if *f == 0.0)
        })
        .map(|(_, id)| *id)
        .collect();
    assert_eq!(zero_leaves.len(), 3);
    let nan_id = ids[6];

    for probe in [
        InnerValue::Int(0),
        InnerValue::F64(0.0),
        InnerValue::F64(-0.0),
    ] {
        let found = manager
            .lookup_by_index(1001, std::slice::from_ref(&probe))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            found.len(),
            3,
            "probe {probe:?} must find exactly the zeros"
        );
        for id in &zero_leaves {
            assert!(found.contains(id));
        }
        assert!(!found.contains(&nan_id));
    }

    let found = manager
        .lookup_by_index(1001, &[InnerValue::F64(f64::NAN)])
        .await
        .unwrap()
        .unwrap();
    assert!(found.is_empty(), "NaN probe must match nothing");
}

/// Oracle: records Equal (per the scan filter's `scalar_ref_cmp`) to ANY
/// member of the IN set. The planner evaluates IN as a union of per-value
/// probes (read_planner.rs `try_plan_index_scan`: one lookup value set per
/// member, results unioned), so the union must equal the oracle exactly.
#[tokio::test]
async fn rv_funclib_5fa455f7_in_probe_union_matches_scan() {
    let (manager, ids) = setup_corpus().await;
    let in_sets: Vec<Vec<InnerValue>> = vec![
        vec![InnerValue::Int(1), InnerValue::F64(-0.0)],
        vec![InnerValue::F64(1.0), InnerValue::F64(f64::NAN)],
        vec![InnerValue::Int(0), InnerValue::F64(0.0)],
        vec![
            InnerValue::Int(9007199254740993),
            InnerValue::F64(9007199254740992.0),
        ],
        vec![
            InnerValue::Int(i64::MAX),
            InnerValue::F64(1.7976931348623157e308),
        ],
        vec![
            InnerValue::F64(0.0),
            InnerValue::F64(1.0),
            InnerValue::F64(f64::NAN),
        ],
    ];
    for (set_i, set) in in_sets.iter().enumerate() {
        let mut union: BTreeSet<RecordId> = BTreeSet::new();
        for probe in set {
            let found = manager
                .lookup_by_index(1001, std::slice::from_ref(probe))
                .await
                .unwrap()
                .unwrap_or_default();
            union.extend(found.iter().copied());
        }
        let expected: BTreeSet<RecordId> = CORPUS
            .iter()
            .zip(&ids)
            .filter(|(leaf, _)| {
                set.iter()
                    .any(|probe| scalar_ref_cmp(as_scalar(probe), leaf) == Some(Ordering::Equal))
            })
            .map(|(_, id)| *id)
            .collect();
        assert_eq!(
            union, expected,
            "IN set #{set_i}: union of per-value probes must equal the scan-filter oracle"
        );
    }
}

#[tokio::test]
async fn rv_funclib_5fa455f7_unique_int_after_f64_conflict() {
    let (data_store, _, manager) = create_manager();
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_unique_index(index_def).await.unwrap();

    // Key-equality assertions (pin the shared canonical key).
    let int_one = manager.unique_keys_for(&create_test_value(&[(1, InnerValue::Int(1))]));
    let f64_one = manager.unique_keys_for(&create_test_value(&[(1, InnerValue::F64(1.0))]));
    assert!(!int_one.is_empty());
    assert_eq!(int_one, f64_one, "Int(1) and F64(1.0) must share one key");

    let nan = manager.unique_keys_for(&create_test_value(&[(1, InnerValue::F64(f64::NAN))]));
    assert!(
        nan.is_empty(),
        "NaN must claim no unique key (never conflicts with NaN)"
    );

    // Behavioral validation: insert F64(1.0), then Int(1) must be a
    // unique conflict through `validate_unique_for_create` (the same
    // canonical builder the write path uses).
    let f64_rec = create_test_value(&[(1, InnerValue::F64(1.0))]);
    let rid = RecordId::new();
    data_store
        .set(rid.to_bytes().into(), f64_rec.to_bytes().unwrap())
        .await
        .unwrap();
    // unique postings land via on_record_created_unique (validate.rs); the regular-family on_record_created is a no-op here.
    manager
        .on_record_created_unique(&rid, &f64_rec)
        .await
        .unwrap();

    for val in [InnerValue::Int(1), InnerValue::F64(1.0)] {
        let res = manager
            .validate_unique_for_create(&create_test_value(&[(1, val.clone())]))
            .await;
        assert!(
            matches!(res, Err(DbError::DuplicateKey(_))),
            "{val:?} must conflict with the inserted F64(1.0)"
        );
    }

    // NaN-then-NaN inserts both succeed and claim no conflict.
    for _ in 0..2 {
        let nan_rec = create_test_value(&[(1, InnerValue::F64(f64::NAN))]);
        assert!(
            manager.validate_unique_for_create(&nan_rec).await.is_ok(),
            "NaN must not conflict with the stored F64(1.0) or a prior NaN"
        );
        let nan_rid = RecordId::new();
        data_store
            .set(nan_rid.to_bytes().into(), nan_rec.to_bytes().unwrap())
            .await
            .unwrap();
        manager
            .on_record_created_unique(&nan_rid, &nan_rec)
            .await
            .unwrap();
    }
    let res = manager
        .validate_unique_for_create(&create_test_value(&[(1, InnerValue::F64(f64::NAN))]))
        .await;
    assert!(res.is_ok(), "NaN must not conflict with NaN");
    // ...while Int(1) is still a conflict.
    let res = manager
        .validate_unique_for_create(&create_test_value(&[(1, InnerValue::Int(1))]))
        .await;
    assert!(
        matches!(res, Err(DbError::DuplicateKey(_))),
        "Int(1) must still conflict after NaN inserts"
    );
}

/// RV-FUNCLIB-5FA455F7: unique-index equality matrix over canonical
/// equivalence classes. For each class a FRESH manager stores exactly ONE
/// representative (validate first, then claim) and EVERY corpus value is
/// probed: DuplicateKey iff the probe is in the same class. NaN is in no
/// class: a stored NaN claims no key and a NaN probe never conflicts.
///
/// Revert sensitivity: reverting ONLY the canonical numeric key to the
/// old tag+bits encoding breaks the cross-variant pairs in this matrix
/// ({Int(1), F64(1.0)}, {Int(0), F64(0.0), F64(-0.0)}, {Int(2^53),
/// F64(2^53)}): the probe of the other variant no longer conflicts with the
/// stored representative. Each probe sees only the one stored value, so a
/// same-variant match cannot hide the missing cross-variant conflict.
/// This test covers equality conflicts only; it makes no IN-set claim.
#[tokio::test]
async fn rv_funclib_5fa455f7_unique_class_conflict_matrix() {
    // Canonical equivalence class of a corpus value; None = NaN (no class).
    fn class_of(v: &InnerValue) -> Option<usize> {
        match v {
            InnerValue::Int(0) => Some(0),
            InnerValue::F64(f) if *f == 0.0 => Some(0), // +0.0 and -0.0
            InnerValue::Int(1) => Some(1),
            InnerValue::F64(f) if *f == 1.0 => Some(1),
            InnerValue::F64(f) if *f == 1.5 => Some(2),
            InnerValue::Int(i64::MAX) => Some(3),
            InnerValue::Int(i64::MIN) => Some(4),
            InnerValue::Int(9007199254740993) => Some(5),
            InnerValue::Int(9007199254740992) => Some(6),
            InnerValue::F64(f) if *f == 9007199254740992.0 => Some(6),
            InnerValue::F64(f) if *f == f64::MAX => Some(7),
            InnerValue::F64(f) if f.is_nan() => None,
            other => unreachable!("unknown corpus leaf {other:?}"),
        }
    }
    // Every corpus value is a representative once (classes with several
    // members are exercised from each member), NaN included.
    for rep in CORPUS {
        let (data_store, _, manager) = create_manager();
        let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
        manager.create_unique_index(index_def).await.unwrap();

        let rep_rec = create_test_value(&[(1, rep.clone())]);
        assert!(
            manager.validate_unique_for_create(&rep_rec).await.is_ok(),
            "representative {rep:?} must validate Ok on a fresh unique index"
        );
        let rid = RecordId::new();
        data_store
            .set(rid.to_bytes().into(), rep_rec.to_bytes().unwrap())
            .await
            .unwrap();
        manager
            .on_record_created_unique(&rid, &rep_rec)
            .await
            .unwrap();

        for probe in CORPUS {
            let expect_conflict = match (class_of(rep), class_of(probe)) {
                (Some(a), Some(b)) => a == b,
                _ => false, // NaN claims no key and never conflicts
            };
            let res = manager
                .validate_unique_for_create(&create_test_value(&[(1, probe.clone())]))
                .await;
            assert_eq!(
                matches!(res, Err(DbError::DuplicateKey(_))),
                expect_conflict,
                "stored {rep:?}, probe {probe:?}: conflict iff same equivalence class"
            );
        }
    }
}
