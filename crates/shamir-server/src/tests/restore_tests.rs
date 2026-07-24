//! N-6 (CR-D5, #786): unit coverage for `restore.rs`'s step-5 swap-failure
//! error-message split and the step-3/step-4 staged-temp-dir cleanup.
//!
//! These are narrow, filesystem-only unit tests (no server boot) — the full
//! restore lifecycle (real `users`/`server_meta` stores, session
//! invalidation, actual server reboot) is already covered end-to-end by
//! `tests/backup_restore_e2e.rs`. This module isolates JUST the two
//! `restore()` failure shapes that test file doesn't otherwise poke at:
//! the swap-failure message split and the earlier-step cleanup.

use std::fs;
use std::path::Path;

use tempfile::TempDir;

use crate::backup::backup;
use crate::restore::{restore, RestoreError};

/// Build a well-formed `backup::backup` snapshot (with a valid manifest) at
/// a fresh temp dir, containing one small file — enough for `restore()`'s
/// step 2 (`verify_manifest`) and step 3 (copy) to succeed normally.
fn make_snapshot(root: &Path) -> std::path::PathBuf {
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a.txt"), b"hello").unwrap();
    let dst = root.join("snapshot_dest");
    fs::create_dir_all(&dst).unwrap();
    let report = backup(&src, &dst).expect("backup ok");
    report.dest_dir
}

// ----------------------------------------------------------------------------
// N-6: swap-failure message split — rollback SUCCEEDS case.
// ----------------------------------------------------------------------------

/// Forces `restore()`'s step-5 second rename (`temp_dir -> data_dir`) to
/// fail while the rollback rename (`backup_sibling -> data_dir`) succeeds,
/// by holding an open file handle inside the STAGED temp dir — on Windows,
/// an open handle inside a directory blocks `fs::rename` of that directory
/// (a sharing violation), while `backup_sibling` (nothing held open inside
/// it) renames back cleanly. Asserts the NEW `SwapFailedRollbackSucceeded`
/// message: `data_dir` is intact (holds the ORIGINAL pre-restore content),
/// no manual rename instruction, `temp_dir` still on disk for inspection.
///
/// `#[cfg(windows)]`: this failure-forcing mechanism is inherently a
/// Windows/NTFS behavior (an open handle blocks a rename of its containing
/// directory) — on POSIX, `rename(2)` does NOT check for open file
/// descriptors inside either directory at all, so the held handle here
/// would not make the rename fail, and `restore()` would simply succeed,
/// making `.unwrap_err()` panic. There is no equivalent portable trick that
/// blocks specifically the SECOND rename (`temp_dir -> data_dir`) without
/// also blocking the FIRST (`data_dir -> backup_sibling`) this test relies
/// on succeeding — a POSIX-side regression test for this exact code path
/// would need a dedicated test hook inside `restore()` itself, which is out
/// of proportion for this coverage. The code path itself is NOT
/// Windows-specific (`fs::rename`'s `Err` arm is handled identically on
/// every platform) — only this test's ABILITY to trigger that arm
/// deterministically is.
#[cfg(windows)]
#[test]
fn swap_failure_with_successful_rollback_gets_new_message_and_leaves_data_dir_intact() {
    let root = TempDir::new().unwrap();
    let snapshot = make_snapshot(root.path());

    // Pre-existing data_dir — restore()'s step 5 preserves this as the
    // `.pre_restore_backup_*` sibling once the (attempted, here failing)
    // swap runs.
    let data_dir = root.path().join("data_dir");
    fs::create_dir_all(&data_dir).unwrap();
    fs::write(data_dir.join("original.txt"), b"ORIGINAL PRE-RESTORE DATA").unwrap();

    // `restore()` is synchronous and mints `temp_dir` internally
    // (`{parent}/{data_dir_name}.restore_tmp_{stamp}`) — there's no hook to
    // intercept mid-call, so a background thread polls for the staged temp
    // dir's appearance (step 3's copy completes synchronously before step 5
    // runs) and opens a read handle on the file inside it the instant it
    // shows up, holding it for the rest of `restore()`'s call. On Windows,
    // an open handle inside a directory blocks `fs::rename` of that
    // directory (sharing violation) — so step 5's `fs::rename(&temp_dir,
    // data_dir)` fails, while `backup_sibling` (nothing held open inside
    // it) still renames back cleanly in the rollback.
    let parent = root.path().to_path_buf();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = stop.clone();
    let held_handle: std::sync::Arc<std::sync::Mutex<Option<std::fs::File>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let held_handle2 = held_handle.clone();
    let watcher = std::thread::spawn(move || {
        while !stop2.load(std::sync::atomic::Ordering::SeqCst) {
            if let Ok(entries) = fs::read_dir(&parent) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name = name.to_string_lossy();
                    if name.contains(".restore_tmp_") {
                        let candidate = entry.path().join("a.txt");
                        if let Ok(f) = fs::File::open(&candidate) {
                            *held_handle2.lock().unwrap() = Some(f);
                            return;
                        }
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    });

    let err = restore(&snapshot, &data_dir, true).unwrap_err();

    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    let _ = watcher.join();

    match &err {
        RestoreError::SwapFailedRollbackSucceeded {
            data_dir: dd,
            temp_dir,
            ..
        } => {
            assert_eq!(dd, &data_dir);
            assert!(
                temp_dir.exists(),
                "temp_dir must survive on disk for inspection/retry: {}",
                temp_dir.display()
            );
        }
        other => panic!(
            "expected SwapFailedRollbackSucceeded (rollback should have \
             succeeded — only the staged temp dir was locked, not the \
             pre_restore_backup sibling), got: {other:?}"
        ),
    }

    // Message must NOT instruct a manual rename (that's the OTHER variant's
    // language) — and must state data_dir is intact / no action needed.
    let msg = err.to_string();
    assert!(
        !msg.contains("operator must manually rename"),
        "rollback-succeeded message must not carry the manual-rename \
         instruction meant for the both-failed case: {msg}"
    );
    assert!(
        msg.contains("intact") && msg.contains("no manual action"),
        "rollback-succeeded message must plainly state data_dir is intact \
         and no manual action is needed: {msg}"
    );

    // Core guarantee: data_dir holds the ORIGINAL pre-restore content — the
    // rollback genuinely put it back, this isn't just a message-text check.
    let restored_original = fs::read_to_string(data_dir.join("original.txt")).unwrap();
    assert_eq!(restored_original, "ORIGINAL PRE-RESTORE DATA");

    drop(held_handle);
}

// ----------------------------------------------------------------------------
// W-5(a) (#790, Wave D review): the step-5 FIRST rename's failure path must
// clean up the staged temp_dir too, same as steps 3/4 — previously a bare
// `?` orphaned it with no reference anywhere in the error message.
// ----------------------------------------------------------------------------

/// Forces `restore()`'s step-5 FIRST rename (`data_dir -> backup_sibling`)
/// to fail, by holding an open file handle inside the PRE-EXISTING
/// `data_dir` itself (unlike the rollback test above, which locks the
/// STAGED temp dir to fail the SECOND rename) — on Windows, an open handle
/// inside a directory blocks `fs::rename` of that directory. `data_dir`
/// exists synchronously before `restore()` is even called here, so (unlike
/// the second-rename test) no background watcher thread is needed: the
/// handle can simply be opened up front and held for the whole call.
///
/// Asserts: the error is a plain `RestoreError::Io` (nothing has been
/// staged for a rollback yet — this is NOT `SwapFailedRollbackSucceeded`/
/// `SwapPartialFailure`, those are the SECOND rename's failure shapes), AND
/// the staged `*.restore_tmp_*` temp_dir is cleaned up (W-5(a)'s fix,
/// reusing the same `cleanup_staged_temp_dir` helper N-6 already added for
/// steps 3/4) rather than orphaned on disk with no pointer to it.
///
/// `#[cfg(windows)]`: same platform limitation as
/// `swap_failure_with_successful_rollback_gets_new_message_and_leaves_data_dir_intact`
/// above — an open file handle only blocks a directory rename on
/// Windows/NTFS, not on POSIX (`rename(2)` does not check for open fds).
#[cfg(windows)]
#[test]
fn step5_first_rename_failure_cleans_up_staged_temp_dir() {
    let root = TempDir::new().unwrap();
    let snapshot = make_snapshot(root.path());

    let data_dir = root.path().join("data_dir");
    fs::create_dir_all(&data_dir).unwrap();
    fs::write(data_dir.join("original.txt"), b"ORIGINAL PRE-RESTORE DATA").unwrap();

    // Hold an open read handle inside `data_dir` itself for the whole
    // `restore()` call — blocks the FIRST rename
    // (`fs::rename(data_dir, &backup_sibling)`) with a sharing violation on
    // Windows, before the second rename is ever attempted.
    let held = fs::File::open(data_dir.join("original.txt")).unwrap();

    let err = restore(&snapshot, &data_dir, true).unwrap_err();

    assert!(
        matches!(err, RestoreError::Io(_)),
        "the FIRST rename's failure must surface as a plain RestoreError::Io \
         (nothing has been staged for a rollback yet, so this must NOT be \
         SwapFailedRollbackSucceeded/SwapPartialFailure, which are the SECOND \
         rename's failure shapes), got: {err:?}"
    );

    let leftover = fs::read_dir(root.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().contains(".restore_tmp_"));
    assert!(
        !leftover,
        "the staged *.restore_tmp_* dir must be cleaned up after a step-5 \
         FIRST-rename failure (W-5(a), #790) -- it survived, meaning the \
         cleanup did not run or failed silently"
    );

    // data_dir itself must be untouched -- the first rename never actually
    // completed (it failed), so the original content is exactly as it was.
    let original = fs::read_to_string(data_dir.join("original.txt")).unwrap();
    assert_eq!(original, "ORIGINAL PRE-RESTORE DATA");

    drop(held);
}

// ----------------------------------------------------------------------------
// W-5(b) (#790, Wave D review): the staged temp_dir is now created with a
// single atomic `fs::create_dir` call instead of a separate `exists()` check
// followed by `create_dir_all` -- closing a check-then-act TOCTOU. The
// operator-facing "already exists" message must stay identical.
// ----------------------------------------------------------------------------

/// A pre-existing directory already occupying the computed `temp_dir` name
/// must still be reported via the SAME `AlreadyExists`-shaped
/// `RestoreError::Io` message as before the `fs::create_dir` swap -- the
/// underlying detection mechanism changed (a single atomic syscall instead
/// of a separate `exists()` probe), but the operator-facing behavior must
/// not.
#[test]
fn preexisting_temp_dir_name_collision_still_reports_already_exists() {
    let root = TempDir::new().unwrap();
    let snapshot = make_snapshot(root.path());

    let data_dir = root.path().join("data_dir");

    // `restore()` computes `temp_dir` deterministically as
    // `{parent}/{data_dir_name}.restore_tmp_{stamp}` where `stamp` is a
    // second-granularity UTC `YYYYMMDD_HHMMSS` timestamp taken INSIDE
    // `restore()` itself -- there is no hook to read it back directly, so
    // pre-create a directory at the name `crate::backup::utc_timestamp()`
    // produces right here, immediately before calling `restore()`, to
    // minimize (though not with absolute certainty eliminate) the
    // vanishingly small second-boundary race between this call and
    // `restore()`'s own internal timestamp.
    let stamp = crate::backup::utc_timestamp();
    let temp_dir = root.path().join(format!("data_dir.restore_tmp_{stamp}"));
    fs::create_dir_all(&temp_dir).unwrap();

    let err = restore(&snapshot, &data_dir, true).unwrap_err();

    match &err {
        RestoreError::Io(io_err) => {
            assert_eq!(
                io_err.kind(),
                std::io::ErrorKind::AlreadyExists,
                "expected an AlreadyExists io error, got: {io_err:?}"
            );
        }
        other => panic!("expected RestoreError::Io(AlreadyExists), got: {other:?}"),
    }
    let msg = err.to_string();
    assert!(
        msg.contains("temporary restore dir already exists"),
        "the AlreadyExists message text must be unchanged by the \
         create_dir_all -> create_dir mechanism swap: {msg}"
    );
}

// ----------------------------------------------------------------------------
// N-6: staged temp dir cleanup on an EARLIER (step 3/4) failure.
// ----------------------------------------------------------------------------

/// A step-2 manifest-verification failure happens BEFORE step 3 even
/// creates the temp dir — sanity check that `restore()` still fails cleanly
/// and (trivially) leaves nothing to clean up. This isolates the "no temp
/// dir was ever created" baseline before the step-3/4 cleanup tests below.
#[test]
fn manifest_verification_failure_creates_no_temp_dir() {
    let root = TempDir::new().unwrap();
    let bogus_snapshot = root.path().join("no_manifest_here");
    fs::create_dir_all(&bogus_snapshot).unwrap();
    fs::write(bogus_snapshot.join("a.txt"), b"hello").unwrap();
    // No manifest.json written — verify_manifest (step 2) must fail first.

    let data_dir = root.path().join("data_dir");

    let err = restore(&bogus_snapshot, &data_dir, true).unwrap_err();
    assert!(
        matches!(err, RestoreError::ManifestVerification(_)),
        "expected a manifest verification failure, got {err:?}"
    );

    let leftover = fs::read_dir(root.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().contains(".restore_tmp_"));
    assert!(
        !leftover,
        "no *.restore_tmp_* dir should exist — step 2 failed before step 3 \
         ever created one"
    );
}

/// Forces the step-4 `users` open/invalidate failure (a corrupted `users`
/// store staged inside the snapshot, mirroring
/// `restore_failure_at_pre_swap_invalidation_leaves_data_dir_untouched` in
/// `tests/backup_restore_e2e.rs`, but WITHOUT a real server boot) and
/// asserts the staged `*.restore_tmp_*` temp dir is removed afterward — the
/// N-6 cleanup gap this task closes.
#[test]
fn step4_invalidation_failure_cleans_up_staged_temp_dir() {
    let root = TempDir::new().unwrap();
    let src = root.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("a.txt"), b"hello").unwrap();
    // A `users` "directory" that is actually a FILE — `FjallUserDirectory::
    // open` (fjall keyspace open) on a path that is a plain file, not a
    // directory, fails structurally, exercising step 4's failure path
    // without needing a real corrupted fjall keyspace.
    fs::write(src.join("users"), b"not a directory, not a valid keyspace").unwrap();

    let dst = root.path().join("snapshot_dest");
    fs::create_dir_all(&dst).unwrap();
    let report = backup(&src, &dst).expect("backup ok");

    let data_dir = root.path().join("data_dir");

    let err = restore(&report.dest_dir, &data_dir, true).unwrap_err();
    assert!(
        matches!(
            err,
            RestoreError::UserDirectory(_) | RestoreError::Invalidate(_)
        ),
        "expected a step-4 users-open/invalidate failure, got {err:?}"
    );

    let leftover = fs::read_dir(root.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .any(|e| e.file_name().to_string_lossy().contains(".restore_tmp_"));
    assert!(
        !leftover,
        "the staged *.restore_tmp_* dir must be cleaned up after a step-4 \
         (invalidate) failure — it survived, meaning the N-6 cleanup did \
         not run or failed silently"
    );

    // data_dir itself must be untouched — the swap (step 5) never ran.
    assert!(
        !data_dir.exists(),
        "data_dir must not have been created — restore failed before step 5"
    );
}
