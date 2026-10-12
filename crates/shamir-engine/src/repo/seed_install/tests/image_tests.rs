use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryRepo;
use shamir_storage::types::Repo;
use shamir_types::types::value::InnerValue;

use crate::meta::recovery_marker::load_next_tx_id_snapshot;
use crate::repo::{SeedImageWriter, SeedInstallState, SeedTable};

use super::helpers::{inventory, payload, populated, receipt, repo};

// Counterfactual: Complete before bookmark/data barriers admits any injected
// pre-completion attempt; failure to checkpoint 1 produces no downstream gap.
#[tokio::test]
async fn interruptions_refuse_partial_image_and_fresh_attempt_converges() {
    for boundary in 0..=6 {
        let backing = Arc::new(InMemoryRepo::new());
        let image = repo(backing, Vec::new());
        let info = image.tx_info_store().await.unwrap();
        let mut writer = if boundary == 0 {
            let mut writer = SeedImageWriter::begin(image.clone(), receipt(), inventory())
                .await
                .unwrap();
            writer.binding(700, "outer").await.unwrap();
            writer.binding(900, "inner").await.unwrap();
            writer
        } else {
            populated(image.clone()).await
        };
        writer.fail_at = Some(boundary);
        if boundary == 0 {
            assert!(writer
                .row(0, Bytes::from_static(b"key"), payload())
                .await
                .is_err());
            assert!(writer.finish(&[]).await.is_err());
        } else {
            assert!(writer.finish(&[]).await.is_err());
        }
        let state = SeedInstallState::load(&info).await.unwrap();
        if boundary < 6 {
            assert!(matches!(state, Some(SeedInstallState::Installing(_))));
            assert!(SeedInstallState::require_complete(&info, true)
                .await
                .is_err());
            assert!(SeedInstallState::completed_retry(&info, &receipt())
                .await
                .is_err());
        } else {
            assert!(matches!(state, Some(SeedInstallState::Complete(_))));
            assert!(SeedInstallState::completed_retry(&info, &receipt())
                .await
                .unwrap());
        }
        assert_eq!(
            image.replication_bookmark().await.unwrap(),
            if boundary < 5 { 0 } else { 73 },
        );
        // Never resume the abandoned image. Exact artifact on fresh staging.
        let fresh = repo(Arc::new(InMemoryRepo::new()), Vec::new());
        assert_eq!(
            populated(fresh.clone()).await.finish(&[]).await.unwrap(),
            receipt(),
        );
        assert_eq!(fresh.replication_bookmark().await.unwrap(), 73);
    }
}

// Counterfactual: copying V to the local floor yields 74 instead of 2; omitting
// inventory/counter/interner persistence loses names or counts on cold handles.
#[tokio::test]
async fn cold_handles_read_remapped_rows_counters_and_bootstrap_gap() {
    let backing = Arc::new(InMemoryRepo::new());
    let image = repo(backing.clone(), Vec::new());
    populated(image.clone()).await.finish(&[]).await.unwrap();
    let local_identity = image.repo_identity().await.unwrap();
    assert_ne!(
        local_identity.lineage_id,
        receipt().source_identity.lineage_id
    );
    let info = image.tx_info_store().await.unwrap();
    let tables = SeedTable::load_inventory(&info).await.unwrap();
    assert_eq!(tables, inventory());
    let cold = repo(backing.clone(), tables);
    let table = cold.get_table("items").await.unwrap();
    assert_eq!(table.counter().get().await.unwrap(), 1);
    let empty = cold.get_table("empty").await.unwrap();
    assert_eq!(empty.counter().get().await.unwrap(), 0);
    let manager = cold.repo_interner().await.unwrap();
    let interner = manager.get().await.unwrap();
    let outer = interner.get_ind("outer").unwrap();
    let inner = interner.get_ind("inner").unwrap();
    assert_ne!(outer.id(), 700);
    assert_ne!(inner.id(), 900);
    let bytes = table
        .mvcc_store()
        .unwrap()
        .get_at(b"opaque-key", 1)
        .await
        .unwrap()
        .unwrap();
    let row = InnerValue::from_bytes(bytes).unwrap();
    let InnerValue::Map(root) = row else {
        panic!("not a record");
    };
    let InnerValue::List(list) = root.get(&outer).unwrap() else {
        panic!("not a list");
    };
    let InnerValue::Map(nested) = &list[0] else {
        panic!("not nested map");
    };
    assert_eq!(nested.get(&inner), Some(&InnerValue::Bin(vec![0, 255])));
    let gate = cold.tx_gate().await.unwrap();
    assert_eq!(
        (
            gate.last_committed(),
            gate.visible_floor(),
            gate.durable_watermark()
        ),
        (1, 1, 1),
    );
    assert_eq!(load_next_tx_id_snapshot(&info).await.unwrap(), Some(1));
    assert_eq!(gate.assign_next_version_guarded().version(), 2);
    let changelog = backing.store_get("__changelog__").await.unwrap();
    let clean = changelog
        .get(shamir_tx::changefeed::clean_horizon_key().into())
        .await
        .unwrap();
    assert_eq!(
        shamir_tx::changefeed::decode_clean_horizon(&clean).unwrap(),
        1
    );
    assert_eq!(
        cold.read_changelog_from(1, 10).await.unwrap().gap_at,
        Some(1)
    );
}

#[tokio::test]
async fn exact_retry_and_ordinary_repository_admission_are_distinct() {
    let image = repo(Arc::new(InMemoryRepo::new()), Vec::new());
    let info = image.tx_info_store().await.unwrap();
    assert_eq!(
        SeedInstallState::require_complete(&info, false)
            .await
            .unwrap(),
        None,
    );
    assert!(SeedInstallState::require_complete(&info, true)
        .await
        .is_err());
    populated(image.clone()).await.finish(&[]).await.unwrap();
    assert!(SeedInstallState::completed_retry(&info, &receipt())
        .await
        .unwrap());
    let items = image.get_table("items").await.unwrap();
    assert_eq!(items.counter().get().await.unwrap(), 1);
    let mut other = receipt();
    other.artifact_digest = "c".repeat(64);
    assert!(SeedInstallState::completed_retry(&info, &other)
        .await
        .is_err());
    assert!(SeedImageWriter::begin(image, receipt(), inventory())
        .await
        .is_err());
    let ordinary = repo(Arc::new(InMemoryRepo::new()), inventory());
    assert!(SeedImageWriter::begin(ordinary, receipt(), inventory())
        .await
        .is_err());
    let bookmarked = repo(Arc::new(InMemoryRepo::new()), Vec::new());
    bookmarked.advance_replication_bookmark(8).await.unwrap();
    assert!(SeedImageWriter::begin(bookmarked, receipt(), inventory())
        .await
        .is_err());
}
