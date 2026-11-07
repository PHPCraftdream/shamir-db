use crate::repo::instance::repo_instance::table_store::copy::copy_store_contents;
use crate::repo::journal::backend::repo_types::RepoFactory;
use crate::repo::repo_types::BoxRepoFactory;
use async_trait::async_trait;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::types::{RecordKey, Repo, Store};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct FailSecondBatchStore {
    inner: Arc<dyn Store>,
    batches: AtomicUsize,
}

#[async_trait]
impl Store for FailSecondBatchStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }

    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.inner.set(key, value).await
    }

    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }

    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }

    async fn set_many(&self, items: Vec<(RecordKey, Bytes)>) -> DbResult<Vec<bool>> {
        if self.batches.fetch_add(1, Ordering::SeqCst) == 1 {
            let mut items = items.into_iter();
            let (key, value) = items.next().expect("second page is nonempty");
            self.inner.set(key, value).await?;
            return Err(DbError::Internal(
                "injected second copy batch failure".into(),
            ));
        }
        self.inner.set_many(items).await
    }

    async fn flush(&self) -> DbResult<()> {
        self.inner.flush().await
    }

    fn iter_stream(
        &self,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        self.inner.iter_stream(batch_size)
    }

    fn scan_prefix_stream(
        &self,
        prefix: Bytes,
        batch_size: usize,
    ) -> Pin<Box<dyn Stream<Item = Result<Vec<(RecordKey, Bytes)>, DbError>> + Send>> {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

#[test]
fn second_page_copy_failure_preserves_partial_write_and_retry_target_is_clean() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let repo = BoxRepoFactory::in_memory().create().await.unwrap();
        let source = repo.store_get("copy-source").await.unwrap();
        let values: Vec<_> = (0..300_u32)
            .map(|n| Bytes::from(n.to_le_bytes().to_vec()))
            .collect();
        let keys = source.insert_many(values.clone()).await.unwrap();
        source.flush().await.unwrap();

        let failed_inner = repo.store_get("copy-failed").await.unwrap();
        let failed = Arc::new(FailSecondBatchStore {
            inner: Arc::clone(&failed_inner),
            batches: AtomicUsize::new(0),
        });
        assert!(copy_store_contents(Arc::clone(&source), failed.clone())
            .await
            .is_err());
        assert_eq!(failed.batches.load(Ordering::SeqCst), 2);
        assert_eq!(keys.len(), 300);
        assert_eq!(
            source
                .get_many(keys.clone())
                .await
                .unwrap()
                .into_iter()
                .flatten()
                .collect::<Vec<_>>(),
            values
        );
        assert_eq!(
            failed_inner
                .iter_stream(512)
                .next()
                .await
                .unwrap()
                .unwrap()
                .len(),
            257
        );

        let retry = repo.store_get("copy-retry").await.unwrap();
        copy_store_contents(Arc::clone(&source), Arc::clone(&retry))
            .await
            .unwrap();
        let copied = retry.iter_stream(512).next().await.unwrap().unwrap();
        assert_eq!(copied.len(), values.len());
        assert_eq!(
            copied
                .into_iter()
                .map(|(_, value)| value)
                .collect::<std::collections::BTreeSet<_>>(),
            values.into_iter().collect()
        );
    });
}
