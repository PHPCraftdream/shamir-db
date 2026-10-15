#![cfg(unix)]

use std::fs;
use std::path::Path;
use std::time::Duration;

use futures_util::StreamExt;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::tx::IsolationLevel;
use shamir_db::storage::types::RecordKey;
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{repl_scope, replication_profile, subscription};
use shamir_query_builder::doc;
use shamir_query_types::admin::{ReplDirection, ReplMode};
use shamir_tx::{event_from_wal_entry, ChangeOp, ChangelogEvent};
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
async fn fixture(root: &Path, clean: bool) -> (u64, Vec<(Vec<u8>, Vec<u8>)>, ChangelogEvent) {
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
    let put_version = mvcc
        .set_versioned_no_wait(RecordKey::from_slice(&[0, 255, 128]), payload.clone())
        .await
        .unwrap();
    let deleted = RecordKey::from_slice(&[255, 0, 1]);
    let delete_put_version = mvcc
        .set_versioned_no_wait(deleted.clone(), payload)
        .await
        .unwrap();
    let delete_version = mvcc.delete_versioned_no_wait(deleted).await.unwrap();
    assert_eq!((put_version, delete_put_version, delete_version), (1, 2, 3));
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
    assert_eq!(
        version, 4,
        "the committed transaction must be WAL version 4"
    );
    let wal = repo.repo_wal().await.unwrap();
    let recovered = wal.recover().await.unwrap();
    assert_eq!(
        recovered.len(),
        1,
        "only the final transaction WAL survives"
    );
    assert_eq!(recovered[0].commit_version, 4);
    let expected_event = event_from_wal_entry(&recovered[0])
        .expect("the genuine final WAL entry must derive its journal event");
    assert_eq!(expected_event.commit_version, 4);
    assert_eq!(expected_event.changes.len(), 1);
    assert_eq!(expected_event.changes[0].table, "rows");
    assert_eq!(expected_event.changes[0].op, ChangeOp::Put);
    assert_eq!(
        expected_event.changes[0].key.as_ref(),
        committed_id.as_bytes()
    );
    let expected_value_bytes = value.to_bytes().unwrap();
    assert_eq!(
        expected_event.changes[0].value.as_deref(),
        Some(expected_value_bytes.as_ref())
    );
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
    (version, expected, expected_event)
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
fn certificate(root: &Path) -> (Option<(u64, u64)>, u64, u64, Vec<ChangelogEvent>) {
    let rt = runtime();
    let result = rt.block_on(async {
        let source = MaintenanceOpen::open(root).await.unwrap();
        let repo = source.db.get_db("app").unwrap().get_repo("main").unwrap();
        let consistent = repo.consistent_version().await.unwrap();
        let journal = repo.read_changelog_from(1, 100).await.unwrap();
        (
            repo.certified_gap_interval().await.unwrap(),
            repo.clean_horizon().await.unwrap(),
            consistent,
            journal.events,
        )
    });
    drop(rt);
    result
}

fn exercise(clean: bool) {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime();
    let (version, expected, expected_event) = rt.block_on(fixture(dir.path(), clean));
    // Unclean control is admission/runtime teardown, NOT power-loss simulation.
    drop(rt);
    let expected_gap = if clean { None } else { Some((1, version - 1)) };
    assert_eq!(version, 4);
    let before = certificate(dir.path());
    assert_eq!(before.0, expected_gap);
    assert_eq!(before.1, version);
    assert_eq!(before.2, version);
    assert_eq!(before.3, vec![expected_event.clone()]);
    for attempt in 0..2 {
        let out = dir.path().join(format!("seed-{attempt}"));
        let rt = runtime();
        let hash = rt
            .block_on(export(dir.path(), "app", "main", &out))
            .unwrap();
        drop(rt);
        assert_artifact(&out, &hash, version, before.0, &expected);
        let after = certificate(dir.path());
        assert_eq!(after.0, expected_gap);
        assert_eq!(after.1, version);
        assert_eq!(after.2, version);
        assert_eq!(after.3, vec![expected_event.clone()]);
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
                repo.consistent_version().await.unwrap(),
                repo.read_changelog_from(1, 100).await.unwrap().events,
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

/// PR8b: the seed exporter must refuse a replication destination whose
/// subscription is quarantined (`resync_required`) — stale follower data
/// must never be served as a seed, and no partial artifact may be left
/// behind. Healthy repos pass the gate (the round-trip tests above cross
/// the same check un-quarantined and succeed).
#[test]
fn disk_export_refuses_resync_quarantined_repo() {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime();
    rt.block_on(async {
        let (version, _, _) = fixture(dir.path(), true).await;
        assert_eq!(version, 4);
        let source = ShamirDb::init(SystemStoreConfig::Fjall(
            dir.path().join("shamir_db_meta.redb"),
        ))
        .await
        .unwrap();
        // Healthy repo is not quarantined.
        assert_eq!(source.read_quarantine_reason("app", "main"), None);
        // Pull profile + ACTIVE subscription over app/main, then flip the
        // subscription to `resync_required` through the REAL persisted path
        // (mirrors quarantine_reads_tests::quarantine).
        let mut b = Batch::new();
        b.id(1);
        b.op(
            "profile",
            replication_profile("prof_seed").stream(
                repl_scope("app").repo("main").build(),
                ReplDirection::Pull,
                ReplMode::ReadOnly,
            ),
        );
        b.op(
            "sub",
            subscription("sub_seed", "tcp://peer:9000", "pub", "prof_seed"),
        );
        source
            .execute("app", &b.to_request_via_msgpack())
            .await
            .unwrap();
        source
            .mark_subscription_resync_required("sub_seed", 7, 3)
            .await
            .unwrap();
        assert!(source.read_quarantine_reason("app", "main").is_some());
    });
    drop(rt);
    // Re-open cold (like the exporter's MaintenanceOpen path) and refuse.
    let rt = runtime();
    rt.block_on(async {
        let source = MaintenanceOpen::open(dir.path()).await.unwrap();
        let out = dir.path().join("quarantined-seed");
        let err = export_open(&source.db, "app", "main", &out, 1024)
            .await
            .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("resync_required") && message.contains("app/main"),
            "unexpected refusal message: {message}"
        );
        // Fail closed: no partial artifact on disk.
        assert!(!out.try_exists().unwrap());
    });
    drop(rt);
}
