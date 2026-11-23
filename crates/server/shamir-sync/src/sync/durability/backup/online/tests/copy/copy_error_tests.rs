//! Typed-failure tests of the copy path: a listing error is never a
//! silent omission (and never a published manifest), a destination
//! failure is never the tolerated vanished-source race, a failed
//! partial-tree cleanup is reported SEPARATELY from the primary error,
//! every destination-side publication failure is classified apart from a
//! source io error, and the scheduled rendering of a failure carries no
//! path and no OS text (CI only).

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use tempfile::TempDir;

use super::super::failure_injection_tests::{run as run_online, write_file, InjectPort};
use crate::sync::durability::backup::online::copy_engine::test_seams::{self, SeamGuard};
use crate::sync::durability::backup::online::copy_engine::{run_copy_pass, CopyError, CopyOptions};
use crate::sync::durability::backup::online::manifest;
use crate::sync::durability::backup::online::progress::{BackupStage, CancelFlag};
use crate::sync::durability::backup::online::protocol::{
    stage_and_io_reason, CleanupFailure, OnlineBackupError, OnlineBackupFailure,
};

/// A directory-entry failure while listing the source fails the WHOLE
/// protocol typed with the EXACT failing entry, and no `manifest.json`
/// is ever published: with the hole cleared the same tree completes a
/// run whose manifest describes every source file.
///
/// The seam replaces the metadata RESULT the listing consumes for the
/// matching path, so the injected error travels the very match arm that
/// handles a genuine `symlink_metadata` error.
///
/// Mutant: dropping the entry instead of failing the listing
/// (`filter_map(|entry| entry.ok()..)` / `Err(_) => continue`), or never
/// consulting the injection, makes the pass return `Ok`, so `expect_err`
/// fails and no typed path is ever asserted.
#[test]
fn enumeration_error_fails_the_pass_and_drops_no_file_silently() {
    let source = TempDir::new().expect("source temp dir");
    InjectPort::source_with_data(source.path());
    let failing = source.path().join("repos/alpha/store.bin");
    let failing_for_hook = failing.clone();
    let seam: SeamGuard = test_seams::set_symlink_metadata(Box::new(move |path: &Path| {
        if path == failing_for_hook.as_path() {
            Some(Err(std::io::Error::new(
                ErrorKind::PermissionDenied,
                "listing denied",
            )))
        } else {
            None
        }
    }));
    let cancel = CancelFlag::default();
    let to = TempDir::new().expect("destination root");
    let port = InjectPort::two_repos(source.path());
    let (result, _progress) = run_online(&port, source.path(), to.path(), &cancel);
    drop(seam);
    let error = result.expect_err("a listing error must fail the run, never drop the entry");
    let OnlineBackupError::Failed {
        stage,
        source: primary,
        ..
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(
        *stage,
        BackupStage::Copying,
        "the source listing runs in the copy stage"
    );
    let OnlineBackupFailure::Copy(CopyError::Io { path, source: io }) = primary else {
        panic!("expected CopyError::Io naming the unreadable entry, got {primary:?}");
    };
    assert_eq!(
        path, &failing,
        "the typed listing error must name the exact unreadable entry"
    );
    assert_eq!(io.kind(), ErrorKind::PermissionDenied);
    let rendered = error.stage_and_reason();
    assert!(
        rendered.contains("copying"),
        "the scheduled rendering must name the failed stage: {rendered}"
    );
    assert!(
        rendered.contains("copy-io-permission-denied"),
        "the scheduled rendering must name the source listing failure: {rendered}"
    );
    // No manifest was published: the owned snapshot child was removed
    // with every byte of the partial copy, so the destination root holds
    // nothing at all.
    assert_eq!(
        fs::read_dir(to.path()).expect("destination root").count(),
        0,
        "a failed run must publish nothing — in particular no manifest.json"
    );
    // Pre-fix behaviour (a silent drop) is impossible: without the
    // injected error the very same tree completes the protocol, and its
    // manifest describes exactly the six source files.
    let fresh = TempDir::new().expect("fresh destination root");
    let port = InjectPort::two_repos(source.path());
    let (result, _progress) = run_online(&port, source.path(), fresh.path(), &cancel);
    let report = result.expect("the same tree must complete without the injected metadata error");
    let raw = fs::read_to_string(report.dest_dir.join("manifest.json"))
        .expect("a clean run publishes the manifest");
    let decoded = manifest::decode(&raw).expect("decode the published manifest");
    let mut listed: Vec<&str> = decoded
        .files
        .iter()
        .map(|entry| entry.path.as_str())
        .collect();
    listed.sort_unstable();
    assert_eq!(
        listed,
        vec![
            "repos/alpha.shamirwal/00000000.meta",
            "repos/alpha.shamirwal/00000000.wal",
            "repos/alpha/store.bin",
            "repos/beta.shamirwal/00000000.meta",
            "repos/beta.shamirwal/00000000.wal",
            "repos/beta/store.bin",
        ],
        "every source file must be captured"
    );
}

/// A destination-side `create_dir_all` failure is a DESTINATION error,
/// never the tolerated vanished-source race (`CopyError::Io` with
/// `NotFound`, the only shape the pass classifies as `missing_files`).
///
/// Mutant: reverting to `fs::create_dir_all(parent).is_ok()` makes the
/// pass either report `Io` or succeed silently depending on the later
/// `File::create`, and the `Dest` assertion fails.
#[test]
fn destination_create_failure_is_a_destination_error() {
    let source = TempDir::new().expect("source temp dir");
    let dest = TempDir::new().expect("destination temp dir");
    write_file(&source.path().join("sub").join("a.bin"), b"payload-bytes");
    // Fixture act: a regular FILE occupies the destination directory the
    // pass must create for `sub/a.bin`.
    let blocker = dest.path().join("sub");
    fs::write(&blocker, b"not a directory").expect("plant destination blocker file");
    let cancel = CancelFlag::default();
    let error = run_copy_pass(
        source.path(),
        dest.path(),
        &[],
        &[],
        &cancel,
        &CopyOptions::default(),
    )
    .expect_err("a destination directory failure must fail the pass");
    match error {
        CopyError::Dest { path, .. } => {
            assert_eq!(
                path, blocker,
                "CopyError::Dest must name the destination parent it failed to create"
            );
        }
        CopyError::Io { .. } => {
            panic!("a destination failure must not be reported as a source Io error")
        }
        other => panic!("expected CopyError::Dest, got {other:?}"),
    }
    assert!(
        blocker.is_file(),
        "the destination blocker must be untouched by the failed pass"
    );
}

/// The atomic create of the snapshot child is a DESTINATION failure: a
/// missing destination root fails at `Preparing` with the protocol-level
/// destination reason, never as a generic io error, and nothing is owned
/// (no pin is acquired, so none can leak).
///
/// Mutant: keeping the `OnlineBackupFailure::Io` mapping makes the typed
/// `Dest` assertion and the `destination-io-<kind>` reason fail.
#[test]
fn destination_root_create_failure_is_a_destination_error() {
    let source = TempDir::new().expect("source temp dir");
    InjectPort::source_with_data(source.path());
    let parent = TempDir::new().expect("destination parent");
    // A destination root that does not exist: creating the timestamped
    // snapshot child under it fails with `NotFound` on every platform.
    let to = parent.path().join("absent-destination-root");
    let cancel = CancelFlag::default();
    let port = InjectPort::single_repo(source.path());
    let (result, _progress) = run_online(&port, source.path(), &to, &cancel);
    let error = result.expect_err("a destination create failure must fail the run");
    let OnlineBackupError::Failed {
        stage,
        source: primary,
        cleanup,
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Preparing);
    assert_eq!(
        *cleanup, None,
        "nothing was owned, so there is no cleanup failure to report"
    );
    let OnlineBackupFailure::Dest { source: io } = primary else {
        panic!("the snapshot child create failure must be a DESTINATION failure, got {primary:?}");
    };
    assert_eq!(io.kind(), ErrorKind::NotFound);
    assert_eq!(
        error.stage_and_reason(),
        "preparing: destination-io-not-found",
        "the scheduled reason must name the destination, never a source io error"
    );
    assert!(
        !to.exists(),
        "the failed create must leave no destination behind"
    );
    assert_eq!(port.pins_currently_held(), 0);
    assert_eq!(
        port.pins_released.load(Ordering::SeqCst),
        0,
        "no pin was acquired, so none may be released"
    );
}

/// The primary failure of a cleanup-fault run: the typed mid-file copy
/// error naming the SOURCE path, at `Copying`.
fn assert_primary_copy_failure(error: &OnlineBackupError, source_root: &Path) {
    let OnlineBackupError::Failed {
        stage,
        source: primary,
        ..
    } = error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Copying);
    let OnlineBackupFailure::Copy(CopyError::Io { path, source: io }) = primary else {
        panic!("the primary error must stay the typed copy failure, got {primary:?}");
    };
    assert_eq!(path, &source_root.join("repos/alpha/store.bin"));
    assert!(
        io.to_string().contains("injected mid-file copy failure"),
        "the primary error must stay the real copy cause, got: {io}"
    );
}

/// The cleanup failure attached to a failed run, if any.
fn reported_cleanup(error: &OnlineBackupError) -> Option<CleanupFailure> {
    match error {
        OnlineBackupError::Failed { cleanup, .. } => *cleanup,
        other => panic!("expected Failed, got {other:?}"),
    }
}

/// A mid-file copy failure plus a failed removal of the owned partial
/// destination.
fn install_copy_and_removal_faults() -> (SeamGuard, SeamGuard) {
    let writes: SeamGuard = test_seams::set_after_first_write(Box::new(|path: &Path| {
        if path.ends_with(PathBuf::from("repos/alpha/store.bin")) {
            Err(std::io::Error::other("injected mid-file copy failure"))
        } else {
            Ok(())
        }
    }));
    let removal: SeamGuard = test_seams::set_remove_dir_all(Box::new(|_path: &Path| {
        Some(Err(std::io::Error::new(
            ErrorKind::PermissionDenied,
            "injected partial-tree removal failure",
        )))
    }));
    (writes, removal)
}

/// A failed removal of the owned partial destination is reported
/// SEPARATELY: the primary copy failure stays the source error and the
/// cleanup failure is a bounded `CleanupFailure`. A successful removal
/// reports no cleanup failure at all, and — because each seam guard
/// clears ONLY its own hook — the write fault survives the removal
/// guard's drop.
///
/// Mutant 1: the old `let _ = fs::remove_dir_all(..)` reports
/// `cleanup: None` and the first assertion fails. Mutant 2: a
/// clear-everything seam guard lets the removal guard's drop wipe the
/// write fault, so the post-drop run SUCCEEDS and its `expect_err`
/// fails.
#[test]
fn cleanup_failure_is_reported_separately_from_the_primary_copy_failure() {
    let source = TempDir::new().expect("source temp dir");
    InjectPort::source_with_data(source.path());
    let cancel = CancelFlag::default();
    // Case 1: both faults live for the run.
    {
        let (writes, removal) = install_copy_and_removal_faults();
        let to = TempDir::new().expect("destination temp dir");
        let port = InjectPort::single_repo(source.path());
        let (result, _progress) = run_online(&port, source.path(), to.path(), &cancel);
        drop(removal);
        drop(writes);
        let error = result.expect_err("the injected copy failure must fail the run");
        assert_primary_copy_failure(&error, source.path());
        assert_eq!(
            reported_cleanup(&error),
            Some(CleanupFailure {
                kind: ErrorKind::PermissionDenied
            }),
            "the failed removal must be reported separately, not discarded"
        );
    }
    // Case 2 (negative control): BOTH faults are installed again in this
    // case's own scope, so the run fails for the primary copy reason and
    // still reports the failed removal.
    {
        let (writes, removal) = install_copy_and_removal_faults();
        let to = TempDir::new().expect("second destination temp dir");
        let port = InjectPort::single_repo(source.path());
        let (result, _progress) = run_online(&port, source.path(), to.path(), &cancel);
        drop(removal);
        drop(writes);
        let error = result.expect_err("the injected copy failure must fail the run");
        assert_primary_copy_failure(&error, source.path());
        assert_eq!(
            reported_cleanup(&error),
            Some(CleanupFailure {
                kind: ErrorKind::PermissionDenied
            }),
            "a failed removal with both faults installed still reports the cleanup failure"
        );
    }
    // Case 3: the primary copy failure with a SUCCESSFUL removal reports
    // no cleanup failure; the same write fault still fails a further run
    // after the removal guard is dropped.
    {
        let writes = test_seams::set_after_first_write(Box::new(|path: &Path| {
            if path.ends_with(PathBuf::from("repos/alpha/store.bin")) {
                Err(std::io::Error::other("injected mid-file copy failure"))
            } else {
                Ok(())
            }
        }));
        let removal: SeamGuard =
            test_seams::set_remove_dir_all(Box::new(|_path: &Path| Some(Ok(()))));
        let to = TempDir::new().expect("third destination temp dir");
        let port = InjectPort::single_repo(source.path());
        let (result, _progress) = run_online(&port, source.path(), to.path(), &cancel);
        drop(removal);
        let error = result.expect_err("the injected copy failure must fail the run");
        assert_primary_copy_failure(&error, source.path());
        assert_eq!(
            reported_cleanup(&error),
            None,
            "a successful removal must report no cleanup failure"
        );
        // Independence oracle: the removal guard's drop must NOT clear
        // the write fault (the old clear-everything guard cleared it, so
        // this run would succeed).
        let to = TempDir::new().expect("fourth destination temp dir");
        let port = InjectPort::single_repo(source.path());
        let (result, _progress) = run_online(&port, source.path(), to.path(), &cancel);
        drop(writes);
        let error = result.expect_err("the write fault must survive the removal guard's drop");
        assert_primary_copy_failure(&error, source.path());
        assert_eq!(reported_cleanup(&error), None);
    }
}

/// A manifest publication (rename) failure is a DESTINATION failure at
/// `Verifying`: the typed protocol-level destination reason, no source
/// path in the rendering, and the owned snapshot directory is removed
/// again (so no `manifest.json` is left behind anywhere).
///
/// Mutant: mapping the publication failure back to
/// `OnlineBackupFailure::Io` makes the `Dest` and the
/// `destination-io-<kind>` assertions fail; swallowing the publication
/// error would let the run report success with no manifest.
#[test]
fn manifest_publication_failure_is_a_destination_error() {
    let source = TempDir::new().expect("source temp dir");
    InjectPort::source_with_data(source.path());
    let seam: SeamGuard = test_seams::set_manifest_rename(Box::new(|target: &Path| {
        if target.ends_with("manifest.json") {
            Some(Err(std::io::Error::new(
                ErrorKind::PermissionDenied,
                "injected manifest rename failure",
            )))
        } else {
            None
        }
    }));
    let to = TempDir::new().expect("destination temp dir");
    let port = InjectPort::single_repo(source.path());
    let cancel = CancelFlag::default();
    let (result, _progress) = run_online(&port, source.path(), to.path(), &cancel);
    drop(seam);
    let error = result.expect_err("a failed manifest rename must fail the run");
    let OnlineBackupError::Failed {
        stage,
        source: primary,
        cleanup,
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Verifying);
    assert_eq!(*cleanup, None, "the owned directory was removed normally");
    let OnlineBackupFailure::Dest { source: io } = primary else {
        panic!("the manifest publication failure must be a DESTINATION failure, got {primary:?}");
    };
    assert_eq!(io.kind(), ErrorKind::PermissionDenied);
    let rendered = error.stage_and_reason();
    assert_eq!(
        rendered, "verifying: destination-io-permission-denied",
        "the scheduled reason must be the destination vocabulary token"
    );
    assert!(
        !rendered.contains("repos") && !rendered.contains('/') && !rendered.contains('\\'),
        "no path may reach the scheduled rendering: {rendered}"
    );
    assert_eq!(
        fs::read_dir(to.path()).expect("destination root").count(),
        0,
        "the failed run must remove its owned snapshot directory entirely"
    );
    assert_eq!(port.pins_currently_held(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
}

/// A directory fsync failure is a DESTINATION failure naming the
/// DIRECTORY (never the source), and the owned snapshot directory is
/// still removed.
///
/// Mutant: a best-effort directory fsync swallows the error, the run
/// succeeds and the typed `Dest` assertion never runs.
#[test]
fn directory_fsync_failure_is_a_destination_error() {
    let source = TempDir::new().expect("source temp dir");
    InjectPort::source_with_data(source.path());
    let seam: SeamGuard = test_seams::set_fsync_dir(Box::new(|path: &Path| {
        if path.ends_with("alpha.shamirwal") {
            Some(Err(std::io::Error::new(
                ErrorKind::PermissionDenied,
                "injected directory fsync failure",
            )))
        } else {
            None
        }
    }));
    let to = TempDir::new().expect("destination temp dir");
    let port = InjectPort::single_repo(source.path());
    let cancel = CancelFlag::default();
    let (result, _progress) = run_online(&port, source.path(), to.path(), &cancel);
    drop(seam);
    let error = result.expect_err("a failed directory fsync must fail the run");
    let OnlineBackupError::Failed {
        stage,
        source: primary,
        ..
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Copying);
    let OnlineBackupFailure::Copy(CopyError::Dest { path, source: io }) = primary else {
        panic!("the directory fsync failure must be a DESTINATION copy failure, got {primary:?}");
    };
    assert_eq!(io.kind(), ErrorKind::PermissionDenied);
    assert!(
        path.ends_with("alpha.shamirwal"),
        "CopyError::Dest must name the destination directory it failed to fsync, got {path:?}"
    );
    assert!(
        path.starts_with(to.path()),
        "the named path must be inside the destination root, never the source, got {path:?}"
    );
    assert!(
        !path.starts_with(source.path()),
        "a destination failure must never be attributed to a source path, got {path:?}"
    );
    assert_eq!(
        error.stage_and_reason(),
        "copying: copy-destination-io-permission-denied",
        "the scheduled reason must be the destination copy token"
    );
    assert_eq!(
        fs::read_dir(to.path()).expect("destination root").count(),
        0,
        "the failed run must remove its owned snapshot directory"
    );
    assert_eq!(port.pins_currently_held(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
}

/// A filesystem that cannot fsync a directory (`Unsupported` /
/// `InvalidInput`) is NOT a destination failure: the run completes and
/// publishes its manifest.
///
/// Mutant: dropping the documented tolerance fails every run on such a
/// filesystem, so the run below returns `Err` instead of a report.
#[test]
fn unsupported_directory_fsync_stays_tolerated() {
    let source = TempDir::new().expect("source temp dir");
    InjectPort::source_with_data(source.path());
    let seam: SeamGuard = test_seams::set_fsync_dir(Box::new(|path: &Path| {
        if path.ends_with("alpha.shamirwal") {
            Some(Err(std::io::Error::from(ErrorKind::Unsupported)))
        } else if path.ends_with("beta.shamirwal") {
            Some(Err(std::io::Error::from(ErrorKind::InvalidInput)))
        } else {
            None
        }
    }));
    let to = TempDir::new().expect("destination temp dir");
    let port = InjectPort::two_repos(source.path());
    let cancel = CancelFlag::default();
    let (result, _progress) = run_online(&port, source.path(), to.path(), &cancel);
    drop(seam);
    let report = result.expect("an unsupported directory fsync must stay tolerated");
    assert!(
        report.dest_dir.join("manifest.json").is_file(),
        "the tolerated fsync must not prevent publishing the manifest"
    );
    assert_eq!(port.pins_currently_held(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
}

/// The SCHEDULED rendering of a destination copy failure is the stage
/// word plus bounded tokens: no path, no path separator, no OS text.
///
/// Mutant: rendering the `io::Error` (or the `CopyError`) via `Display`
/// puts the canary path/OS text into the string and the assertions fail.
#[test]
fn stage_and_reason_is_stage_and_kind_without_paths() {
    let error = OnlineBackupError::Failed {
        stage: BackupStage::Copying,
        source: OnlineBackupFailure::Copy(CopyError::Dest {
            path: PathBuf::from("CANARY-destination/sub/secret.bin"),
            source: std::io::Error::new(ErrorKind::PermissionDenied, "CANARY-os-error-text"),
        }),
        cleanup: Some(CleanupFailure {
            kind: ErrorKind::PermissionDenied,
        }),
    };
    let rendered = error.stage_and_reason();
    assert_eq!(
        rendered,
        "copying: copy-destination-io-permission-denied; cleanup-failed-permission-denied",
        "the rendering must be the stage plus the fixed reason tokens"
    );
    assert!(
        !rendered.contains("CANARY"),
        "no path or OS text may reach the scheduled rendering: {rendered}"
    );
    assert!(
        !rendered.contains('/') && !rendered.contains('\\'),
        "no path separator may reach the scheduled rendering: {rendered}"
    );
    // A cancelled run carries the same optional cleanup failure, and the
    // spawn-failure helper shares the identical path-free vocabulary.
    let cancelled = OnlineBackupError::Cancelled {
        stage: BackupStage::Copying,
        cleanup: Some(CleanupFailure {
            kind: ErrorKind::PermissionDenied,
        }),
    };
    assert_eq!(
        cancelled.stage_and_reason(),
        "copying: cancelled; cleanup-failed-permission-denied"
    );
    assert_eq!(
        stage_and_io_reason(BackupStage::Preparing, ErrorKind::PermissionDenied),
        "preparing: io-permission-denied"
    );
}
