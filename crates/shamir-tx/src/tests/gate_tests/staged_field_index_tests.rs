use crate::staging_store::StagingStore;
use crate::tx::support::staged_field_index::{fold_decodes, reset_fold_decodes};
use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey};
use shamir_types::core::interner::InternerKey;
use shamir_types::record_view::{scalar_ref_numeric_eq, RecordRef, RecordView};
use shamir_types::types::value::{InnerValue, QueryValue};
use std::sync::Arc;

fn record(field: &InternerKey, value: InnerValue) -> Bytes {
    let mut fields = shamir_types::types::common::new_map();
    fields.insert(field.clone(), value);
    InnerValue::Map(fields)
        .to_bytes()
        .expect("record serialization")
}
fn key(n: u32) -> RecordKey {
    Bytes::copy_from_slice(&n.to_le_bytes()).into()
}
fn matches_bytes(bytes: &[u8], field: &InternerKey, value: &QueryValue) -> Result<bool, ()> {
    match RecordView::new(bytes) {
        Ok(row) => Ok(row
            .scalar_at(std::slice::from_ref(field))
            .is_some_and(|s| scalar_ref_numeric_eq(s, value))),
        Err(_) => {
            let row = InnerValue::from_bytes(bytes).map_err(|_| ())?;
            Ok(row
                .scalar_at(std::slice::from_ref(field))
                .is_some_and(|s| scalar_ref_numeric_eq(s, value)))
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
struct Oracle {
    matches: Vec<RecordKey>,
    undecodable: Vec<RecordKey>,
}
fn oracle(
    rows: impl Iterator<Item = (RecordKey, Bytes)>,
    field: &InternerKey,
    value: &QueryValue,
) -> Oracle {
    let (mut matches, mut undecodable) = (Vec::new(), Vec::new());
    for (row_key, bytes) in rows {
        match matches_bytes(&bytes, field, value) {
            Ok(true) => matches.push(row_key),
            Ok(false) => (),
            Err(()) => undecodable.push(row_key),
        }
    }
    matches.sort();
    undecodable.sort();
    Oracle {
        matches,
        undecodable,
    }
}
fn linear(staging: &StagingStore, field: &InternerKey, value: &QueryValue) -> Oracle {
    oracle(
        staging.iter_ops().filter_map(|op| match op {
            KvOp::Set(k, b) => Some((k, b)),
            _ => None,
        }),
        field,
        value,
    )
}
fn indexed(staging: &StagingStore, field: &InternerKey, value: &QueryValue) -> Oracle {
    oracle(staging.probe_field(field, value).into_iter(), field, value)
}
fn parity(staging: &StagingStore, field: &InternerKey, queries: &[QueryValue], step: usize) {
    for query in queries {
        assert_eq!(
            indexed(staging, field, query),
            linear(staging, field, query),
            "step {step}, query {query:?}"
        );
    }
}

#[test]
fn rv_engine_0db9257b_fold_decodes_linear() {
    let field = InternerKey::new(1);
    let mut staging = StagingStore::new(Arc::new(InMemoryStore::new()));
    reset_fold_decodes();
    for i in 0..200 {
        assert!(indexed(&staging, &field, &QueryValue::Int(i))
            .matches
            .is_empty());
        staging.set(key(i as u32), record(&field, InnerValue::Int(i)));
    }
    assert!(indexed(&staging, &field, &QueryValue::Int(199))
        .matches
        .contains(&key(199)));
    assert_eq!(fold_decodes(), 200, "folded {} rows", fold_decodes());
}

#[tokio::test]
async fn rv_engine_0db9257b_index_matches_linear_scan() {
    let field = InternerKey::new(1);
    let mut staging = StagingStore::new(Arc::new(InMemoryStore::new()));
    for (i, value) in [
        InnerValue::Int(5),
        InnerValue::F64(5.0),
        InnerValue::Str("5".into()),
        InnerValue::Str("text".into()),
        InnerValue::Null,
        InnerValue::Bool(true),
        InnerValue::List(vec![InnerValue::Int(5)]),
    ]
    .into_iter()
    .enumerate()
    {
        staging.set(key(i as u32), record(&field, value));
    }
    staging.set_many([
        (key(10), record(&field, InnerValue::Int(-5))),
        (key(11), record(&field, InnerValue::Str("-5".into()))),
    ]);

    // Verify corrupt-candidate handling alongside matching valid candidates.
    staging.set(key(20), Bytes::from_static(b"\xc4"));
    let int_five = QueryValue::Int(5);
    let corrupt = indexed(&staging, &field, &int_five);
    assert_eq!(corrupt.undecodable, vec![key(20)]);
    assert!(corrupt.matches.contains(&key(0)) && corrupt.matches.contains(&key(1)));
    let expected = linear(&staging, &field, &int_five);
    assert_eq!(corrupt, expected);
    staging.remove(key(20));

    let mut seed = 0x225u32;
    let mut recent_int = 5i64;
    for step in 0..320 {
        let advance = |s: &mut u32| {
            *s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *s
        };
        let op = advance(&mut seed) % 5;
        let value_choice = advance(&mut seed) % 11;
        let target = key(advance(&mut seed) % 32);
        let value = match value_choice {
            0 => InnerValue::Int(5),
            1 => InnerValue::Int((step as i64 % 17) - 8),
            2 => InnerValue::F64(5.0),
            3 => InnerValue::F64(f64::NAN),
            4 => InnerValue::Str("5".into()),
            5 => InnerValue::Str("1000".into()),
            6 => InnerValue::Str("plain".into()),
            7 => InnerValue::Null,
            8 => InnerValue::Bool(true),
            9 => InnerValue::Str("text".into()),
            _ => InnerValue::Null,
        };
        let value = (value_choice != 10).then_some(value);
        let written_int = if matches!(op, 0 | 1 | 3) {
            value.as_ref().and_then(|v| {
                if let InnerValue::Int(n) = v {
                    Some(*n)
                } else {
                    None
                }
            })
        } else {
            None
        };
        let bytes = || {
            value
                .clone()
                .map(|v| record(&field, v))
                .unwrap_or_else(|| Bytes::from_static(b"\xc4"))
        };
        match op {
            0 => staging.set(target, bytes()),
            1 => staging.set_many([(target, bytes())]),
            2 => staging.remove(target),
            3 => {
                let overwrite = key(0);
                staging.set(overwrite.clone(), record(&field, InnerValue::Int(5)));
                recent_int = 5;
                staging.set(overwrite, bytes());
            }
            _ => staging
                .rewrite_set_bytes(|bytes| {
                    let Ok(mut row) = InnerValue::from_bytes(bytes) else {
                        return Ok(bytes.clone());
                    };
                    if let InnerValue::Map(ref mut fields) = row {
                        fields.insert(field.clone(), InnerValue::Int(recent_int));
                    }
                    row.to_bytes().map_err(|e| e.to_string())
                })
                .await
                .unwrap(),
        }
        if let Some(n) = written_int {
            recent_int = n;
        }
        let queries = [
            QueryValue::Int(5),
            QueryValue::Int(recent_int),
            QueryValue::F64(5.0),
            QueryValue::Str("5".into()),
            QueryValue::Str("plain".into()),
            QueryValue::Str("text".into()),
            QueryValue::Str("1000".into()),
            QueryValue::Null,
            QueryValue::Bool(true),
        ];
        parity(&staging, &field, &queries, step);
    }
}
