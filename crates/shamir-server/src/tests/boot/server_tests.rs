use fs4::fs_std::FileExt;

/// Proves the advisory OS file lock mechanism used for the single-instance
/// guard:
/// 1. First `try_lock_exclusive` succeeds.
/// 2. A second `try_lock_exclusive` on the same path fails (would-block).
/// 3. After dropping the first lock, a third `try_lock_exclusive` succeeds
///    (release-on-drop).
///
/// This tests the lock primitive directly on a tempfile — it does NOT
/// boot a full `ServerLauncher`. The wiring from `launch()` into
/// `ServerHandle._data_dir_lock` is verified by compilation (the field
/// is populated in the `Ok(ServerHandle { ... })` constructor; removing
/// it is a compile error). A full-launcher test would require TLS certs,
/// ports, bootstrap, etc. and is covered by `mvp_e2e`.
#[test]
fn data_dir_lock_blocks_second_instance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let lock_path = dir.path().join(".shamir.lock");

    // 1. First lock — must succeed.
    let file1 = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .expect("open lock file");
    file1.try_lock_exclusive().expect("first lock must succeed");

    // 2. Second lock on the same path — must fail (WouldBlock).
    let file2 = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock file again");
    let err = file2
        .try_lock_exclusive()
        .expect_err("second lock must fail while first is held");
    // On Unix, fs4 returns WouldBlock; on Windows the OS error code 33
    // ("The process cannot access the file because another process has
    // locked a portion of the file") maps to Uncategorized in current
    // Rust std. Accept either.
    let is_lock_conflict =
        err.kind() == std::io::ErrorKind::WouldBlock || err.raw_os_error() == Some(33);
    assert!(
        is_lock_conflict,
        "expected WouldBlock or OS error 33, got: {err:?}",
    );

    // 3. Drop first lock → third attempt must succeed (release-on-drop).
    drop(file1);
    let file3 = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open lock file third time");
    file3
        .try_lock_exclusive()
        .expect("third lock must succeed after first is dropped");
}
