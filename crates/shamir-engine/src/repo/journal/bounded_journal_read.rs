use std::sync::{Arc, OnceLock};

use bytes::Bytes;
use shamir_tx::{ChangelogStore, JournalRead};

/// One ascending journal page, limited to the confirmed completion prefix.
///
/// `bound <= sampled_w`, and `confirmed` is exactly `bound >= sampled_w`.
/// Events belong to the journal prefix at or below `bound`; `gap_at` retains
/// the underlying journal's conservative resynchronization semantics.
/// An empty page proves exhaustion through `bound` only when `confirmed`,
/// `gap_at` is absent, and the requested limit is nonzero. A short page is NOT
/// exhaustion: the storage adapter can stop early at its byte budget.
/// Confirmation establishes completed writes, not per-event fsync durability.
pub struct BoundedJournalRead {
    pub read: JournalRead,
    pub sampled_w: u64,
    pub bound: u64,
    pub confirmed: bool,
}

// read_from_bounded converts range failures to gaps. Observe that same single
// read without changing its decoding/gap policy or doing a second store read.
pub(crate) struct ErrorPreservingJournalStore {
    inner: Arc<dyn ChangelogStore>,
    error: OnceLock<String>,
}

impl ErrorPreservingJournalStore {
    pub(crate) fn new(inner: Arc<dyn ChangelogStore>) -> Self {
        Self {
            inner,
            error: OnceLock::new(),
        }
    }

    pub(crate) fn error(&self) -> Option<&str> {
        self.error.get().map(String::as_str)
    }
}

#[async_trait::async_trait]
impl ChangelogStore for ErrorPreservingJournalStore {
    async fn put(&self, key: Bytes, value: Bytes) -> Result<(), String> {
        self.inner.put(key, value).await
    }

    async fn range_from(&self, from_key: Bytes, limit: usize) -> Result<Vec<Bytes>, String> {
        let result = self.inner.range_from(from_key, limit).await;
        if let Err(error) = &result {
            let _ = self.error.set(error.clone());
        }
        result
    }
}
