use crate::repo::{BoxRepoFactory, RepoFactory, RepoInstance, SeedInstallState, SeedTable};

use super::helpers::{populated, receipt};

// Unix Store::flush cold reopen evidence, not a physical power-loss claim.
// The independent raw factory bypasses caches and a new RepoInstance rebuilds
// gate, counters and interner from the persisted stores.
#[tokio::test]
async fn disk_cold_reopen_preserves_receipt_counts_bindings_and_local_floor() {
    let directory = tempfile::tempdir().unwrap();
    let factory = BoxRepoFactory::fjall_raw(directory.path());
    let backing = factory.create().await.unwrap();
    let image = RepoInstance::new("main".into(), backing.clone(), Vec::new());
    populated(image.clone()).await.finish(&[]).await.unwrap();
    let info = image.tx_info_store().await.unwrap();
    let tables = SeedTable::load_inventory(&info).await.unwrap();
    // No background disk WAL is constructed by RepoInstance::new. The journal
    // worker must release its store before dropping the last backend owner.
    image
        .changefeed()
        .await
        .unwrap()
        .feed
        .shutdown_and_wait()
        .await
        .unwrap();
    drop(info);
    drop(image);
    drop(backing);
    drop(factory);
    let factory = BoxRepoFactory::fjall_raw(directory.path());
    let cold = RepoInstance::new(
        "main".into(),
        factory.create().await.unwrap(),
        tables.iter().map(SeedTable::config).collect(),
    );
    let info = cold.tx_info_store().await.unwrap();
    assert_eq!(
        SeedInstallState::require_complete(&info, true)
            .await
            .unwrap(),
        Some(receipt()),
    );
    assert_eq!(cold.replication_bookmark().await.unwrap(), 73);
    let items = cold.get_table("items").await.unwrap();
    let empty = cold.get_table("empty").await.unwrap();
    assert_eq!(items.counter().get().await.unwrap(), 1);
    assert_eq!(empty.counter().get().await.unwrap(), 0);
    let manager = cold.repo_interner().await.unwrap();
    let interner = manager.get().await.unwrap();
    assert!(interner.get_ind("outer").is_some());
    assert!(interner.get_ind("inner").is_some());
    let row = cold
        .get_table("items")
        .await
        .unwrap()
        .mvcc_store()
        .unwrap()
        .get_at(b"opaque-key", 1)
        .await
        .unwrap();
    assert!(row.is_some());
    let gate = cold.tx_gate().await.unwrap();
    assert_eq!((gate.visible_floor(), gate.durable_watermark()), (1, 1));
    assert_eq!(gate.assign_next_version_guarded().version(), 2);
    assert_eq!(
        cold.read_changelog_from(1, 8).await.unwrap().gap_at,
        Some(1)
    );
    cold.changefeed()
        .await
        .unwrap()
        .feed
        .shutdown_and_wait()
        .await
        .unwrap();
}
