use crate::mvcc_store::{MvccStore, Retention};
use crate::repo_tx_gate::RepoTxGate;
use crate::tests::mvcc_store_tests::gc::test_stores::failing_store::FailingStore;
use crate::version_codec::encode_version_key;
use bytes::Bytes;
use shamir_storage::error::DbError;
use shamir_storage::types::{RecordKey, Store};
use std::sync::atomic::Ordering;
use std::sync::Arc;

#[tokio::test]
async fn rv_tx_fb689ad7_vacuum_reports_scan_failure_and_retries() {
    let history = Arc::new(FailingStore::new());
    let gate = Arc::new(RepoTxGate::fresh());
    let mvcc = MvccStore::new(history.clone() as Arc<dyn Store>, gate);
    mvcc.set_retention(Retention::keep_history()).unwrap();

    let key = Bytes::from_static(b"key");
    for value in [b"v1".as_slice(), b"v2", b"v3"] {
        mvcc.set_versioned(RecordKey::from(key.clone()), Bytes::copy_from_slice(value))
            .await
            .unwrap();
    }
    let versions = [1, 2, 3];
    for version in versions {
        assert!(history
            .get(encode_version_key(&key, version).into())
            .await
            .is_ok());
    }
    assert_eq!(mvcc.ts_index_len(), 3);
    mvcc.set_retention(Retention {
        max_count: Some(1),
        max_age_secs: None,
        min_count: None,
    })
    .unwrap();

    history.fail_scan.store(true, Ordering::Relaxed);
    let error = mvcc.vacuum_key(&key, 0).await.unwrap_err();
    assert!(matches!(error, DbError::Storage(_)));
    for version in versions {
        assert!(history
            .get(encode_version_key(&key, version).into())
            .await
            .is_ok());
    }
    assert_eq!(mvcc.ts_index_len(), 3);

    mvcc.vacuum_key(&key, 0).await.unwrap();
    assert!(history
        .get(encode_version_key(&key, 1).into())
        .await
        .is_err());
    assert!(history
        .get(encode_version_key(&key, 2).into())
        .await
        .is_err());
    assert!(history
        .get(encode_version_key(&key, 3).into())
        .await
        .is_ok());
    assert_eq!(mvcc.ts_index_len(), 1);
}
