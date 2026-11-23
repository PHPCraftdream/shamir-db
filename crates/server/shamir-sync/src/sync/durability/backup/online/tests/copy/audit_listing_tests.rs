//! The audit chain listing and its capture rounds: an archive that appears
//! between rounds forces another round (bounded), an endless supply of them
//! fails the pass typed, and an unreadable entry is a typed refusal instead
//! of a silently hidden archive. The faults are injected through the
//! `#[cfg(test)]` seams of `copy_engine::test_seams`, so an injected result
//! travels the same production match arm as a genuine one.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::super::failure_injection_tests::{run as run_online, write_file, InjectPort};
use super::audit_fixtures;
use super::audit_fixtures::archive_name;
use crate::sync::durability::audit_chain_files::list_audit_files;
use crate::sync::durability::backup::online::copy_engine::test_seams;
use crate::sync::durability::backup::online::copy_engine::{
    run_data_dir_copy_pass, CopyError, CopyOptions,
};
use crate::sync::durability::backup::online::progress::{BackupStage, CancelFlag};
use crate::sync::durability::backup::online::protocol::{OnlineBackupError, OnlineBackupFailure};

/// One late rotation archive forces exactly one more round, and the
/// PUBLICATION describes it.
///
/// The `after_capture` hook fires once per captured file (phase-1 files
/// included), so it acts only on the SECOND capture of the fixture's archive —
/// the residual pass E, the pass whose result defines the manifest — and
/// writes the NEXT archive into the source with no appender involved.
///
/// Mutant caught: a data-directory pass that captures ONE round and never
/// re-lists (the single-listing order) — the late archive is absent from the
/// decoded manifest and from the destination while the run still reports
/// success, so the manifest assertions fail.
#[test]
fn one_late_archive_forces_another_round_and_lands_in_the_snapshot() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    write_file(&source.join("repos/alpha/store.bin"), b"store-bytes");
    let first = archive_name(1);
    let first_for_hook = first.clone();
    let captures = Arc::new(AtomicUsize::new(0));
    let captures_for_hook = captures.clone();
    let source_for_hook = source.to_path_buf();
    let seam = test_seams::set_after_capture(Box::new(move |path: &Path| {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if name != first_for_hook || captures_for_hook.fetch_add(1, Ordering::SeqCst) != 1 {
            return;
        }
        write_file(&source_for_hook.join(archive_name(2)), b"archived-2\n");
    }));
    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run_online(&port, source, to, &CancelFlag::default());
    drop(seam);
    let report = result.expect("the round that re-lists the late archive must succeed");
    assert_eq!(
        captures.load(Ordering::SeqCst),
        2,
        "the hook must act in the residual pass, once"
    );
    let paths = audit_fixtures::manifest_paths(&report.dest_dir);
    for expected in [first.clone(), archive_name(2)] {
        assert!(
            paths.contains(&expected),
            "the published manifest must list {expected}: {paths:?}"
        );
    }
    let bytes = b"archived-2\n";
    assert_eq!(
        fs::read(to.join(archive_name(2))).expect("late archive copy"),
        bytes,
        "the late archive must be copied to the destination"
    );
    assert_eq!(
        audit_fixtures::manifest_entry(&report.dest_dir, &archive_name(2)),
        (hex::encode(Sha256::digest(bytes)), bytes.len() as u64),
        "the manifest must describe the late archive bytes exactly"
    );
}

/// A source that keeps producing archives is refused after exactly FOUR
/// rounds.
///
/// The round boundary is counted INDEPENDENTLY of the production bound: the
/// active log is captured last in every audit round, so its capture count is
/// the number of rounds the pass entered. The reported value is matched
/// against the literal 4, never against the constant that drives the loop, so
/// raising the bound to five (or running a fifth round while still reporting
/// four) turns this test red.
///
/// Mutant caught: an unstable listing tolerated as if it were stable (the
/// pass returns the last round and `Ok`) — `expect_err` fails; a bound other
/// than four — the literal assertions fail.
#[test]
fn audit_files_that_keep_appearing_fail_the_pass_after_four_rounds() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    write_file(&source.join(archive_name(1)), b"archived-1\n");
    write_file(&source.join("audit.log"), b"active\n");
    let rounds_seen = Arc::new(AtomicUsize::new(0));
    let rounds_for_hook = rounds_seen.clone();
    let next = Arc::new(AtomicU64::new(2));
    let next_for_hook = next.clone();
    let source_for_hook = source.to_path_buf();
    let seam = test_seams::set_after_capture(Box::new(move |path: &Path| {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if name == "audit.log" {
            // Captured last in every audit round: one round boundary.
            rounds_for_hook.fetch_add(1, Ordering::SeqCst);
            return;
        }
        if !name.starts_with("audit.log.") {
            return;
        }
        let index = next_for_hook.fetch_add(1, Ordering::SeqCst);
        write_file(&source_for_hook.join(archive_name(index)), b"late\n");
    }));
    let result = run_data_dir_copy_pass(
        source,
        to,
        &[],
        &CancelFlag::default(),
        &CopyOptions::default(),
    );
    drop(seam);
    let error = result.expect_err("an endlessly appearing archive must fail the pass typed");
    assert_eq!(
        rounds_seen.load(Ordering::SeqCst),
        4,
        "the pass must enter exactly four audit rounds"
    );
    match error {
        CopyError::AuditFilesUnstable { rounds } => {
            assert_eq!(rounds, 4, "the refusal must report four rounds");
        }
        other => panic!("expected AuditFilesUnstable, got {other:?}"),
    }
}

/// One unreadable audit entry is a typed, path-free refusal.
///
/// `list_audit_files` is the ONE listing both the capture round and the
/// stability check use, so an entry dropped on a metadata error hides a
/// possibly-new archive from BOTH listings and the snapshot silently lacks
/// it.
///
/// Mutant caught: the parent's `Err(_) => continue` (silent omission), and,
/// for the same reason, a listing that never consults the injection: the
/// parent returns `Ok` and `expect_err` fails.
#[test]
fn an_unreadable_audit_entry_is_a_typed_refusal_not_a_hidden_archive() {
    let source_dir = TempDir::new().expect("source temp dir");
    let source = source_dir.path();
    write_file(&source.join(archive_name(1)), b"archived\n");
    write_file(&source.join("audit.log"), b"active\n");
    let cancel = CancelFlag::default();
    // Control: without the fault the listing names the archive first and the
    // active log last.
    let listing = list_audit_files(source, &cancel, None).expect("the fixture must list");
    let names: Vec<String> = listing
        .iter()
        .map(|path| {
            path.file_name()
                .expect("listed name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        names,
        vec![archive_name(1), "audit.log".to_string()],
        "the fixture must list the archive before the active log"
    );
    let failing = source.join(archive_name(1));
    let seam = test_seams::set_symlink_metadata(Box::new(move |path: &Path| {
        if path == failing {
            Some(Err(std::io::Error::new(
                ErrorKind::PermissionDenied,
                "listing denied",
            )))
        } else {
            None
        }
    }));
    let error = list_audit_files(source, &cancel, None)
        .expect_err("an unreadable audit entry must be typed, never skipped");
    drop(seam);
    match &error {
        CopyError::AuditListing { source } => {
            assert_eq!(
                source.kind(),
                ErrorKind::PermissionDenied,
                "the typed refusal must carry the metadata error"
            );
        }
        other => panic!("expected AuditListing, got {other:?}"),
    }
    let rendered = error.to_string();
    assert!(
        !rendered.contains("audit.log"),
        "the copy error must not echo the entry name: {rendered}"
    );
    assert!(
        !rendered.contains('/') && !rendered.contains('\\'),
        "no path separator may reach the message: {rendered}"
    );
}

/// A PERSISTENT metadata fault on a top-level archive fails the WHOLE run
/// with the typed, path-free refusal — phase 1 included.
///
/// Phase 1 lists the data dir before the audit phase exists and reads the
/// entry's metadata BEFORE applying the phase-1 audit-name exclusion; without
/// the classification in `copy_engine::list_files` that failure leaves as the
/// path-bearing `CopyError::Io`, which a manual run's `last_error` renders
/// with the filesystem path.
///
/// Mutant caught: classifying an unreadable reserved audit name as
/// `CopyError::Io` — the failure-type assertion fails and the canary (the
/// archive name and the fixture directory) appears in the rendered chain.
#[test]
fn a_persistent_metadata_fault_on_an_archive_fails_the_run_path_free() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    write_file(&source.join("repos/alpha/store.bin"), b"store-bytes");
    let archive = archive_name(1);
    write_file(&source.join(&archive), b"archived\n");
    write_file(&source.join("audit.log"), b"active\n");
    let failing = source.join(&archive);
    let seam = test_seams::set_symlink_metadata(Box::new(move |path: &Path| {
        if path == failing {
            Some(Err(std::io::Error::new(
                ErrorKind::PermissionDenied,
                "listing denied",
            )))
        } else {
            None
        }
    }));
    let port = InjectPort::single_repo(source);
    let (result, progress) = run_online(&port, source, to, &CancelFlag::default());
    drop(seam);
    let error = result.expect_err("a persistent metadata fault must fail the run");
    let OnlineBackupError::Failed {
        stage,
        source: primary,
        ..
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Copying);
    assert!(
        matches!(
            primary,
            OnlineBackupFailure::Copy(CopyError::AuditListing { .. })
        ),
        "the phase-1 refusal must be the typed audit listing error, got {primary:?}"
    );
    // The manual `last_error` is the Display chain (server
    // `full_error_chain`); build the same text here.
    let mut rendered = error.to_string();
    let mut link = std::error::Error::source(&error);
    while let Some(cause) = link {
        rendered.push_str(": ");
        rendered.push_str(&cause.to_string());
        link = cause.source();
    }
    let manual = progress.last_error.as_deref().expect("last_error");
    let scheduled = error.stage_and_reason();
    let fixture_dir = source_dir.path().display().to_string();
    for text in [rendered.as_str(), manual, scheduled.as_str()] {
        assert!(
            !text.contains(&archive),
            "no rendered refusal may name the archive: {text}"
        );
        assert!(
            !text.contains(&fixture_dir),
            "no rendered refusal may carry the fixture path: {text}"
        );
    }
    assert_eq!(
        scheduled, "copying: copy-io-permission-denied",
        "the scheduled token must stay in the path-free copy vocabulary"
    );
}

/// An archive copied by an UNSTABLE round and then pruned by the writer is
/// reconciled away before publication.
///
/// Retention deletes archives, so an archive captured by round 1 can be gone
/// from the source when the accepted round lists the directory. Its
/// destination copy would then have no manifest entry and the run's own
/// self-verification would reject the snapshot as holding an unlisted file.
///
/// Mutant caught: reconciling only the active log (the previous
/// `remove_uncaptured_active_log`) — the pruned archive stays in the
/// destination, the run fails at `Verifying` and the manifest assertions
/// never run; the in-hook assertion below also pins that the archive really
/// reached the destination first, so the test cannot pass vacuously.
#[test]
fn an_archive_pruned_between_rounds_leaves_no_unmanifested_destination_copy() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    write_file(&source.join("repos/alpha/store.bin"), b"store-bytes");
    let pruned = archive_name(1);
    let late = archive_name(2);
    write_file(&source.join(&pruned), b"archived-1\n");
    write_file(&source.join("audit.log"), b"active\n");
    let pruned_for_hook = pruned.clone();
    let source_for_hook = source.to_path_buf();
    let dest_for_hook = to.to_path_buf();
    let late_for_hook = late.clone();
    let pruned_once = Arc::new(AtomicBool::new(false));
    let pruned_for_flag = pruned_once.clone();
    let seam = test_seams::set_after_capture(Box::new(move |path: &Path| {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if name != pruned_for_hook || pruned_for_flag.swap(true, Ordering::SeqCst) {
            return;
        }
        // The archive really reached the destination before it was pruned.
        assert!(
            dest_for_hook.join(&pruned_for_hook).exists(),
            "the pruned archive must have been copied first"
        );
        fs::remove_file(source_for_hook.join(&pruned_for_hook))
            .expect("retention prunes the archive");
        write_file(&source_for_hook.join(&late_for_hook), b"archived-2\n");
    }));
    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run_online(&port, source, to, &CancelFlag::default());
    drop(seam);
    let report = result.expect("a pruned archive must not fail the run at self-verification");
    assert!(
        pruned_once.load(Ordering::SeqCst),
        "the retention step must run"
    );
    let paths = audit_fixtures::manifest_paths(&report.dest_dir);
    assert!(
        paths.contains(&late),
        "the archive the writer left must be published: {paths:?}"
    );
    assert!(
        !paths.contains(&pruned),
        "the pruned archive must not be published: {paths:?}"
    );
    let dest_pruned = report.dest_dir.join(&pruned);
    assert!(
        !dest_pruned.exists(),
        "the unmanifested copy of the pruned archive must be removed: {}",
        dest_pruned.display()
    );
    assert_eq!(
        fs::read(report.dest_dir.join(&late)).expect("late archive copy"),
        b"archived-2\n"
    );
}
