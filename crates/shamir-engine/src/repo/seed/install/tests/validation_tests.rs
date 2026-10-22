use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryRepo;

use crate::repo::{SeedImageWriter, SeedInstallState};

use super::helpers::{inventory, payload, receipt, repo};

#[tokio::test]
async fn fresh_repo_identity_epoch_zero_is_valid_receipt_evidence() {
    let source = repo(Arc::new(InMemoryRepo::new()), Vec::new());
    let identity = source.repo_identity().await.unwrap();
    assert_eq!(identity.history_epoch, 0);
    assert_eq!(source.repo_identity().await.unwrap(), identity);

    let mut evidence = receipt();
    evidence.source_identity = identity;
    evidence.validate().unwrap();

    // Commit version zero remains reserved independently of the history epoch.
    for version in [0, u64::MAX] {
        evidence.source_version = version;
        assert!(evidence.validate().is_err());
    }
}

#[tokio::test]
async fn reserved_binding_and_excess_inventory_are_refused() {
    let image = repo(Arc::new(InMemoryRepo::new()), Vec::new());
    let info = image.tx_info_store().await.unwrap();
    let tables = (0..65_537)
        .map(|index| crate::repo::SeedTable {
            name: format!("table-{index}"),
            live_records: 0,
        })
        .collect();
    assert!(SeedImageWriter::begin(image.clone(), receipt(), tables)
        .await
        .is_err());
    assert_eq!(SeedInstallState::load(&info).await.unwrap(), None);
    assert_eq!(image.table_count(), 0);
    let mut writer = SeedImageWriter::begin(image.clone(), receipt(), inventory())
        .await
        .unwrap();
    assert!(writer.binding(0, "reserved").await.is_err());
    let manager = image.repo_interner().await.unwrap();
    assert!(manager.get().await.unwrap().is_empty());
    assert!(writer.finish(&[]).await.is_err());
    assert_eq!(image.replication_bookmark().await.unwrap(), 0);
}

#[tokio::test]
async fn invalid_receipt_refuses_before_install_mutation() {
    for overflow in [false, true] {
        let image = repo(Arc::new(InMemoryRepo::new()), Vec::new());
        let info = image.tx_info_store().await.unwrap();
        let mut invalid = receipt();
        if overflow {
            invalid.source_version = u64::MAX;
        } else {
            invalid.local_baseline = 73;
        }
        assert!(SeedImageWriter::begin(image.clone(), invalid, inventory())
            .await
            .is_err());
        assert_eq!(SeedInstallState::load(&info).await.unwrap(), None);
        let gate = image.tx_gate().await.unwrap();
        assert_eq!(gate.version_allocation_high_water_mark(), 0);
    }
}

// Counterfactual: permissive remap leaves an unknown numeric id unchanged;
// accepting duplicate keys silently overwrites history while inflating counts.
#[tokio::test]
async fn missing_bindings_and_duplicate_keys_poison_attempts() {
    let image = repo(Arc::new(InMemoryRepo::new()), Vec::new());
    let mut writer = SeedImageWriter::begin(image, receipt(), inventory())
        .await
        .unwrap();
    writer.binding(700, "outer").await.unwrap();
    assert!(writer
        .row(0, Bytes::from_static(b"key"), payload())
        .await
        .is_err());
    assert!(writer.binding(900, "inner").await.is_err());
    assert!(writer.finish(&[]).await.is_err());

    let image = repo(Arc::new(InMemoryRepo::new()), Vec::new());
    let mut tables = inventory();
    tables[0].live_records = 2;
    let mut writer = SeedImageWriter::begin(image, receipt(), tables)
        .await
        .unwrap();
    writer.binding(700, "outer").await.unwrap();
    writer.binding(900, "inner").await.unwrap();
    writer
        .row(0, Bytes::from_static(b"key"), payload())
        .await
        .unwrap();
    assert!(writer
        .row(0, Bytes::from_static(b"key"), payload())
        .await
        .is_err());
    assert!(writer.finish(&[]).await.is_err());
}

#[tokio::test]
async fn conflicting_binding_and_underfilled_inventory_cannot_complete() {
    let image = repo(Arc::new(InMemoryRepo::new()), Vec::new());
    let info = image.tx_info_store().await.unwrap();
    let mut writer = SeedImageWriter::begin(image, receipt(), inventory())
        .await
        .unwrap();
    writer.binding(700, "outer").await.unwrap();
    assert!(writer.binding(900, "outer").await.is_err());
    assert!(writer.finish(&[]).await.is_err());
    assert!(SeedInstallState::require_complete(&info, true)
        .await
        .is_err());
    let image = repo(Arc::new(InMemoryRepo::new()), Vec::new());
    let writer = SeedImageWriter::begin(image, receipt(), inventory())
        .await
        .unwrap();
    assert!(writer.finish(&[]).await.is_err());
}
