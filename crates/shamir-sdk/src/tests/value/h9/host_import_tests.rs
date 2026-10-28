use crate::error::ErrorKind;
use crate::host_imports::{with_native_imports, NativeReply};
use crate::{Db, Value};
#[cfg(feature = "query-builder")]
use shamir_query_builder::query::Query;

fn packed(value: Value) -> NativeReply {
    NativeReply::Packed(1, rmp_serde::to_vec(&value).unwrap())
}

#[test]
fn rv_sdk_b5acd67d_host_imports_preserve_exact_table_values_and_filter() {
    let row = Value::Map(vec![("id".into(), Value::Int(1))]);
    let query_result = vec![row.clone()];
    let filter = Value::Map(vec![("active".into(), Value::Bool(true))]);
    let expected_filter = filter.clone();
    let expected_row = row.clone();
    let expected_query_result = query_result.clone();
    with_native_imports(
        move |operation, table, request| {
            assert_eq!(table, "items");
            match operation {
                "db_get" => {
                    assert_eq!(
                        rmp_serde::from_slice::<Value>(request).unwrap(),
                        Value::Int(1)
                    );
                    packed(expected_row.clone())
                }
                "db_insert" => packed(Value::Map(vec![
                    ("id".into(), Value::Int(2)),
                    ("name".into(), Value::Str("stored".into())),
                ])),
                "db_query" => {
                    assert_eq!(
                        rmp_serde::from_slice::<Value>(request).unwrap(),
                        expected_filter
                    );
                    packed(Value::List(expected_query_result.clone()))
                }
                _ => panic!("unexpected import: {operation}"),
            }
        },
        || {
            let table = Db::new().table("items");
            assert_eq!(table.get(Value::Int(1)).unwrap(), Some(row));
            assert_eq!(
                table.insert(Value::Map(vec![])).unwrap(),
                Value::Map(vec![
                    ("id".into(), Value::Int(2)),
                    ("name".into(), Value::Str("stored".into())),
                ])
            );
            assert_eq!(table.query(Some(filter)).unwrap(), query_result);
        },
    );
}

#[test]
fn rv_sdk_b5acd67d_host_imports_return_none_for_missing_get() {
    with_native_imports(
        |operation, table, request| {
            assert_eq!(operation, "db_get");
            assert_eq!(table, "items");
            assert_eq!(
                rmp_serde::from_slice::<Value>(request).unwrap(),
                Value::Int(404)
            );
            NativeReply::Packed(0, vec![])
        },
        || assert_eq!(Db::new().table("items").get(Value::Int(404)).unwrap(), None),
    );
}

#[test]
fn rv_sdk_6d2497ca_host_imports_classify_null_insert_and_non_list_query() {
    with_native_imports(
        |operation, _, _| {
            assert_eq!(operation, "db_insert");
            packed(Value::Null)
        },
        || {
            let error = Db::new()
                .table("items")
                .insert(Value::Map(vec![]))
                .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::User);
            assert_eq!(error.message(), "db_insert returned null");
        },
    );
    with_native_imports(
        |operation, _, _| {
            assert_eq!(operation, "db_query");
            packed(Value::Int(7))
        },
        || {
            let error = Db::new().table("items").query(None).unwrap_err();
            assert_eq!(error.kind(), ErrorKind::User);
            assert_eq!(
                error.message(),
                "db_query expected list, got unexpected value"
            );
        },
    );
}

#[test]
fn rv_sdk_6d2497ca_host_imports_classify_missing_insert_and_query_results() {
    with_native_imports(
        |operation, _, _| {
            assert_eq!(operation, "db_insert");
            NativeReply::Packed(0, vec![])
        },
        || {
            assert_eq!(
                Db::new()
                    .table("items")
                    .insert(Value::Map(vec![]))
                    .unwrap_err()
                    .kind(),
                ErrorKind::Host
            )
        },
    );
    with_native_imports(
        |operation, _, _| {
            assert_eq!(operation, "db_query");
            NativeReply::Packed(0, vec![])
        },
        || {
            assert_eq!(
                Db::new().table("items").query(None).unwrap_err().kind(),
                ErrorKind::Host
            )
        },
    );
}

#[cfg(feature = "query-builder")]
#[test]
fn rv_sdk_6d2497ca_execute_classifies_malformed_and_empty_host_responses() {
    let mut batch = shamir_query_builder::batch::Batch::new();
    batch.id("q");
    batch.query("rows", Query::from("items"));
    with_native_imports(
        |operation, _, _| {
            assert_eq!(operation, "db_execute");
            NativeReply::Raw(vec![0x81, 0xa1])
        },
        || {
            assert_eq!(
                Db::new().execute(&batch).unwrap_err().kind(),
                ErrorKind::Codec
            )
        },
    );
    with_native_imports(
        |operation, _, _| {
            assert_eq!(operation, "db_execute");
            NativeReply::Raw(vec![])
        },
        || {
            assert_eq!(
                Db::new().execute(&batch).unwrap_err().kind(),
                ErrorKind::Host
            )
        },
    );
}
