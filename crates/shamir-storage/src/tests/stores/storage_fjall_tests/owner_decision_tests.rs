use super::*;
use futures::StreamExt;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::test]
async fn rv_storage_cc5c79ba_names_use_utf8_byte_boundaries_without_filtering() {
    let dir = tempfile::tempdir().unwrap();
    let repo = FjallRepo::new(dir.path()).unwrap();
    let valid = [
        "x".repeat(255),
        format!("{}é", "x".repeat(253)),
        "punct /\\:#?💖".to_string(),
    ];
    for name in &valid {
        let store = repo.store_get(name).await.unwrap();
        store
            .set(RecordKey::from_slice(b"key"), Bytes::from_static(b"value"))
            .await
            .unwrap();
        assert_eq!(
            store.get(RecordKey::from_slice(b"key")).await.unwrap(),
            Bytes::from_static(b"value")
        );
    }
    let invalid = [
        String::new(),
        "x".repeat(256),
        format!("{}é", "x".repeat(254)),
    ];
    let before = repo.stores_list().await.unwrap();
    for name in &invalid {
        assert!(matches!(
            repo.store_get(name).await,
            Err(DbError::Validation(_))
        ));
        assert!(matches!(
            repo.store_delete(name).await,
            Err(DbError::Validation(_))
        ));
        let mut batch = repo.begin_atomic_batch().await.unwrap();
        batch
            .set(
                "staged-valid",
                RecordKey::from_slice(b"key"),
                Bytes::from_static(b"value"),
            )
            .unwrap();
        assert!(matches!(
            batch.set(name, RecordKey::from_slice(b"key"), Bytes::new()),
            Err(DbError::Validation(_))
        ));
        assert!(matches!(
            batch.delete(name, RecordKey::from_slice(b"key")),
            Err(DbError::Validation(_))
        ));
        drop(batch);
        let empty = repo.begin_atomic_batch().await.unwrap();
        assert!(matches!(
            empty.commit().await,
            Ok(AtomicCommitOutcome::Committed)
        ));
    }
    let after = repo.stores_list().await.unwrap();
    assert_eq!(before.len(), after.len());
    for name in &before {
        assert!(after.contains(name));
    }
    for name in &valid {
        assert!(repo.store_delete(name).await.unwrap());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rv_storage_0a9e3a41_spawn_failure_is_cached_and_preserves_source() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::builder(dir.path()).open().unwrap());
    let keyspace = db
        .keyspace("worker-failure", KeyspaceCreateOptions::default)
        .unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&attempts);
    let store = Arc::new(FjallStore {
        keyspace,
        db,
        worker: std::sync::OnceLock::new(),
        spawn_worker: Some(Box::new(move |_, _| {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "injected spawn failure",
            ))
        })),
    });
    let key = RecordKey::from_slice(b"not-written");
    let (insert, transact) = tokio::join!(
        store.insert(Bytes::from_static(b"value")),
        store.transact(vec![KvOp::Set(key.clone(), Bytes::from_static(b"value"))]),
    );
    for err in [
        insert.unwrap_err(),
        transact.unwrap_err(),
        store.insert(Bytes::new()).await.unwrap_err(),
        store
            .transact(vec![KvOp::Remove(key.clone())])
            .await
            .unwrap_err(),
    ] {
        let DbError::Io(err) = err else {
            panic!("expected Io error")
        };
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        let wrapper = err.get_ref().unwrap();
        let original = wrapper
            .source()
            .unwrap()
            .downcast_ref::<io::Error>()
            .unwrap();
        assert_eq!(original.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(original.to_string(), "injected spawn failure");
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(matches!(store.get(key).await, Err(DbError::NotFound(_))));
    assert!(store.iter_stream(10).next().await.is_none());
}

#[test]
fn rv_storage_0a9e3a41_concurrent_first_use_spawns_one_worker() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::builder(dir.path()).open().unwrap());
    let keyspace = db
        .keyspace("worker-success", KeyspaceCreateOptions::default)
        .unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let calls = Arc::clone(&attempts);
    let store = Arc::new(FjallStore {
        keyspace,
        db,
        worker: std::sync::OnceLock::new(),
        spawn_worker: Some(Box::new(move |keyspace, db| {
            calls.fetch_add(1, Ordering::SeqCst);
            WriteWorker::spawn(keyspace, db)
        })),
    });
    let barrier = Arc::new(std::sync::Barrier::new(8));
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            scope.spawn(move || {
                barrier.wait();
                assert!(store.worker().is_ok());
            });
        }
    });
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[test]
fn rv_storage_cc5c79ba_commit_preflights_all_names_before_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(Database::builder(dir.path()).open().unwrap());
    let existing = db
        .keyspace("existing", KeyspaceCreateOptions::default)
        .unwrap();
    existing.insert(b"key", b"old").unwrap();
    let key = RecordKey::from_slice(b"key");
    let ops = vec![
        (
            "would-create".to_string(),
            KvOp::Set(key.clone(), Bytes::from_static(b"new")),
        ),
        (
            "existing".to_string(),
            KvOp::Set(key.clone(), Bytes::from_static(b"new")),
        ),
        ("x".repeat(256), KvOp::Remove(key)),
    ];
    assert!(matches!(
        exec_atomic_commit(&db, ops),
        Err(DbError::Validation(_))
    ));
    assert!(!db.keyspace_exists("would-create"));
    assert_eq!(existing.get(b"key").unwrap().unwrap().as_ref(), b"old");
}
