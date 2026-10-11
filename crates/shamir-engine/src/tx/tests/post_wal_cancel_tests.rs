use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use shamir_tx::{IndexWriteOp, IsolationLevel};
use shamir_types::types::value::InnerValue;
use tokio::sync::Notify;

use crate::repo::{BoxRepoFactory, RepoInstance};
use crate::table::table_manager::table_token_for;
use crate::table::TableConfig;
use crate::tx::commit::{PostWalPrePublishHook, TEST_POST_WAL_PRE_PUBLISH_HOOK};

#[tokio::test]
async fn post_wal_cancel_survives_drain_truncation_and_reopen_with_pre_wal_control() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("repo");
    let hook = Arc::new(PostWalPrePublishHook {
        repo_name: "post_wal_cancel".into(),
        pre_wal: AtomicBool::new(false),
        after_counter: AtomicBool::new(false),
        fail_orphan_markers: AtomicBool::new(false),
        reached: Notify::new(),
        resume: Notify::new(),
        armed: AtomicBool::new(false),
    });
    TEST_POST_WAL_PRE_PUBLISH_HOOK
        .set(Arc::clone(&hook))
        .ok()
        .unwrap();
    let repo = RepoInstance::from_factory(
        "post_wal_cancel".into(),
        BoxRepoFactory::fjall_raw(path.clone()),
        vec![TableConfig::new("t")],
    )
    .await
    .unwrap();
    let tbl = repo.get_table("t").await.unwrap();
    let gate = repo.tx_gate().await.unwrap();
    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    let a = tbl
        .insert_tx(&InnerValue::Str("A".into()), Some(&mut tx))
        .await
        .unwrap();
    repo.commit_tx(tx).await.unwrap();
    drop(snapshot);

    hook.armed.store(true, Ordering::Release);
    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    let b = tbl
        .insert_tx(&InnerValue::Str("B".into()), Some(&mut tx))
        .await
        .unwrap();
    let posting_key = Bytes::from_static(b"post_wal_cancel_posting");
    let posting_value = Bytes::copy_from_slice(b.as_bytes());
    tx.index_write_set.push((
        table_token_for("t"),
        IndexWriteOp::SetPosting {
            key: posting_key.clone(),
            value: posting_value.clone(),
            provenance: Default::default(),
        },
    ));
    let owned = repo.clone();
    let task = tokio::spawn(async move { owned.commit_tx(tx).await });
    hook.reached.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    drop(snapshot);

    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    let c = tbl
        .insert_tx(&InnerValue::Str("C".into()), Some(&mut tx))
        .await
        .unwrap();
    let outcome = repo.commit_tx(tx).await.unwrap();
    drop(snapshot);
    repo.drainer().drain_all(&repo).await.unwrap();
    assert_eq!(gate.durable_watermark(), gate.last_committed());
    assert_eq!(gate.last_committed(), outcome.commit_version);
    assert_eq!(tbl.get(b).await.unwrap(), InnerValue::Str("B".into()));
    assert_eq!(
        tbl.info_store()
            .get(posting_key.clone().into())
            .await
            .unwrap(),
        posting_value,
    );
    assert_eq!(tbl.counter().get().await.unwrap(), 3);
    repo.drainer().drain_all(&repo).await.unwrap();
    assert_eq!(tbl.counter().get().await.unwrap(), 3);

    hook.after_counter.store(true, Ordering::Release);
    hook.armed.store(true, Ordering::Release);
    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    let after_counter = tbl
        .insert_tx(&InnerValue::Str("counter-applied".into()), Some(&mut tx))
        .await
        .unwrap();
    let owned = repo.clone();
    let task = tokio::spawn(async move { owned.commit_tx(tx).await });
    hook.reached.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    drop(snapshot);
    let orphan_version = gate.orphan_max();
    hook.fail_orphan_markers.store(true, Ordering::Release);
    assert!(repo.drainer().drain_all(&repo).await.is_err());
    assert!(gate.is_orphaned_post_wal(orphan_version));
    assert!(gate.durable_watermark() < orphan_version);
    assert_eq!(tbl.counter().get().await.unwrap(), 4);
    repo.drainer().drain_all(&repo).await.unwrap();
    assert!(!gate.is_orphaned_post_wal(orphan_version));
    assert_eq!(tbl.counter().get().await.unwrap(), 4);
    assert_eq!(
        tbl.get(after_counter).await.unwrap(),
        InnerValue::Str("counter-applied".into()),
    );
    assert_eq!(gate.durable_watermark(), gate.last_committed());

    hook.pre_wal.store(true, Ordering::Release);
    hook.armed.store(true, Ordering::Release);
    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    assert!(tbl
        .set_tx(a, &InnerValue::Str("pre-WAL".into()), Some(&mut tx))
        .await
        .unwrap());
    let owned = repo.clone();
    let task = tokio::spawn(async move { owned.commit_tx(tx).await });
    hook.reached.notified().await;
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    drop(snapshot);
    assert_eq!(tbl.get(a).await.unwrap(), InnerValue::Str("A".into()));
    assert_eq!(gate.durable_watermark(), gate.completion().watermark());

    let (mut tx, snapshot) = repo.begin_tx(IsolationLevel::Serializable).await.unwrap();
    assert!(tbl
        .set_tx(a, &InnerValue::Str("A".into()), Some(&mut tx))
        .await
        .unwrap());
    let padding = InnerValue::Str(
        "x".repeat(shamir_tunables::instance_defaults::WAL_SEGMENT_MAX_BYTES as usize + 1024),
    );
    tbl.insert_tx(&padding, Some(&mut tx)).await.unwrap();
    repo.commit_tx(tx).await.unwrap();
    drop(snapshot);
    assert_eq!(tbl.get(a).await.unwrap(), InnerValue::Str("A".into()));
    let segments_before = wal_segment_names(&path).await;
    assert!(segments_before.len() >= 2);
    repo.repo_interner().await.unwrap().persist().await.unwrap();
    repo.drainer().drain_all(&repo).await.unwrap();
    repo.flush_buffers().await.unwrap();
    assert_eq!(gate.durable_watermark(), gate.last_committed());
    let segments_after = wal_segment_names(&path).await;
    assert!(segments_after.len() < segments_before.len());
    assert!(segments_before
        .iter()
        .any(|name| !segments_after.contains(name)));
    let wal = repo.repo_wal().await.unwrap();
    assert!(!wal.recover().await.unwrap().iter().any(|entry| {
        entry
            .ops
            .iter()
            .any(|op| matches!(op, shamir_wal::WalOpV2::Put { rid, .. } if *rid == b))
    }));
    drop(wal);
    drop(gate);
    drop(tbl);
    drop(repo);

    let reopened = RepoInstance::from_factory(
        "post_wal_cancel".into(),
        BoxRepoFactory::fjall_raw(path),
        vec![TableConfig::new("t")],
    )
    .await
    .unwrap();
    let tbl = reopened.get_table("t").await.unwrap();
    for (rid, value) in [(a, "A"), (b, "B"), (c, "C")] {
        assert_eq!(tbl.get(rid).await.unwrap(), InnerValue::Str(value.into()));
    }
    assert_eq!(tbl.get(a).await.unwrap(), InnerValue::Str("A".into()));
    assert_eq!(
        tbl.info_store().get(posting_key.into()).await.unwrap(),
        posting_value,
    );
}

async fn wal_segment_names(path: &Path) -> Vec<String> {
    let mut name = path.file_name().unwrap().to_os_string();
    name.push(".shamirwal");
    let mut entries = tokio::fs::read_dir(path.with_file_name(name))
        .await
        .unwrap();
    let mut names = Vec::new();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "wal")
        {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    names
}
