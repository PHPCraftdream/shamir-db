use std::fs;

use shamir_sync::replication_seed::{LogicalValue, PortableItem, RecordItem};
use tokio_util::sync::CancellationToken;

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
fn ambiguous_nested_values_refuse_instead_of_coercing() {
    assert!(record_value(&doc! { "nested" => "text" }.into()).is_err());
    let value = record_value(&doc! { "integer" => 42_i64, "boolean" => true }.into()).unwrap();
    assert_eq!(
        value,
        LogicalValue::Map(vec![
            ("integer".into(), LogicalValue::Int(42)),
            ("boolean".into(), LogicalValue::Bool(true))
        ])
    );
}
