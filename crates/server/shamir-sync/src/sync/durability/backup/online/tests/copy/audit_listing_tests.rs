//! The audit chain listing and its capture rounds: an archive that appears
//! between rounds forces another round (bounded), an endless supply of them
//! fails the pass typed, and an unreadable entry is a typed refusal instead
//! of a silently hidden archive. The faults are injected through the
//! `#[cfg(test)]` seams of `copy_engine::test_seams`, so an injected result
//! travels the same production match arm as a genuine one.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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
/// `after_capture` fires once per captured file. Archive 1 is captured THREE
/// times: once in pass D, once in the residual pass E's first audit round —
/// where the hook injects archive 2 — and once in E's retry. The injection is
/// counted separately from the captures, so a fixture that injects in the
/// wrong pass cannot pass as "two captures".
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
    audit_fixtures::seed_alpha_repo(source);
    let first = archive_name(1);
    let first_for_hook = first.clone();
    let captures = Arc::new(AtomicUsize::new(0));
    let captures_for_hook = captures.clone();
    let injected = Arc::new(AtomicBool::new(false));
    let injected_for_hook = injected.clone();
    let source_for_hook = source.to_path_buf();
    let seam = test_seams::set_after_capture(Box::new(move |path: &Path| {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if name != first_for_hook {
            return;
        }
        // Pass D captures it first; the residual pass E's first round is the
        // second capture, and that is where the late archive appears.
        if captures_for_hook.fetch_add(1, Ordering::SeqCst) == 1
            && !injected_for_hook.swap(true, Ordering::SeqCst)
        {
            write_file(&source_for_hook.join(archive_name(2)), b"archived-2\n");
        }
    }));
    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run_online(&port, source, to, &CancelFlag::default());
    drop(seam);
    let report = result.expect("the round that re-lists the late archive must succeed");
    assert!(
        injected.load(Ordering::SeqCst),
        "the residual pass must inject the late archive"
    );
    assert_eq!(
        captures.load(Ordering::SeqCst),
        3,
        "archive 1 is captured in pass D, in E's first round and in E's retry"
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
        fs::read(report.dest_dir.join(archive_name(2))).expect("late archive copy"),
        bytes,
        "the late archive must be copied into the run directory"
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
    audit_fixtures::seed_alpha_repo(source);
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

/// An archive THIS pass captured in an earlier round and retention then
/// pruned refuses the run.
///
/// The anchors are captured by phase 1 of the manifest-defining pass (step E of
/// `online/protocol.rs`) BEFORE any audit listing. An audit round of that same
/// pass then copies archive 1; retention prunes it from the source before the
/// accepted round lists the directory. Reconciling the destination would delete
/// the copy of an archive the captured anchors still name — publication would
/// pass (it hashes files, it does not read anchors) and boot would refuse the
/// snapshot with `UnanchoredSuffix`. The pass must refuse instead of turning a
/// refused backup into an unbootable success, and it must refuse TYPED and
/// path-free: `CopyError::AuditArchiveLost` under the stable reason token.
///
/// The retention step runs inside the capture hook, immediately after the
/// SECOND capture of archive 1: the first is pass D's audit round (a harmless
/// earlier pass), the second is a round of pass E. The hook itself is the
/// rendezvous — no sleep, no channel.
///
/// Mutant caught: reconciling every destination audit name the accepted round
/// did not capture, without asking whether an earlier round of the SAME pass
/// copied it (the branch before `AuditArchiveLost`) — the run succeeds and
/// leaves a published snapshot whose anchors need the deleted archive.
#[test]
fn an_archive_captured_by_an_earlier_round_that_retention_pruned_refuses_the_run() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    audit_fixtures::seed_alpha_repo(source);
    let pruned = archive_name(1);
    let late = archive_name(2);
    write_file(&source.join(&pruned), b"archived-1\n");
    write_file(&source.join("audit.log"), b"active\n");
    let pruned_for_hook = pruned.clone();
    let source_for_hook = source.to_path_buf();
    let late_for_hook = late.clone();
    let captures = Arc::new(AtomicUsize::new(0));
    let captures_for_hook = captures.clone();
    let pruned_once = Arc::new(AtomicBool::new(false));
    let pruned_for_flag = pruned_once.clone();
    let seam = test_seams::set_after_capture(Box::new(move |path: &Path| {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if name != pruned_for_hook {
            return;
        }
        // Pass D's audit round captures it first, pass E's first round the
        // second time. Retention runs between that round and the accepted one.
        if captures_for_hook.fetch_add(1, Ordering::SeqCst) + 1 != 2 {
            return;
        }
        assert!(
            !pruned_for_flag.swap(true, Ordering::SeqCst),
            "the retention step must run exactly once"
        );
        fs::remove_file(source_for_hook.join(&pruned_for_hook))
            .expect("retention prunes the archive");
        write_file(&source_for_hook.join(&late_for_hook), b"archived-2\n");
    }));
    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run_online(&port, source, to, &CancelFlag::default());
    drop(seam);
    assert!(
        pruned_once.load(Ordering::SeqCst),
        "the retention step must run"
    );
    assert_eq!(
        captures.load(Ordering::SeqCst),
        2,
        "pass D and pass E's first round capture archive 1; the refusal precedes a third"
    );
    let error = result.expect_err("losing a captured archive must refuse the run");
    let OnlineBackupError::Failed {
        stage,
        source: primary,
        cleanup,
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Sealing);
    assert_eq!(
        *cleanup, None,
        "the destination removal succeeds, so there is no cleanup failure"
    );
    assert!(
        matches!(
            primary,
            OnlineBackupFailure::Copy(CopyError::AuditArchiveLost)
        ),
        "the refusal must be the typed lost-archive error, got {primary:?}"
    );
    assert_eq!(
        error.stage_and_reason(),
        "sealing: copy-audit-archive-lost",
        "the scheduled token must stay in the path-free copy vocabulary"
    );
    let rendered = format!("{primary}");
    assert!(
        !rendered.contains(&pruned) && !rendered.contains(&late),
        "the refusal must not name any archive: {rendered}"
    );
    assert_eq!(
        fs::read_dir(to).expect("destination root").count(),
        0,
        "R4: the refused snapshot must not survive in the destination"
    );
}

/// An archive pruned BEFORE the manifest-defining pass captured it is
/// reconciled away, and the run still publishes a bootable snapshot.
///
/// Pass D's audit round copies archive 1 into the destination. Retention then
/// prunes it and a rotation leaves archive 2 — both at the D to E boundary, so
/// no audit round of pass E ever captured archive 1. Its destination copy has
/// no manifest entry, and reconciliation must delete it or self-verification
/// refuses the snapshot as holding an unlisted file. A prefix that was already
/// gone before any round of the manifest-defining pass listed it is the
/// documented pre-existing limit (the anchors a rotation advanced can only name
/// archives retention keeps).
///
/// The D to E boundary hook is the rendezvous: it runs after pass D's audit
/// round and before pass E's first listing, and it records the run directory it
/// found so the destination assertions below are pinned to `report.dest_dir`.
///
/// Mutant caught: reconciling only the ACTIVE log (the previous
/// `remove_uncaptured_active_log`) — the stale archive copy survives, the run
/// fails at `Verifying` and the manifest assertions never run.
#[test]
fn an_archive_pruned_before_the_manifest_defining_pass_is_reconciled_away() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    audit_fixtures::seed_alpha_repo(source);
    let pruned = archive_name(1);
    let late = archive_name(2);
    write_file(&source.join(&pruned), b"archived-1\n");
    write_file(&source.join("audit.log"), b"active\n");
    let pruned_for_hook = pruned.clone();
    let source_for_hook = source.to_path_buf();
    let late_for_hook = late.clone();
    let to_for_hook = to.to_path_buf();
    let observed = Arc::new(Mutex::new(None::<PathBuf>));
    let observed_for_hook = observed.clone();
    let pruned_once = Arc::new(AtomicBool::new(false));
    let pruned_for_flag = pruned_once.clone();
    let seam = test_seams::set_before_boundary(Box::new(move |stage: BackupStage| {
        if stage != BackupStage::Sealing || pruned_for_flag.swap(true, Ordering::SeqCst) {
            return;
        }
        // Pass D's audit round really copied the archive into the run
        // directory before retention pruned it.
        let run_dir = fs::read_dir(&to_for_hook)
            .expect("destination root")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|path| path.join(&pruned_for_hook).is_file())
            .unwrap_or_else(|| {
                panic!("the pruned archive must have been copied into the run directory first")
            });
        *observed_for_hook.lock().expect("observed run dir") = Some(run_dir);
        fs::remove_file(source_for_hook.join(&pruned_for_hook))
            .expect("retention prunes the archive");
        write_file(&source_for_hook.join(&late_for_hook), b"archived-2\n");
    }));
    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run_online(&port, source, to, &CancelFlag::default());
    drop(seam);
    assert!(
        pruned_once.load(Ordering::SeqCst),
        "the retention step must run"
    );
    let report = result.expect("a prefix gone before pass E listed it must not fail the run");
    let observed = observed
        .lock()
        .expect("observed run dir")
        .clone()
        .expect("the boundary hook must have run");
    assert_eq!(
        observed, report.dest_dir,
        "the destination assertions must address the run directory the protocol created"
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

/// An ENUMERATION failure of the data-directory root is the typed, path-free
/// audit refusal — a different arm from failing to OPEN the root.
///
/// Phase 1 of the data-directory pass is the one listing whose root owns the
/// audit chain files, so a dropped entry there could hide a rotation archive:
/// the failure must be `CopyError::AuditListing` (`copy-io-<kind>`, no path).
/// Failing to open the root itself keeps the path-bearing `CopyError::Io` —
/// the operator needs the directory name — which the arm order in
/// `list_files` pins (`read_dir` fails before any entry is yielded).
///
/// Mutant caught: classifying the iterator failure as `CopyError::Io` — the
/// typed assertion fails and the rendered refusal carries the fixture path.
#[test]
fn an_iterator_failure_of_the_data_dir_root_is_a_typed_path_free_refusal() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    audit_fixtures::seed_alpha_repo(source);
    write_file(&source.join("audit.log"), b"active\n");
    let root = source.to_path_buf();
    let seam = test_seams::set_read_dir_entry(Box::new(move |dir: &Path| {
        if dir == root.as_path() {
            Some(std::io::Error::new(
                ErrorKind::PermissionDenied,
                "iteration denied",
            ))
        } else {
            None
        }
    }));
    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run_online(&port, source, to, &CancelFlag::default());
    drop(seam);
    let error = result.expect_err("an enumeration failure must fail the run");
    let OnlineBackupError::Failed {
        stage,
        source: primary,
        ..
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Copying);
    let OnlineBackupFailure::Copy(CopyError::AuditListing { source }) = primary else {
        panic!(
            "the data-root enumeration failure must be the typed audit listing, got {primary:?}"
        );
    };
    assert_eq!(
        source.kind(),
        ErrorKind::PermissionDenied,
        "the typed refusal must carry the enumeration error"
    );
    assert_eq!(
        error.stage_and_reason(),
        "copying: copy-io-permission-denied",
        "the scheduled token must stay in the path-free copy vocabulary"
    );
    let rendered = format!("{primary}");
    let fixture_dir = source_dir.path().display().to_string();
    assert!(
        !rendered.contains(&fixture_dir) && !rendered.contains("audit.log"),
        "the enumeration refusal must carry no path and no entry name: {rendered}"
    );
}

/// A file that merely happens to be named `audit.log` under a WAL root is an
/// ordinary WAL file: its metadata failure keeps the path-bearing
/// `CopyError::Io`.
///
/// The audit classification is scoped to phase 1 of the DATA-directory pass
/// (`skip_audit_files`). Every other listing — the WAL directory here — is an
/// `AuditFiles::InOrder` pass whose root is not the audit root, so a reserved
/// name in it is not an audit chain file and its unreadable metadata must name
/// the path the operator has to fix.
///
/// Mutant caught: classifying by name at EVERY pass root (the ungated
/// `dir == source_root && is_audit_log_file_name` condition) — the failure
/// becomes the path-free `CopyError::AuditListing` and the typed `Io` naming
/// the WAL file fails.
#[test]
fn a_wal_root_file_named_audit_log_stays_a_plain_io_refusal() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    audit_fixtures::seed_alpha_repo(source);
    let wal_audit_log = source.join("repos/alpha.shamirwal/audit.log");
    write_file(&wal_audit_log, b"not-an-audit-chain-file\n");
    let failing = wal_audit_log.clone();
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
    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run_online(&port, source, to, &CancelFlag::default());
    drop(seam);
    let error = result.expect_err("an unreadable WAL file must fail the run");
    let OnlineBackupError::Failed {
        stage,
        source: primary,
        ..
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Copying);
    let OnlineBackupFailure::Copy(CopyError::Io { path, source }) = primary else {
        panic!("a WAL file named audit.log must stay a plain io failure, got {primary:?}");
    };
    assert_eq!(
        path, &wal_audit_log,
        "the ordinary listing failure must name the exact unreadable file"
    );
    assert_eq!(source.kind(), ErrorKind::PermissionDenied);
}
