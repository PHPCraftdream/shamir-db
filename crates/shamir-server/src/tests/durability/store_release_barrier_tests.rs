use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::server::wait_stores_released;

/// A fake fjall store root: `lock` + `version` marker files.
fn make_store(root: &Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).expect("store dir");
    std::fs::write(dir.join("version"), b"3").expect("version marker");
    std::fs::write(dir.join("lock"), b"").expect("lock file");
    dir
}

fn hold_lock(dir: &Path) -> File {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(dir.join("lock"))
        .expect("open lock");
    file.try_lock().expect("take the store lock");
    file
}

#[tokio::test]
async fn no_stores_means_released() {
    let temp = tempfile::tempdir().expect("tempdir");
    let res = wait_stores_released(temp.path(), Duration::from_secs(5)).await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn unlocked_stores_are_released_at_once() {
    let temp = tempfile::tempdir().expect("tempdir");
    make_store(temp.path(), "a.redb");
    make_store(&temp.path().join("nested"), "b");
    let res = wait_stores_released(temp.path(), Duration::from_secs(5)).await;
    assert!(res.is_ok());
}

#[tokio::test]
async fn waits_until_the_last_holder_drops_the_lock() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = make_store(temp.path(), "held.redb");
    let holder = hold_lock(&store);

    let released = Arc::new(AtomicBool::new(false));
    let release = {
        let released = Arc::clone(&released);
        async move {
            tokio::time::sleep(Duration::from_millis(80)).await;
            released.store(true, Ordering::SeqCst);
            drop(holder);
        }
    };
    let (res, ()) = tokio::join!(
        wait_stores_released(temp.path(), Duration::from_secs(30)),
        release
    );

    assert!(res.is_ok());
    assert!(
        released.load(Ordering::SeqCst),
        "the barrier must not return before the holder dropped the lock"
    );
}

#[tokio::test]
async fn reports_the_stores_still_locked_after_the_timeout() {
    let temp = tempfile::tempdir().expect("tempdir");
    let store = make_store(temp.path(), "held.redb");
    let _holder = hold_lock(&store);

    let res = wait_stores_released(temp.path(), Duration::from_millis(100)).await;

    assert_eq!(res, Err(vec![store]));
}

#[tokio::test]
async fn directories_without_both_markers_are_not_stores() {
    let temp = tempfile::tempdir().expect("tempdir");
    let dir = temp.path().join("not-a-store");
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join("lock"), b"").expect("lock only");
    let _holder = hold_lock(&dir);

    let res = wait_stores_released(temp.path(), Duration::from_millis(100)).await;

    assert!(res.is_ok());
}
