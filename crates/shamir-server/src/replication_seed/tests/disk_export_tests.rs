#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::tx::IsolationLevel;
use shamir_db::storage::types::RecordKey;
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_query_builder::doc;
use shamir_types::codecs::interned::query_value_to_inner;
use tokio::runtime::{Builder, Runtime};

use crate::replication_seed::codec::{decode, digest};
use crate::replication_seed::exporter::export_open;
use crate::replication_seed::maintenance_open::MaintenanceOpen;
use crate::replication_seed::{export, verify, Manifest};

fn runtime() -> Runtime {
    Builder::new_current_thread().enable_all().build().unwrap()
}

// Real server-layout catalogue and raw Fjall data; runtime teardown releases
// repository-internal owners deterministically, without sleeps or lock retries.
async fn fixture(root: &Path, clean: bool) -> (u64, Vec<(Vec<u8>, Vec<u8>)>) {
    let meta = root.join("shamir_db_meta.redb");
    let source = ShamirDb::init(SystemStoreConfig::Fjall(meta))
        .await
        .unwrap();
    source.create_db("app").await;
    source
        .add_repo(
            "app",
            RepoConfig::new("main", BoxRepoFactory::fjall_raw(root.join("data"))),
        )
        .await
        .unwrap();
    source
        .add_table("app", "main", "rows", false)
        .await
        .unwrap();
    source
        .add_table("app", "main", "empty", false)
        .await
        .unwrap();
    let repo = source.get_db("app").unwrap().get_repo("main").unwrap();
    let table = repo.get_table("rows").await.unwrap();
    let interner = table.interner().get().await.unwrap();
    let value = query_value_to_inner(&doc! { "name" => "committed" }.into(), interner).unwrap();
    let payload = value.to_bytes().unwrap();
    let mvcc = table.mvcc_store().unwrap();
    mvcc.set_versioned_no_wait(RecordKey::from_slice(&[0, 255, 128]), payload.clone())
        .await
        .unwrap();
    let deleted = RecordKey::from_slice(&[255, 0, 1]);
    mvcc.set_versioned_no_wait(deleted.clone(), payload)
        .await
        .unwrap();
    mvcc.delete_versioned_no_wait(deleted).await.unwrap();
    // Stage a genuine transaction and abort by the documented RAII contract.
    let aborted_value =
        query_value_to_inner(&doc! { "name" => "aborted" }.into(), interner).unwrap();
    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let aborted_id = table
        .insert_tx(&aborted_value, Some(&mut tx))
        .await
        .unwrap();
    drop(tx);
    drop(snapshot);
    // Commit last so the durable recovery marker covers the raw MVCC writes.
    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
    let committed_id = table.insert_tx(&value, Some(&mut tx)).await.unwrap();
    repo.commit_tx(tx).await.unwrap();
    drop(snapshot);
    repo.drainer().drain_all(&repo).await.unwrap();
    repo.flush_all_history().await.unwrap();
    repo.flush_buffers().await.unwrap();
    repo.flush_seed_metadata().await.unwrap();
    source.flush_seed_catalogue().await.unwrap();
    let version = repo.consistent_version().await.unwrap();
    let stream = mvcc.snapshot_stream(1, version);
    futures_util::pin_mut!(stream);
    let mut expected = Vec::new();
    while let Some(batch) = stream.next().await {
        for (key, value) in batch.unwrap() {
            expected.push((key.to_vec(), value.to_vec()));
        }
    }
    assert_eq!(expected.len(), 2);
    assert!(expected.iter().any(|(key, _)| key == &[0, 255, 128]));
    assert!(expected
        .iter()
        .any(|(key, _)| key.as_slice() == committed_id.as_bytes()));
    assert!(!expected
        .iter()
        .any(|(key, _)| key.as_slice() == aborted_id.as_bytes()));
    assert!(!expected.iter().any(|(key, _)| key == &[255, 0, 1]));
    if clean {
        repo.shutdown_clean(Duration::from_secs(30)).await.unwrap();
        let before = (
            repo.certified_gap_interval().await.unwrap(),
            repo.clean_horizon().await.unwrap(),
        );
        assert_eq!(before, (None, version));
        // flush_all checkpoints the target again as well as the system repo.
        // With stopped admission this is idempotent, not another allocation.
        source.flush_all().await.unwrap();
        assert_eq!(repo.consistent_version().await.unwrap(), version);
        assert_eq!(
            before,
            (
                repo.certified_gap_interval().await.unwrap(),
                repo.clean_horizon().await.unwrap(),
            ),
        );
    }
    (version, expected)
}

fn assert_artifact(
    out: &Path,
    hash: &str,
    version: u64,
    gap: Option<(u64, u64)>,
    expected: &[(Vec<u8>, Vec<u8>)],
) {
    assert_eq!(verify(out).unwrap(), hash);
    let bytes = fs::read(out.join("manifest.mpack")).unwrap();
    assert_eq!(digest(&bytes), hash);
    let manifest: Manifest = decode(&bytes).unwrap();
    assert_eq!(manifest.version, version);
    assert_eq!(manifest.certified_gap, gap);
    assert_eq!(manifest.tables[0].name, "empty");
    assert_eq!(manifest.tables[0].live_records, 0);
    assert_eq!(manifest.tables[1].name, "rows");
    assert_eq!(manifest.tables[1].live_records, 2);
    let mut rows = Vec::new();
    for file in &manifest.files {
        let bytes = fs::read(out.join(&file.path)).unwrap();
        assert_eq!(bytes.len() as u64, file.bytes);
        assert_eq!(digest(&bytes), file.sha256);
        if file.table.is_some() {
            let mut offset = 0;
            while offset < bytes.len() {
                let prefix = bytes[offset..offset + 4].try_into().unwrap();
                let len = u32::from_be_bytes(prefix) as usize;
                offset += 4;
                let (key, value): (serde_bytes::ByteBuf, serde_bytes::ByteBuf) =
                    decode(&bytes[offset..offset + len]).unwrap();
                rows.push((key.into_vec(), value.into_vec()));
                offset += len;
            }
        }
    }
    assert_eq!(rows, expected);
}

// Independently cold-open the certificate, then tear down ALL internal owners
// before the next public export. No live database handle spans runtime drops.
fn certificate(root: &Path) -> (Option<(u64, u64)>, u64) {
    let rt = runtime();
    let result = rt.block_on(async {
        let source = MaintenanceOpen::open(root).await.unwrap();
        let repo = source.db.get_db("app").unwrap().get_repo("main").unwrap();
        (
            repo.certified_gap_interval().await.unwrap(),
            repo.clean_horizon().await.unwrap(),
        )
    });
    drop(rt);
    result
}

fn exercise(clean: bool) {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime();
    let (version, expected) = rt.block_on(fixture(dir.path(), clean));
    // Unclean control is admission/runtime teardown, NOT power-loss simulation.
    drop(rt);
    let before = certificate(dir.path());
    let expected_gap = if clean { None } else { Some((1, version)) };
    assert_eq!(before, (expected_gap, version));
    for attempt in 0..2 {
        let out = dir.path().join(format!("seed-{attempt}"));
        let rt = runtime();
        let hash = rt
            .block_on(export(dir.path(), "app", "main", &out))
            .unwrap();
        drop(rt);
        assert_artifact(&out, &hash, version, before.0, &expected);
        assert_eq!(
            certificate(dir.path()),
            before,
            "public export must preserve existing gap AND J across cold reopen",
        );
    }
    // Keep direct MaintenanceOpen + bounded helper coverage in addition to the
    // public path. This also checks the certificate while the owner is held.
    let rt = runtime();
    rt.block_on(async {
        let source = MaintenanceOpen::open(dir.path()).await.unwrap();
        let out = dir.path().join("helper-seed");
        let hash = export_open(&source.db, "app", "main", &out, 1024)
            .await
            .unwrap();
        assert_artifact(&out, &hash, version, before.0, &expected);
        let repo = source.db.get_db("app").unwrap().get_repo("main").unwrap();
        assert_eq!(
            (
                repo.certified_gap_interval().await.unwrap(),
                repo.clean_horizon().await.unwrap(),
            ),
            before,
        );
    });
    drop(rt);
}

// Red with unsupported MaintenanceOpen; catches skipped recovery/deletion/abort.
#[test]
fn clean_disk_export_round_trip_twice_preserves_j_and_gap() {
    exercise(true);
}

#[test]
fn unclean_disk_export_certifies_and_twice_preserves_j_and_gap() {
    exercise(false);
}
