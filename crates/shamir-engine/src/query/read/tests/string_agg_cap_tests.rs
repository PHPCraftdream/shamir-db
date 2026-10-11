use bytes::Bytes;
use shamir_query_builder::select;
use shamir_query_builder::val::{col, lit, null};
use shamir_storage::error::DbError;
use shamir_types::codecs::interned::query_value_to_inner;
use shamir_types::core::interner::Interner;
use shamir_types::types::common::new_map;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use crate::query::filter::FilterContext;
use crate::query::read::aggregate::{apply_aggregate_all_with_context, try_apply_group_by};
use crate::query::read::{GroupBy, Select};

fn selection(separator: &str) -> Select {
    Select {
        items: vec![select::agg_fn_with_args(
            "string_agg",
            "value",
            "joined",
            [lit(separator)],
        )],
        distinct: false,
    }
}

fn fixture(interner: &Interner, groups: &[i64]) -> Vec<(RecordId, Bytes)> {
    groups
        .iter()
        .map(|group| {
            let mut row = new_map();
            row.insert("value".into(), QueryValue::Str("é".into()));
            row.insert("group".into(), QueryValue::Int(*group));
            let value = query_value_to_inner(&QueryValue::Map(row), interner).unwrap();
            (RecordId::new(), value.to_bytes().unwrap())
        })
        .collect()
}

#[test]
fn string_agg_rejects_64_kib_separator_at_creation() {
    let interner = Interner::default();
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs);
    let error =
        apply_aggregate_all_with_context(&[], &selection(&"s".repeat(64 * 1024)), &interner, &ctx)
            .unwrap_err();
    assert!(
        matches!(&error, DbError::Validation(message) if message.contains("1024")),
        "{error:?}",
    );
}

#[test]
fn string_agg_default_context_and_byte_boundary() {
    let interner = Interner::default();
    let refs = new_map();
    let records = fixture(&interner, &[0, 0]);
    let select = selection("|");
    let ctx = FilterContext::new(&interner, &refs);
    let result = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap();
    assert_eq!(result[0]["joined"], "é|é");
    let ctx = ctx.with_max_accumulator_bytes(5);
    let result = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap();
    assert_eq!(result[0]["joined"], "é|é");
    let ctx = ctx.with_max_accumulator_bytes(4);
    let error = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap_err();
    assert!(
        matches!(&error, DbError::Validation(message)
            if message == "result_too_large: ResultBytes actual=5 max=4"),
        "{error:?}",
    );
}

#[test]
fn string_agg_caps_each_group_independently() {
    let interner = Interner::default();
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs).with_max_accumulator_bytes(5);
    let select = selection("|");
    let group = GroupBy::new(["group"]);
    let records = fixture(&interner, &[0, 0, 1, 1]);
    let result = try_apply_group_by(&records, &group, &select, &interner, &ctx).unwrap();
    assert_eq!(result.len(), 2);
    assert_eq!(result[0]["joined"], "é|é");
    assert_eq!(result[1]["joined"], "é|é");
    let records = fixture(&interner, &[0, 0, 1, 1, 1]);
    let error = try_apply_group_by(&records, &group, &select, &interner, &ctx).unwrap_err();
    assert!(
        matches!(&error, DbError::Validation(message)
            if message == "result_too_large: ResultBytes actual=8 max=5"),
        "{error:?}",
    );
}

#[test]
fn string_agg_expression_input_obeys_cap() {
    let interner = Interner::default();
    let refs = new_map();
    let records = fixture(&interner, &[0, 0]);
    let select = Select {
        items: vec![select::agg_fn_with_args_of(
            "string_agg",
            col("value"),
            "joined",
            [lit("|")],
        )],
        distinct: false,
    };
    let ctx = FilterContext::new(&interner, &refs).with_max_accumulator_bytes(4);
    let error = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap_err();
    assert!(
        matches!(&error, DbError::Validation(message)
            if message == "result_too_large: ResultBytes actual=5 max=4"),
        "{error:?}",
    );
}

#[test]
fn string_agg_preserves_empty_null_and_type_mismatch_semantics() {
    let interner = Interner::default();
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs).with_max_accumulator_bytes(16);
    let records = fixture(&interner, &[0, 0]);
    let select = selection("|");
    let empty = apply_aggregate_all_with_context(&[], &select, &interner, &ctx).unwrap();
    assert_eq!(empty[0]["joined"], "");
    let select = Select {
        items: vec![select::agg_fn_of("string_agg", null(), "joined")],
        distinct: false,
    };
    let nulls = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap();
    assert_eq!(nulls[0]["joined"], "");
    let select = Select {
        items: vec![select::agg_fn_of("string_agg", lit(1_i64), "joined")],
        distinct: false,
    };
    let error = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap_err();
    assert!(
        matches!(&error, DbError::Function(message) if message == "type_mismatch"),
        "{error:?}",
    );
    let select = Select {
        items: vec![select::agg_fn("string_agg", "group", "joined")],
        distinct: false,
    };
    let ignored = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap();
    assert_eq!(ignored[0]["joined"], "");
}

#[test]
fn string_agg_default_separator_is_capped() {
    let interner = Interner::default();
    let refs = new_map();
    let ctx = FilterContext::new(&interner, &refs).with_max_accumulator_bytes(4);
    let select = Select {
        items: vec![select::agg_fn("string_agg", "value", "joined")],
        distinct: false,
    };
    let records = fixture(&interner, &[0, 0]);
    let error = apply_aggregate_all_with_context(&records, &select, &interner, &ctx).unwrap_err();
    assert!(
        matches!(&error, DbError::Validation(message)
            if message == "result_too_large: ResultBytes actual=5 max=4"),
        "{error:?}",
    );
}
