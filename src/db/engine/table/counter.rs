//! Record counter for tracking number of records in a table

use crate::db::error::{DbError, DbResult};
use crate::db::storage::types::Store;
use crate::codecs::bytes;
use crate::types::record_id::RecordId;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Get the system record key for storing record count
fn count_key() -> RecordId {
    RecordId::system("count")
}

/// Manages record count in a table
///
/// Provides atomic increment/decrement operations to track the number of records.
/// Uses a mutex for thread safety.
pub struct RecordCounter {
    info_store: Arc<dyn Store>,
    counter_mutex: Mutex<()>,
}

impl RecordCounter {
    /// Create a new record counter
    pub fn new(info_store: Arc<dyn Store>) -> Self {
        Self {
            info_store,
            counter_mutex: Mutex::new(()),
        }
    }

    /// Get current record count
    pub async fn get(&self) -> DbResult<u64> {
        let key_bytes = count_key().to_bytes();
        match self.info_store.get(key_bytes).await {
            Ok(bytes) => {
                let count: u64 = bytes::from_bytes(&bytes)
                    .map_err(|e| DbError::Codec(format!("Failed to deserialize count: {}", e)))?;
                Ok(count)
            }
            Err(DbError::NotFound(_)) => Ok(0),
            Err(e) => Err(e),
        }
    }

    /// Set record count (useful for initialization or manual correction)
    pub async fn set(&self, count: u64) -> DbResult<()> {
        let key_bytes = count_key().to_bytes();
        let bytes = bytes::to_bytes(&count)
            .map_err(|e| DbError::Codec(format!("Failed to serialize count: {}", e)))?;
        self.info_store.set(key_bytes, bytes).await?;
        Ok(())
    }

    /// Increment record count by delta (with mutex lock for thread safety)
    pub async fn increment(&self, delta: i64) -> DbResult<()> {
        let _guard = self.counter_mutex.lock().await;
        let current = self.get().await? as i64;
        let new_count = current + delta;
        if new_count < 0 {
            return Err(DbError::Internal(format!(
                "Record count cannot be negative: current={}, delta={}",
                current, delta
            )));
        }
        self.set(new_count as u64).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::storage::storage_in_memory::InMemoryStore;

    async fn create_counter() -> RecordCounter {
        RecordCounter::new(Arc::new(InMemoryStore::new()))
    }

    #[tokio::test]
    async fn test_counter_initial_state() {
        let counter = create_counter().await;
        assert_eq!(counter.get().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_counter_increment() {
        let counter = create_counter().await;
        counter.increment(1).await.unwrap();
        assert_eq!(counter.get().await.unwrap(), 1);

        counter.increment(5).await.unwrap();
        assert_eq!(counter.get().await.unwrap(), 6);
    }

    #[tokio::test]
    async fn test_counter_decrement() {
        let counter = create_counter().await;
        counter.increment(10).await.unwrap();
        assert_eq!(counter.get().await.unwrap(), 10);

        counter.increment(-3).await.unwrap();
        assert_eq!(counter.get().await.unwrap(), 7);
    }

    #[tokio::test]
    async fn test_counter_cannot_go_negative() {
        let counter = create_counter().await;
        counter.increment(5).await.unwrap();

        let result = counter.increment(-10).await;
        assert!(result.is_err());
        assert_eq!(counter.get().await.unwrap(), 5);
    }

    #[tokio::test]
    async fn test_counter_set() {
        let counter = create_counter().await;
        counter.set(100).await.unwrap();
        assert_eq!(counter.get().await.unwrap(), 100);

        counter.set(50).await.unwrap();
        assert_eq!(counter.get().await.unwrap(), 50);
    }

    #[tokio::test]
    async fn test_counter_thread_safety() {
        let counter = Arc::new(create_counter().await);
        let mut handles = vec![];

        for _i in 0..10 {
            let counter_clone = Arc::clone(&counter);
            handles.push(tokio::spawn(async move {
                for _ in 0..10 {
                    counter_clone.increment(1).await.unwrap();
                }
            }));
        }

        for handle in handles {
            handle.await.unwrap();
        }

        assert_eq!(counter.get().await.unwrap(), 100);
    }
}
