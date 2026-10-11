use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_tx::ChangelogStore;

use crate::repo::changelog_store::StoreChangelog;

// Red before the fix: the raw usize::MAX reservation overflows on empty data.
#[tokio::test]
async fn empty_journal_accepts_maximum_limit_without_reserving_it() {
    let journal = StoreChangelog::new(Arc::new(InMemoryStore::new()));
    assert!(journal
        .range_from(Bytes::new(), usize::MAX)
        .await
        .unwrap()
        .is_empty());
}

// Red before the fix: both 9 MiB entries are returned, exceeding 16 MiB.
#[tokio::test]
async fn journal_byte_budget_returns_a_resumable_prefix() {
    let journal = StoreChangelog::new(Arc::new(InMemoryStore::new()));
    for version in 1_u64..=2 {
        journal
            .put(
                Bytes::copy_from_slice(&version.to_be_bytes()),
                Bytes::from(vec![7; 9 * 1024 * 1024]),
            )
            .await
            .unwrap();
    }
    let first = journal.range_from(Bytes::new(), usize::MAX).await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].len(), 9 * 1024 * 1024);
    let second = journal
        .range_from(Bytes::copy_from_slice(&2_u64.to_be_bytes()), 1)
        .await
        .unwrap();
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].len(), 9 * 1024 * 1024);
}

// Red before the fix: an oversized first entry is returned as a successful page.
#[tokio::test]
async fn oversized_first_event_is_an_error_not_a_successful_empty_page() {
    let journal = StoreChangelog::new(Arc::new(InMemoryStore::new()));
    journal
        .put(Bytes::new(), Bytes::from(vec![0; 16 * 1024 * 1024 + 1]))
        .await
        .unwrap();
    let error = journal.range_from(Bytes::new(), 1).await.unwrap_err();
    assert!(error.contains("byte budget"), "{error}");
}
