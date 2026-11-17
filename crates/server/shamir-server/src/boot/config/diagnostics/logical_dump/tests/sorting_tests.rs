use std::fs;

use shamir_sync::replication_seed::{LogicalValue, PortableItem, RecordItem};
use tokio_util::sync::CancellationToken;

use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_types::types::common::TSet;
use shamir_types::types::value::QueryValue;

use super::super::projection::record_value;
use super::super::sorting::{next_item, Sorter};
use super::super::staging::Stage;
use shamir_query_builder::doc;

fn row(key: Vec<u8>) -> PortableItem {
    PortableItem::Record(RecordItem {
        repository: "main".into(),
        table: "rows".into(),
        raw_key: key,
        value: LogicalValue::Bin(vec![7; 64 * 1024]),
    })
}

#[tokio::test]
async fn multi_run_merge_orders_raw_bytes_and_rejects_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let mut sorter = Sorter::new(dir.path());
    for i in (0_u16..180).rev() {
        let key = i.to_be_bytes().to_vec();
        sorter.push(key.clone(), &row(key), &cancel).await.unwrap();
    }
    let mut sorted = sorter.finish(&cancel).await.unwrap().unwrap();
    for i in 0_u16..180 {
        let PortableItem::Record(item) = next_item(&mut sorted).unwrap().unwrap() else {
            panic!("not a row");
        };
        assert_eq!(item.raw_key, i.to_be_bytes());
    }
    assert!(next_item(&mut sorted).unwrap().is_none());
    let dir = tempfile::tempdir().unwrap();
    let mut sorter = Sorter::new(dir.path());
    sorter.push(vec![0], &row(vec![0]), &cancel).await.unwrap();
    sorter.push(vec![0], &row(vec![0]), &cancel).await.unwrap();
    assert!(sorter
        .finish(&cancel)
        .await
        .unwrap_err()
        .to_string()
        .contains("duplicate"));
}

#[tokio::test]
async fn cancellation_during_sort_cleans_owned_stage() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("dump");
    let stage = Stage::new(&output).unwrap();
    let path = stage.path().to_owned();
    let cancel = CancellationToken::new();
    let mut sorter = Sorter::new(stage.path());
    sorter.push(vec![0], &row(vec![0]), &cancel).await.unwrap();
    cancel.cancel();
    assert!(sorter
        .finish(&cancel)
        .await
        .unwrap_err()
        .to_string()
        .contains("cancelled"));
    drop(stage);
    assert!(!path.exists());
    assert!(!output.exists());
}

#[test]
fn publish_conflict_never_replaces_racing_destination() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("dump");
    let stage = Stage::new(&output).unwrap();
    fs::write(stage.path().join("dump"), b"verified").unwrap();
    fs::write(&output, b"racing owner").unwrap();
    assert!(stage.publish(&output).is_err());
    assert_eq!(fs::read(output).unwrap(), b"racing owner");
}

#[test]
fn stored_strings_and_lists_convert_recursively_but_ambiguous_kinds_refuse() {
    let value = record_value(
        &doc! { "text" => "stored", "list" => QueryValue::List(vec![
            QueryValue::Int(1),
            QueryValue::Str("nested".into()),
            doc! { "inner" => true }.build(),
        ])}
        .into(),
    )
    .unwrap();
    assert_eq!(
        value,
        LogicalValue::Map(vec![
            ("text".into(), LogicalValue::Str("stored".into())),
            (
                "list".into(),
                LogicalValue::List(vec![
                    LogicalValue::Int(1),
                    LogicalValue::Str("nested".into()),
                    LogicalValue::Map(vec![("inner".into(), LogicalValue::Bool(true))]),
                ])
            ),
        ])
    );
    let mut set = TSet::default();
    set.insert(QueryValue::Int(1));
    assert!(record_value(&QueryValue::Set(set)).is_err());
    assert!(record_value(&QueryValue::Dec(Decimal::from(1))).is_err());
    assert!(record_value(&QueryValue::Big(BigInt::from(1))).is_err());
}

#[test]
fn record_budget_still_refuses_deep_or_oversized_stored_values() {
    let mut deep = QueryValue::Int(0);
    for _ in 0..64 {
        deep = QueryValue::List(vec![deep]);
    }
    assert!(record_value(&doc! { "deep" => deep }.build()).is_err());
    assert!(
        record_value(&doc! { "bin" => QueryValue::Bin(vec![7; 512 * 1024 + 1]) }.build()).is_err()
    );
}
