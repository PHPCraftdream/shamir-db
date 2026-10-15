//! Cold-start recovery of an outstanding replication receipt (PR6b-2): a
//! repo that crashed after the durable Prepared phase completes the receipt
//! during the next open's cold reconcile — the versions are certified
//! through the bookmark's clean horizon, so no recovery GAP is declared over
//! them. FAILS on pre-PR6b-2-cold code: the receipt stayed outstanding
//! (fail-stopped open) and/or the batch's versions were declared a GAP.

use std::path::Path;
use std::sync::Arc;

use futures::StreamExt;
use shamir_storage::types::{RecordKey, Store};
use shamir_tx::{decode_version_key, encode_version_key, ChangeOp, ChangelogEvent, RecordChange};
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;

use crate::meta::repl::bookmark_v3::load_bookmark_v3;
use crate::meta::repl::receipt::receipt_key;
use crate::repo::{BoxRepoFactory, RepoInstance};
use crate::table::TableConfig;
use crate::tx::apply_replicated::follower_durability::test_seam::{self, Phase};
use crate::tx::apply_replicated::{apply_replicated_batch, ApplyPolicy};
use crate::tx::ReplicatedEvent;

fn event(leader_version: u64, id: u8) -> ReplicatedEvent {
    ReplicatedEvent {
        event: ChangelogEvent {
            repo: "leader".into(),
            commit_version: leader_version,
            tx_id: leader_version,
            actor: Actor::System,
            timestamp_ns: 0,
            changes: vec![RecordChange {
                table: "items".into(),
                key: RecordId([id; 16]).to_bytes(),
                op: ChangeOp::Put,
                value: Some(InnerValue::Str(format!("row-{id}")).to_bytes().unwrap()),
            }],
        },
        field_bindings: Vec::new(),
    }
}

async fn open(path: &Path, name: &str) -> RepoInstance {
    RepoInstance::from_factory(
        name.into(),
        BoxRepoFactory::fjall_raw(path),
        vec![TableConfig::new("items")],
    )
    .await
    .unwrap()
}

async fn close(repo: RepoInstance) {
    let handle = repo.changefeed().await.unwrap();
    handle.feed.shutdown_and_wait().await.unwrap();
    drop(handle);
    drop(repo);
}

fn repo_name(tag: &str, path: &Path) -> String {
    format!("{tag}-{}", path.display().to_string().replace('\\', "_"))
}

async fn assert_completed_state(repo: &RepoInstance) {
    let info = repo.tx_info_store().await.unwrap();
    // Receipt retired, bookmark v3 advanced with the receipt's clean horizon.
    assert!(matches!(
        info.get(receipt_key()).await,
        Err(shamir_storage::error::DbError::NotFound(_))
    ));
    let v3 = load_bookmark_v3(&info).await.unwrap().unwrap();
    assert_eq!(v3.outstanding_receipt, None);
    assert_eq!(v3.bookmark, 1, "bookmark == upstream_end");
    assert_eq!(v3.clean_through, Some(1));
    drop(info);

    // Journal + history rows landed.
    let journal_bytes = repo
        .follower_journal_store()
        .await
        .unwrap()
        .get(RecordKey::from_slice(&1u64.to_be_bytes()))
        .await
        .expect("journal row at local version 1");
    assert!(!journal_bytes.is_empty());
    let history = repo.raw_store("__history__items").await.unwrap();
    let key = RecordId([1; 16]).to_bytes();
    let row = history
        .get(RecordKey::from_slice(&encode_version_key(key.as_ref(), 1)))
        .await
        .expect("history row at local version 1");
    assert!(!row.is_empty());
    assert_eq!(
        decode_version_key(&encode_version_key(key.as_ref(), 1)),
        Some((key.as_ref(), 1))
    );

    // No recovery GAP covers the completed receipt's versions.
    let feed = repo.changefeed().await.unwrap().feed;
    assert_eq!(
        feed.certified_gap_interval(),
        None,
        "the completed receipt's version must not be declared a gap"
    );
    drop(feed);
}

// Crash after the durable Prepared phase, close, reopen: the cold reconcile
// completes the outstanding receipt (history + journal + counters + bookmark
// advance + retirement) BEFORE classification, so no gap is declared and a
// second reopen observes the identical settled state.
#[tokio::test(flavor = "current_thread")]
async fn cold_reopen_completes_outstanding_receipt() {
    let temp = tempfile::TempDir::new().unwrap();
    let name = repo_name("receipt-cold", temp.path());
    let repo = open(temp.path(), &name).await;

    let hook = Arc::new(test_seam::Hook::default());
    *hook.park_at.lock().unwrap() = Some(Phase::ReceiptPrepared);
    test_seam::install(&name, Arc::clone(&hook));
    let events = [event(1, 1)];
    let mut apply = Box::pin(apply_replicated_batch(
        &repo,
        &events,
        0,
        ApplyPolicy::Trusted,
    ));
    let reached = hook.reached.notified();
    tokio::pin!(reached);
    reached.as_mut().enable();
    tokio::select! {
        _ = &mut reached => {}
        result = &mut apply => {
            panic!("apply completed before ReceiptPrepared: {result:?}")
        }
    }
    drop(apply);
    hook.resume.notify_waiters();
    test_seam::remove(&name);
    close(repo).await;

    for _ in 0..2 {
        let reopened = open(temp.path(), &name).await;
        assert_completed_state(&reopened).await;
        close(reopened).await;
    }
}
