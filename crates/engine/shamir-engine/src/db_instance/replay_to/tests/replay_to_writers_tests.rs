//! Ordinary boot is still permissive; this does not exercise strict apply.

use std::fs;

use bytes::Bytes;
use shamir_types::types::record_id::RecordId;
use shamir_wal::WalOpV2;
use tempfile::TempDir;
use tokio::time::timeout;

use crate::meta::recovery_marker::load_last_committed;
use crate::repo::{BoxRepoFactory, RepoInstance};

use super::replay_to_tests::{entry, write_wal, BOUND};

#[tokio::test]
async fn ordinary_boot_skips_put_with_unresolved_table_token() {
    let dir = TempDir::new().unwrap();
    let store = dir.path().join("data");
    fs::create_dir(&store).unwrap();
    let wal = store.with_file_name("data.shamirwal");
    let put = entry(
        600,
        1,
        WalOpV2::Put {
            table_id_interned: 987_654,
            rid: RecordId([4; 16]),
            body: Bytes::from_static(b"unresolved put"),
        },
    );
    write_wal(&wal, &[put]).await;

    // No tables or interner mappings exist in this fresh repository. Boot
    // must skip the unresolved Put and still finalize the WAL version.
    let repo = timeout(
        BOUND,
        Box::pin(RepoInstance::from_factory(
            "data".into(),
            BoxRepoFactory::fjall_raw(store),
            vec![],
        )),
    )
    .await
    .expect("ordinary boot timed out")
    .expect("ordinary boot should tolerate the unresolved table token");
    let info = timeout(BOUND, repo.tx_info_store())
        .await
        .expect("tx info store open timed out")
        .unwrap();
    let floor = timeout(BOUND, load_last_committed(&info))
        .await
        .expect("recovery floor read timed out")
        .unwrap();
    assert_eq!(floor, Some(1), "boot did not finalize the WAL entry");
}
