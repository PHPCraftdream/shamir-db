use crate::db::engine::table::counter::RecordCounter;
use crate::db::storage::storage_in_memory::InMemoryStore;
use std::sync::Arc;

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
