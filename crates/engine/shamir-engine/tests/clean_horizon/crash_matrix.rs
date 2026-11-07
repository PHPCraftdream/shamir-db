use std::path::Path;

use shamir_engine::meta::recovery_marker::load_last_committed;
use shamir_engine::repo::{BoxRepoFactory, RepoFactory};
use shamir_storage::types::Repo as _;

pub async fn raw_checkpoint(path: &Path) -> (u64, u64) {
    let factory = BoxRepoFactory::fjall_raw(path.to_path_buf());
    let raw = factory.create().await.expect("open raw repo");
    let changelog = raw
        .store_get("__changelog__")
        .await
        .expect("changelog store");
    let journal = match changelog
        .get(shamir_tx::changefeed::clean_horizon_key().into())
        .await
    {
        Ok(bytes) => shamir_tx::changefeed::decode_clean_horizon(&bytes).expect("decode J"),
        Err(shamir_storage::error::DbError::NotFound(_)) => 0,
        Err(error) => panic!("read J: {error}"),
    };
    let tx = raw.store_get("__tx__").await.expect("tx store");
    let floor = load_last_committed(&tx)
        .await
        .expect("read recovery floor")
        .unwrap_or(0);
    drop(changelog);
    drop(tx);
    drop(raw);
    (journal, floor)
}

#[tokio::test]
async fn raw_checkpoint_reader_reports_missing_j_and_floor() {
    let temp = tempfile::tempdir().expect("tempdir");
    let path = temp.path().join("raw_contract");
    let (journal, floor) = raw_checkpoint(&path).await;
    assert_eq!(journal, 0);
    assert_eq!(floor, 0);
}
