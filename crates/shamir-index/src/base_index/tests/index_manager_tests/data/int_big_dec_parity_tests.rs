//! RV-FUNCLIB-5FA455F7 (D1-S2): EXACT Int↔Big/Dec index parity — a `Big`
//! (or integral `Dec`) equal to some `Int` keys like that `Int` (shared
//! canonical numeric key in `shamir_types`), never through `f64`.

use crate::base_index::index_definition::IndexDefinition;
use crate::base_index::index_info_item::IndexInfoItem;
use crate::base_index::tests::index_manager_tests::helpers::{create_manager, create_test_value};
use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_types::record_view::scalar_ref_cmp;
use shamir_types::record_view::ScalarRef;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use std::cmp::Ordering;

fn corpus() -> Vec<InnerValue> {
    vec![
        InnerValue::Int(1),
        InnerValue::Big(BigInt::from(1)),
        InnerValue::Dec(Decimal::from(1)),
        InnerValue::Int(9007199254740993), // 2^53 + 1
        InnerValue::Big(BigInt::from(9007199254740993i64)),
        InnerValue::Big(BigInt::from(9007199254740992i64)), // 2^53
        // Beyond i64 (no f64 collapse possible either way):
        InnerValue::Big(BigInt::from(i64::MIN) - BigInt::from(1u8)),
        InnerValue::Dec(Decimal::new(5, 1)), // 0.5 — no integer equal
    ]
}

#[tokio::test]
async fn rv_funclib_5fa455f7_index_exact_int_big_dec_probe_parity() {
    let corpus = corpus();
    let (data_store, _, manager) = create_manager();
    let index_def = IndexDefinition::new(1001, vec![IndexInfoItem::new(vec![1])]);
    manager.create_index(index_def).await.unwrap();
    let mut ids = Vec::with_capacity(corpus.len());
    for leaf in &corpus {
        let value = create_test_value(&[(1, leaf.clone())]);
        let record_id = RecordId::new();
        data_store
            .set(record_id.to_bytes().into(), value.to_bytes().unwrap())
            .await
            .unwrap();
        manager.on_record_created(&record_id, &value).await.unwrap();
        ids.push(record_id);
    }

    // A probe is always an Int/F64 field value — exactly the FK-shaped
    // query "which records hold a value equal to this Int?". The oracle is
    // the scan filter's `scalar_ref_cmp` (exact Int↔Big since S2).
    for (probe_i, probe) in corpus.iter().enumerate() {
        let probe_scalar = match probe {
            InnerValue::Int(i) => ScalarRef::Int(*i),
            InnerValue::F64(f) => ScalarRef::F64(*f),
            _ => continue, // Big/Dec leaves have no ScalarRef probe form
        };
        let expected: Vec<RecordId> = corpus
            .iter()
            .zip(&ids)
            .filter(|(leaf, _)| scalar_ref_cmp(probe_scalar, leaf) == Some(Ordering::Equal))
            .map(|(_, id)| *id)
            .collect();
        let found = manager
            .lookup_by_index(1001, std::slice::from_ref(probe))
            .await
            .unwrap()
            .unwrap_or_default();
        // EXACT set equality: an index returning extra ids must fail too.
        let mut expected_sorted = expected;
        expected_sorted.sort();
        let mut found_sorted: Vec<RecordId> = found.iter().copied().collect();
        found_sorted.sort();
        assert_eq!(
            found_sorted, expected_sorted,
            "probe #{probe_i} must return EXACTLY its exactly-equal records (Int↔Big/Dec parity)"
        );
    }
}
