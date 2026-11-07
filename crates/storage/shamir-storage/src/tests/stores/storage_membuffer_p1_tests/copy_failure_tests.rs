use crate::error::{DbError, DbResult};
use crate::storage_in_memory::InMemoryStore;
use crate::types::{RecordKey, RecordStream, Repo, Store};
use async_trait::async_trait;
use bytes::Bytes;
use std::sync::Arc;

struct FailingSource;

struct CopyRepo {
    source: Arc<dyn Store>,
    destination: Arc<dyn Store>,
}

#[async_trait]
impl Repo for CopyRepo {
    async fn store_get<S: AsRef<str> + Send>(&self, name: S) -> DbResult<Arc<dyn Store>> {
        match name.as_ref() {
            "source" => Ok(Arc::clone(&self.source)),
            "destination" => Ok(Arc::clone(&self.destination)),
            _ => Err(DbError::Storage("unknown test store".into())),
        }
    }

    async fn store_delete<S: AsRef<str> + Send>(&self, _: S) -> DbResult<bool> {
        Ok(false)
    }

    async fn stores_list(&self) -> DbResult<Vec<String>> {
        Ok(vec!["source".into(), "destination".into()])
    }
}

#[async_trait]
impl Store for FailingSource {
    async fn insert(&self, _: Bytes) -> DbResult<RecordKey> {
        Err(DbError::Storage("unused source write".into()))
    }
    async fn set(&self, _: RecordKey, _: Bytes) -> DbResult<bool> {
        Err(DbError::Storage("unused source write".into()))
    }
    async fn get(&self, _: RecordKey) -> DbResult<Bytes> {
        Err(DbError::Storage("unused source read".into()))
    }
    async fn remove(&self, _: RecordKey) -> DbResult<bool> {
        Err(DbError::Storage("unused source remove".into()))
    }
    fn iter_stream(&self, _: usize) -> RecordStream {
        Box::pin(futures::stream::iter(vec![
            Ok(vec![(
                Bytes::from_static(b"first").into(),
                Bytes::from_static(b"v"),
            )]),
            Err(DbError::Storage("injected partial-copy error".into())),
        ]))
    }
    fn scan_prefix_stream(&self, _: Bytes, _: usize) -> RecordStream {
        Box::pin(futures::stream::empty())
    }
    fn iter_range_stream(&self, _: Option<Bytes>, _: Option<Bytes>, _: usize) -> RecordStream {
        Box::pin(futures::stream::empty())
    }
    fn iter_range_stream_reverse(
        &self,
        _: Option<Bytes>,
        _: Option<Bytes>,
        _: usize,
    ) -> RecordStream {
        Box::pin(futures::stream::empty())
    }
}

#[tokio::test]
async fn rv_storage_995fd07d_partial_copy_failure_preserves_completed_batch() {
    let destination: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    destination
        .set(
            Bytes::from_static(b"first").into(),
            Bytes::from_static(b"old"),
        )
        .await
        .unwrap();
    destination
        .set(
            Bytes::from_static(b"unrelated").into(),
            Bytes::from_static(b"keep"),
        )
        .await
        .unwrap();
    let repo = CopyRepo {
        source: Arc::new(FailingSource),
        destination: Arc::clone(&destination),
    };
    let result = repo.copy_store("source", "destination").await;
    assert!(result.is_err());
    assert_eq!(
        destination
            .get(Bytes::from_static(b"first").into())
            .await
            .unwrap(),
        Bytes::from_static(b"v")
    );
    assert_eq!(
        destination
            .get(Bytes::from_static(b"unrelated").into())
            .await
            .unwrap(),
        Bytes::from_static(b"keep")
    );
}
