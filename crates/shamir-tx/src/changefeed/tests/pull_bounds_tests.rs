use std::sync::Arc;

use bytes::Bytes;

use crate::changefeed::{ChangelogError, ChangelogStore, RepoChangefeed};

struct FailingJournal;

#[async_trait::async_trait]
impl ChangelogStore for FailingJournal {
    async fn put(&self, _key: Bytes, _value: Bytes) -> Result<(), ChangelogError> {
        Ok(())
    }

    async fn range_from(&self, _key: Bytes, _limit: usize) -> Result<Vec<Bytes>, ChangelogError> {
        Err(ChangelogError::Journal(
            "changelog event exceeds byte budget".into(),
        ))
    }
}

// Red before the fix: read errors masquerade as a successful empty tail,
// allowing a follower to continue without the withheld oversized event.
#[tokio::test]
async fn journal_read_failure_signals_resync_at_requested_version() {
    let store: Arc<dyn ChangelogStore> = Arc::new(FailingJournal);
    let feed = RepoChangefeed::new(Arc::clone(&store));
    let result = feed.read_from(&store, 7, usize::MAX).await;
    assert!(result.events.is_empty());
    assert_eq!(result.gap_at, Some(7));
}
