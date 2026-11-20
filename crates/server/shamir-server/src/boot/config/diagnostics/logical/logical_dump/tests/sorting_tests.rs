use std::fs;

use shamir_sync::replication_seed::{LogicalValue, PortableItem, RecordItem};
use tokio_util::sync::CancellationToken;

use num_bigint::BigInt;
use rust_decimal::Decimal;
use shamir_types::types::common::{new_map, TSet};
use shamir_types::types::value::QueryValue;

use super::super::projection::record_value;
use super::super::sorting::{next_item, Sorter};
#[cfg(any(target_os = "linux", target_vendor = "apple", windows))]
use super::super::staging::publish_live_stage;
use super::super::staging::Stage;

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

#[cfg(any(target_os = "linux", target_vendor = "apple", windows))]
#[test]
fn live_publication_renames_without_replacing() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("dump");
    let stage = Stage::for_live(&output).unwrap();
    let source = stage.path().join("dump");
    fs::write(&source, b"verified").unwrap();

    // Simulate a destination appearing after stage creation.
    fs::write(&output, b"racing owner").unwrap();
    let error = publish_live_stage(stage.path(), &output, || Ok(())).unwrap_err();
    assert_eq!(
        error.downcast_ref::<std::io::Error>().unwrap().kind(),
        std::io::ErrorKind::AlreadyExists
    );
    assert!(Stage::for_live(&output).is_err());
    assert_eq!(fs::read(&output).unwrap(), b"racing owner");
    assert_eq!(fs::read(&source).unwrap(), b"verified");

    let fresh_output = root.path().join("fresh");
    publish_live_stage(stage.path(), &fresh_output, || Ok(())).unwrap();
    assert!(!source.try_exists().unwrap());
    let stage_path = stage.path().to_owned();
    drop(stage);
    assert!(!stage_path.try_exists().unwrap());
    assert_eq!(fs::read(fresh_output).unwrap(), b"verified");
}

#[cfg(any(target_os = "linux", target_vendor = "apple", windows))]
#[test]
fn live_publication_callback_can_abort_before_rename() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("dump");
    let stage = Stage::for_live(&output).unwrap();
    let source = stage.path().join("dump");
    fs::write(&source, b"verified").unwrap();

    let error = publish_live_stage(stage.path(), &output, || {
        anyhow::bail!("publication vetoed")
    })
    .unwrap_err();
    assert_eq!(error.to_string(), "publication vetoed");
    assert_eq!(fs::read(&source).unwrap(), b"verified");
    assert!(!output.try_exists().unwrap());
}

#[test]
fn stored_strings_and_lists_convert_recursively_but_ambiguous_kinds_refuse() {
    let mut outer = new_map();
    outer.insert("text".to_string(), QueryValue::Str("stored".into()));
    let mut inner = new_map();
    inner.insert("inner".to_string(), QueryValue::Bool(true));
    outer.insert(
        "list".to_string(),
        QueryValue::List(vec![
            QueryValue::Int(1),
            QueryValue::Str("nested".into()),
            QueryValue::Map(inner),
        ]),
    );
    let value = record_value(&QueryValue::Map(outer)).unwrap();
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
    let mut outer = new_map();
    outer.insert("deep".to_string(), deep);
    assert!(record_value(&QueryValue::Map(outer)).is_err());
    let mut outer = new_map();
    outer.insert("bin".to_string(), QueryValue::Bin(vec![7; 512 * 1024 + 1]));
    assert!(record_value(&QueryValue::Map(outer)).is_err());
}
