use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::ShamirDb;
use std::fs;

use crate::backup::backup;
use crate::restore::{consume_restore_marker, restore, write_restore_marker};

#[tokio::test]
async fn restore_all_repos_once_retry_receipt_and_future_repo() {
    let directory = tempfile::TempDir::new().unwrap();
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("app").await.expect("create_db");
    for name in ["one", "two"] {
        db.add_repo("app", RepoConfig::new(name, BoxRepoFactory::in_memory()))
            .await
            .unwrap();
    }
    let database = db.get_db("app").unwrap();
    let one = database.get_repo("one").unwrap();
    let two = database.get_repo("two").unwrap();
    let operation = write_restore_marker(directory.path()).unwrap();
    let marker = directory.path().join(".shamir_restore_epoch");
    let marker_bytes = fs::read(&marker).unwrap();
    // Simulate bump-before-marker-removal interruption on the first repo.
    one.apply_restore_operation(operation).await.unwrap();
    consume_restore_marker(directory.path(), &db).await.unwrap();
    assert!(!marker.exists());
    assert_eq!(one.repo_identity().await.unwrap().history_epoch, 1);
    assert_eq!(two.repo_identity().await.unwrap().history_epoch, 1);
    // Simulate marker deletion not surviving restart: durable receipts win.
    fs::write(&marker, marker_bytes).unwrap();
    consume_restore_marker(directory.path(), &db).await.unwrap();
    consume_restore_marker(directory.path(), &db).await.unwrap();
    assert_eq!(one.repo_identity().await.unwrap().history_epoch, 1);
    assert_eq!(two.repo_identity().await.unwrap().history_epoch, 1);
    db.add_repo(
        "app",
        RepoConfig::new("future", BoxRepoFactory::in_memory()),
    )
    .await
    .unwrap();
    consume_restore_marker(directory.path(), &db).await.unwrap();
    assert_eq!(
        database
            .get_repo("future")
            .unwrap()
            .repo_identity()
            .await
            .unwrap()
            .history_epoch,
        0
    );
    let next = write_restore_marker(directory.path()).unwrap();
    assert_ne!(operation, next, "a copied marker must not be adopted");
    consume_restore_marker(directory.path(), &db).await.unwrap();
    assert_eq!(one.repo_identity().await.unwrap().history_epoch, 2);
}

#[tokio::test]
async fn corrupt_restore_marker_refused_without_bump() {
    let directory = tempfile::TempDir::new().unwrap();
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("app").await.expect("create_db");
    db.add_repo("app", RepoConfig::new("main", BoxRepoFactory::in_memory()))
        .await
        .unwrap();
    let repo = db.get_db("app").unwrap().get_repo("main").unwrap();
    let before = repo.repo_identity().await.unwrap();
    let marker = directory.path().join(".shamir_restore_epoch");
    fs::write(&marker, b"invalid").unwrap();
    assert!(consume_restore_marker(directory.path(), &db).await.is_err());
    assert!(marker.exists());
    assert_eq!(repo.repo_identity().await.unwrap(), before);
}

#[test]
fn physical_restore_replaces_backup_marker_before_publication() {
    let directory = tempfile::TempDir::new().unwrap();
    let source = directory.path().join("source");
    let snapshots = directory.path().join("snapshots");
    let destination = directory.path().join("restored");
    fs::create_dir(&source).unwrap();
    let old = write_restore_marker(&source).unwrap();
    let snapshot = backup(&source, &snapshots).unwrap().dest_dir;
    restore(&snapshot, &destination, true).unwrap();
    let marker = fs::read(destination.join(".shamir_restore_epoch")).unwrap();
    assert_eq!(&marker[..4], b"SRE1");
    assert_eq!(marker.len(), 20);
    assert_ne!(&marker[4..], &old);
}
