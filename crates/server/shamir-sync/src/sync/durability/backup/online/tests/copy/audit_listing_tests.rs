//! The audit chain listing and its capture rounds: an archive that appears between rounds forces another round (bounded), an endless supply of them fails the pass typed, and an unreadable entry
//! is a typed refusal instead of a silently hidden archive. The faults are injected through the `#[cfg(test)]` seams of `copy_engine::test_seams`, so an injected result travels the same
//! production match arm as a genuine one.

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

/// One late rotation archive forces exactly one more round, and the PUBLICATION describes it.
///
/// `after_capture` fires once per captured file. Archive 1 is captured THREE times: once in pass D, once in the residual pass E's first audit round — where the hook injects archive 2 — and
/// once in E's retry. The injection is counted separately from the captures, so a fixture that injects in the wrong pass cannot pass as "two captures".
///
/// The injection is armed by the SECOND data-directory listing (`after_listing` fires once per pass), never by a capture count the protocol does not promise.
///
/// Mutant caught: a data-directory pass that captures ONE round and never re-lists (the single-listing order) — the late archive is absent from the decoded manifest and from the destination
/// while the run still reports success, so the manifest assertions fail.
#[test]
fn one_late_archive_forces_another_round_and_lands_in_the_snapshot() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    audit_fixtures::seed_alpha_repo(source);
    let first = archive_name(1);
    // The SUBJECT of this test: a rotation archive that exists before the run.
    // Without it the hook below never fires (it is gated on capturing this
    // file), and D/E would see an empty audit inventory.
    write_file(&source.join(&first), b"archived-1\n");
    write_file(&source.join("audit.log"), b"active\n");
    let first_for_hook = first.clone();
    let captures = Arc::new(AtomicUsize::new(0));
    let captures_for_hook = captures.clone();
    let injected = Arc::new(AtomicBool::new(false));
    let injected_for_hook = injected.clone();
    let source_for_hook = source.to_path_buf();
    // Armed by the SECOND data-directory listing: phase 1 of pass D is the
    // first, phase 1 of the residual pass E the second. Everything captured
    // after the arming happens inside the pass that defines the manifest.
    let armed = Arc::new(AtomicBool::new(false));
    let armed_for_listing = armed.clone();
    let listings = Arc::new(AtomicUsize::new(0));
    let listings_for_hook = listings.clone();
    let arm = test_seams::set_after_listing(Box::new(move |_root: &Path| {
        if listings_for_hook.fetch_add(1, Ordering::SeqCst) == 1 {
            armed_for_listing.store(true, Ordering::SeqCst);
        }
    }));
    let armed_for_hook = armed.clone();
    let seam = test_seams::set_after_capture(Box::new(move |path: &Path| {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if name != first_for_hook {
            return;
        }
        captures_for_hook.fetch_add(1, Ordering::SeqCst);
        // The first capture of archive 1 INSIDE the manifest-defining pass
        // injects the late archive; the pass's fresh listing then forces the
        // retry that captures both.
        if armed_for_hook.load(Ordering::SeqCst) && !injected_for_hook.swap(true, Ordering::SeqCst)
        {
            write_file(&source_for_hook.join(archive_name(2)), b"archived-2\n");
        }
    }));
    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run_online(&port, source, to, &CancelFlag::default());
    drop(seam);
    drop(arm);
    let report = result.expect("the round that re-lists the late archive must succeed");
    assert_eq!(
        listings.load(Ordering::SeqCst),
        2,
        "the protocol lists the data dir twice (pass D, then the residual pass E)"
    );
    assert!(
        armed.load(Ordering::SeqCst),
        "the injection is armed only by the manifest-defining pass"
    );
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

/// A file that vanishes inside an UNSTABLE round is counted MISSING, and that count survives into the result the accepted round returns.
///
/// Round 1 lists archives 1 and 7 and the active log; the hook then creates archive 2 and deletes archive 7 BEFORE its copy, so round 1 loses 7 and its fresh listing names 2: round 1 is unstable and round 2 is accepted. The vanished file leaves no destination copy, so nothing is refused: this is the pass-result oracle for the counter itself.
///
/// Mutant caught: dropping `missing_files += 1` on the copy's `NotFound` arm — the result reports 0. Mutant caught: not accumulating a round's counters into `result` (`result.missing_files += audit.missing_files`) — the accepted round saw no loss, so the result still reports 0.
#[test]
fn a_source_that_vanishes_in_an_unstable_round_carries_its_missing_count() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    let first = archive_name(1);
    let doomed = archive_name(7);
    let late = archive_name(2);
    write_file(&source.join("repos/alpha/store.bin"), b"store-bytes");
    write_file(&source.join(&first), b"archived-1\n");
    write_file(&source.join(&doomed), b"archived-7\n");
    write_file(&source.join("audit.log"), b"active\n");
    let doomed_for_hook = doomed.clone();
    let source_for_hook = source.to_path_buf();
    let late_for_hook = late.clone();
    let armed = Arc::new(AtomicBool::new(false));
    let armed_for_hook = armed.clone();
    let seam = test_seams::set_after_copy(Box::new(move |path: &Path| {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if name != archive_name(1) || armed_for_hook.swap(true, Ordering::SeqCst) {
            return;
        }
        // The rotation: a NEW archive appears (the round can no longer be the
        // accepted one) and a pruned one disappears before its own copy.
        write_file(&source_for_hook.join(&late_for_hook), b"archived-2\n");
        fs::remove_file(source_for_hook.join(&doomed_for_hook))
            .expect("retention prunes the archive");
    }));
    let result = run_data_dir_copy_pass(
        source,
        to,
        &[],
        &CancelFlag::default(),
        &CopyOptions::default(),
    );
    drop(seam);
    let result = result.expect("a vanished file is missing, never a failure");
    assert!(
        armed.load(Ordering::SeqCst),
        "the fault must inject inside the audit phase"
    );
    assert_eq!(
        result.missing_files, 1,
        "the vanished archive is counted missing and the count carries into the result"
    );
    let captured: Vec<&str> = result
        .files
        .iter()
        .map(|file| file.rel_path.as_str())
        .collect();
    for name in [first.as_str(), late.as_str(), "audit.log"] {
        assert!(
            captured.contains(&name),
            "the accepted round must describe {name}: {captured:?}"
        );
    }
    assert!(
        !captured.contains(&doomed.as_str()),
        "the vanished archive must not be described: {captured:?}"
    );
    assert!(
        !to.join(&doomed).exists(),
        "the vanished archive was never copied, so no destination copy exists"
    );
}

#[test]
fn stale_active_replacement_without_a_new_archive_exhausts_bounded_retries() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    audit_fixtures::seed_alpha_repo(source);
    let archive = archive_name(1);
    let archive_bytes = b"existing rotation archive\n";
    let stale_active = b"stale active replacement\n";
    write_file(&source.join(&archive), archive_bytes);
    write_file(&to.join(&archive), archive_bytes);
    write_file(&to.join("audit.log"), stale_active);

    let captures = Arc::new(AtomicUsize::new(0));
    let captures_for_hook = captures.clone();
    let archive_for_hook = archive.clone();
    let capture = test_seams::set_after_capture(Box::new(move |path: &Path| {
        if path.file_name().and_then(|name| name.to_str()) == Some(archive_for_hook.as_str()) {
            captures_for_hook.fetch_add(1, Ordering::SeqCst);
        }
    }));
    let destination_scans = Arc::new(AtomicUsize::new(0));
    let scans_for_hook = destination_scans.clone();
    let destination_active_survived = Arc::new(AtomicBool::new(true));
    let survived_for_hook = destination_active_survived.clone();
    let stale_active = stale_active.to_vec();
    let scan = test_seams::set_destination_audit_order(Box::new(move |root, paths| {
        scans_for_hook.fetch_add(1, Ordering::SeqCst);
        let survives = fs::read(root.join("audit.log")).is_ok_and(|bytes| bytes == stale_active);
        survived_for_hook.fetch_and(survives, Ordering::SeqCst);
        paths
    }));
    let result = run_data_dir_copy_pass(
        source,
        to,
        &[],
        &CancelFlag::default(),
        &CopyOptions::default(),
    );
    drop(scan);
    drop(capture);

    assert!(
        !source.join("audit.log").exists(),
        "the direct pass must not copy a source active log"
    );
    assert!(
        matches!(result, Err(CopyError::AuditArchiveLost)),
        "the stale active destination must be refused after bounded retries: {result:?}"
    );
    assert_eq!(
        captures.load(Ordering::SeqCst),
        4,
        "the accepted archive is captured in the initial round and each of three retries"
    );
    assert_eq!(
        destination_scans.load(Ordering::SeqCst),
        4,
        "the destination audit guard scans once in each exhausted round"
    );
    assert!(
        destination_active_survived.load(Ordering::SeqCst),
        "the stale destination active bytes survive every guard scan through refusal"
    );
}

#[test]
fn unmatched_archive_precedes_active_leftover_regardless_of_directory_order() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    let accepted_archive = archive_name(1);
    let unmatched_archive = archive_name(2);
    write_file(&source.join(&accepted_archive), b"accepted archive\n");
    write_file(&to.join(&accepted_archive), b"accepted archive\n");
    write_file(&to.join("audit.log"), b"stale active\n");
    write_file(&to.join(&unmatched_archive), b"stale archive\n");

    let to_root = to.to_path_buf();
    let order = test_seams::set_destination_audit_order(Box::new(move |dir, mut paths| {
        if dir == to_root {
            paths.sort_by_key(|path| {
                if path.file_name().and_then(|name| name.to_str()) == Some("audit.log") {
                    0
                } else {
                    1
                }
            });
        }
        paths
    }));
    let accepted_name = accepted_archive.clone();
    let captures = Arc::new(AtomicUsize::new(0));
    let captures_for_hook = captures.clone();
    let capture = test_seams::set_after_capture(Box::new(move |path: &Path| {
        if path.file_name().and_then(|name| name.to_str()) == Some(accepted_name.as_str()) {
            captures_for_hook.fetch_add(1, Ordering::SeqCst);
        }
    }));

    let result = run_data_dir_copy_pass(
        source,
        to,
        &[],
        &CancelFlag::default(),
        &CopyOptions::default(),
    );
    drop(capture);
    drop(order);
    assert!(
        matches!(result, Err(CopyError::AuditArchiveLost)),
        "an unmatched archive must take priority over an active-log leftover: {result:?}"
    );
    assert_eq!(
        captures.load(Ordering::SeqCst),
        1,
        "the accepted audit round captures its archive once; no reopen retry"
    );
}

/// D copies a real anchored active log. E's phase-1 listing then loses the source file; it reappears during the destination guard scan without a new archive, so the stale D destination copy
/// must remain and E must refuse after its bounded retry captures the replacement.
#[test]
fn active_log_reappearance_without_a_new_archive_is_lost() {
    let source_dir = TempDir::new().expect("source temp dir");
    let dest_dir = TempDir::new().expect("destination temp dir");
    let source = source_dir.path();
    let dest = dest_dir.path();
    audit_fixtures::seed_alpha_repo(source);
    let stale_bytes = b"D active audit history\n";
    write_file(&source.join("audit.log"), stale_bytes);

    let source_log = source.join("audit.log");
    let replacement = b"same-name replacement without an archive\n";
    let listings = Arc::new(AtomicUsize::new(0));
    let listings_for_hook = listings.clone();
    let source_log_for_listing = source_log.clone();
    let remove_source = test_seams::set_after_listing(Box::new(move |_root| {
        // Phase 1 of pass D is listing 1; phase 1 of residual pass E is 2.
        // Lose the active log only after E has listed it, before E's audit round.
        if listings_for_hook.fetch_add(1, Ordering::SeqCst) == 1 {
            fs::remove_file(&source_log_for_listing).expect("remove source after E listing");
        }
    }));

    let active_captures = Arc::new(AtomicUsize::new(0));
    let captures_for_hook = active_captures.clone();
    let capture = test_seams::set_after_capture(Box::new(move |path: &Path| {
        if path.file_name().and_then(|name| name.to_str()) == Some("audit.log") {
            captures_for_hook.fetch_add(1, Ordering::SeqCst);
        }
    }));

    let destination_scans = Arc::new(AtomicUsize::new(0));
    let scans_for_hook = destination_scans.clone();
    let replacement_created = Arc::new(AtomicBool::new(false));
    let created_for_hook = replacement_created.clone();
    let stale_bytes_preserved = Arc::new(AtomicBool::new(false));
    let preserved_for_hook = stale_bytes_preserved.clone();
    let source_log_for_scan = source_log.clone();
    let run_dir_seen = Arc::new(Mutex::new(None::<PathBuf>));
    let run_dir_seen_for_scan = run_dir_seen.clone();
    let scan = test_seams::set_destination_audit_order(Box::new(move |root, paths| {
        scans_for_hook.fetch_add(1, Ordering::SeqCst);
        *run_dir_seen_for_scan.lock().expect("run dir") = Some(root.to_path_buf());
        if !source_log_for_scan.exists() && !created_for_hook.swap(true, Ordering::SeqCst) {
            // E phase 2, round 1: the source disappeared after E's phase-1
            // listing. Reopen under the same name, but create no archive.
            write_file(&source_log_for_scan, replacement);
            let remains = fs::read(root.join("audit.log")).is_ok_and(|bytes| bytes == stale_bytes);
            preserved_for_hook.store(remains, Ordering::SeqCst);
        }
        paths
    }));

    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run_online(&port, source, dest, &CancelFlag::default());
    drop(scan);
    drop(capture);
    drop(remove_source);

    assert_eq!(
        listings.load(Ordering::SeqCst),
        2,
        "D then E phase-1 listing"
    );
    assert_eq!(
        active_captures.load(Ordering::SeqCst),
        2,
        "D captures active log, E's missing-active round omits it, then the bounded retry captures replacement",
    );
    assert_eq!(
        destination_scans.load(Ordering::SeqCst),
        2,
        "destination audit guard scans in D and in E's first stable round; the retry refuses before scanning"
    );
    assert!(
        replacement_created.load(Ordering::SeqCst),
        "E destination scan re-creates the source name"
    );
    assert!(
        stale_bytes_preserved.load(Ordering::SeqCst),
        "the destination still has D's original active bytes"
    );
    let error = result.expect_err("same-name replacement without archive evidence must refuse");
    let OnlineBackupError::Failed {
        stage,
        source: primary,
        cleanup,
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Sealing);
    assert_eq!(*cleanup, None, "the failed run directory is removed");
    assert!(
        matches!(primary, OnlineBackupFailure::Copy(CopyError::AuditArchiveLost)),
        "same-name replacement without missing-round archive evidence must refuse during sealing: {error:?}"
    );
    assert_eq!(
        error.stage_and_reason(),
        "sealing: copy-audit-archive-lost",
        "the refusal token must be stable and path-free"
    );
    let rendered = format!("{error}");
    assert!(
        !rendered.contains(&source.display().to_string())
            && !rendered.contains(&dest.display().to_string()),
        "the refusal must not expose fixture paths: {rendered}"
    );
    assert!(
        !source.join(archive_name(1)).exists(),
        "the source must not acquire archive evidence"
    );
    let run_dir = run_dir_seen
        .lock()
        .expect("run dir")
        .clone()
        .expect("destination scan observed the run");
    assert!(
        !run_dir.exists(),
        "cleanup must remove the run: {}",
        run_dir.display()
    );
}

/// An ITERATOR failure of the phase-2 audit listing is the typed, path-free refusal — the same arm the pass listing uses.
///
/// `list_audit_files` is the ONE phase-2 listing (capture round and stability check): a dropped `DirEntry` there could hide a rotation archive; the injected error travels the shared seam.
///
/// Mutant caught: `let entry = entry?` mapped to a path-bearing `CopyError::Io` (the parent's old shape) — the typed assertion and the path canary fail.
#[test]
fn a_phase_two_listing_iteration_failure_is_a_typed_refusal() {
    let source_dir = TempDir::new().expect("source temp dir");
    let source = source_dir.path();
    write_file(&source.join(archive_name(1)), b"archived\n");
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
    let error = list_audit_files(source, &CancelFlag::default(), None)
        .expect_err("an enumeration failure must be typed, never a silent omission");
    drop(seam);
    let CopyError::AuditListing { source } = &error else {
        panic!("expected AuditListing, got {error:?}");
    };
    assert_eq!(source.kind(), ErrorKind::PermissionDenied);
    let rendered = error.to_string();
    let fixture_dir = source_dir.path().display().to_string();
    assert!(
        !rendered.contains(&fixture_dir) && !rendered.contains(&archive_name(1)),
        "the refusal must carry no path and no entry name: {rendered}"
    );
}

/// A metadata `NotFound` on a top-level archive during phase 1 is the documented VANISHED-ENTRY race, not a refusal — and the phase-2 listing still captures the file.
///
/// The `NotFound`-before-classification order is what makes this work: the entry is skipped in phase 1 (it appeared to vanish between the listing and the stat) and the audit phase, which lists
/// the directory again, takes it.
///
/// Mutant caught: classifying `NotFound` as `AuditListing` (or moving the classification above the `NotFound` arm) — phase 1 refuses and the run never publishes.
#[test]
fn a_phase_one_metadata_not_found_is_skipped_and_phase_two_captures_the_archive() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    audit_fixtures::seed_alpha_repo(source);
    let archive = archive_name(1);
    write_file(&source.join(&archive), b"archived\n");
    write_file(&source.join("audit.log"), b"active\n");
    let failing = source.join(&archive);
    let fired = Arc::new(AtomicBool::new(false));
    let fired_for_hook = fired.clone();
    let seam = test_seams::set_symlink_metadata(Box::new(move |path: &Path| {
        if path == failing && !fired_for_hook.swap(true, Ordering::SeqCst) {
            Some(Err(std::io::Error::from(ErrorKind::NotFound)))
        } else {
            None
        }
    }));
    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run_online(&port, source, to, &CancelFlag::default());
    drop(seam);
    assert!(
        fired.load(Ordering::SeqCst),
        "the phase-1 metadata read must be the one that vanished"
    );
    let report = result.expect("a vanished entry is skipped, never refused");
    let paths = audit_fixtures::manifest_paths(&report.dest_dir);
    assert!(
        paths.contains(&archive),
        "the audit phase re-lists the directory and captures the archive: {paths:?}"
    );
    assert_eq!(
        fs::read(report.dest_dir.join(&archive)).expect("archive copy"),
        b"archived\n"
    );
}

/// A source that keeps producing archives is refused after exactly FOUR rounds.
///
/// The round boundary is counted INDEPENDENTLY of the production bound: the active log is captured last in every audit round, so its capture count is the number of rounds the pass entered. The
/// reported value is matched against the literal 4, never against the constant that drives the loop, so raising the bound to five (or running a fifth round while still reporting four) turns
/// this test red.
///
/// Mutant caught: an unstable listing tolerated as if it were stable (the pass returns the last round and `Ok`) — `expect_err` fails; a bound other than four — the literal assertions fail.
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
/// `list_audit_files` is the ONE listing both the capture round and the stability check use, so an entry dropped on a metadata error hides a possibly-new archive from BOTH listings and the
/// snapshot silently lacks it.
///
/// Mutant caught: the parent's `Err(_) => continue` (silent omission), and, for the same reason, a listing that never consults the injection: the parent returns `Ok` and `expect_err` fails.
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

/// A PERSISTENT metadata fault on a top-level archive fails the WHOLE run with the typed, path-free refusal — phase 1 included.
///
/// Phase 1 lists the data dir before the audit phase exists and reads the entry's metadata BEFORE applying the phase-1 audit-name exclusion; without the classification in
/// `copy_engine::list_files` that failure leaves as the path-bearing `CopyError::Io`, which a manual run's `last_error` renders with the filesystem path.
///
/// Mutant caught: classifying an unreadable reserved audit name as `CopyError::Io` — the failure-type assertion fails and the canary (the archive name and the fixture directory) appears in
/// the rendered chain.
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

/// An archive THIS pass captured in an earlier round and retention then pruned refuses the run.
///
/// The anchors are captured by phase 1 of the manifest-defining pass (step E of `online/protocol.rs`) BEFORE any audit listing. An audit round of that same pass then copies archive 1; retention
/// prunes it from the source before the accepted round lists the directory, so the destination holds an audit chain file the accepted round never captured. The pass must refuse — typed and
/// path-free, `CopyError::AuditArchiveLost` under the stable reason token — BEFORE the manifest exists, and it must delete nothing: publishing would either drop bytes a captured anchor may
/// name or describe a file set the destination contradicts.
///
/// The retention step runs inside the capture hook right after the SECOND capture of archive 1 (the first is pass D, the second a round of pass E); the hook is the rendezvous, no sleep.
///
/// Mutant caught: publishing without the destination check (the branch before `AuditArchiveLost`) — the run succeeds and its own inventory verification then refuses the snapshot only after
/// the manifest was written, or (with a reconciling copy) it silently drops the archive the anchors may name.
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

/// An archive that PASS D captured and retention pruned before pass E refuses the run: the leftover copy is never deleted, and the run never publishes.
///
/// Pass D's audit round copies archive 1 into the destination. At the D to E boundary retention prunes it and a rotation leaves archive 2, so no round of pass E ever captured archive 1 and its
/// destination copy has no manifest entry. The pass-level guard runs before the manifest is written, so the run refuses typed and path-free instead of publishing a snapshot its own inventory
/// contradicts — and the copy deletes nothing on the way out.
///
/// The D to E boundary hook is the rendezvous: it runs after pass D's audit round and before pass E's first listing, and it records the run directory it found so the destination assertion below
/// is pinned to the run the protocol created.
///
/// Mutant caught: a guard that only compares within ONE pass (the previous same-pass `earlier_archives` set) — the cross-pass leftover is ignored, the run succeeds, and either the inventory
/// check refuses after publication or a reconciling copy removes a file it does not own.
#[test]
fn an_archive_captured_by_an_earlier_pass_and_retention_pruned_refuses_the_run() {
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
    let observed = observed
        .lock()
        .expect("observed run dir")
        .clone()
        .expect("the boundary hook must have run");
    let error = result.expect_err("a cross-pass leftover must refuse the run");
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
        "the cleanup guard removes the run directory, so there is no cleanup failure"
    );
    assert!(
        matches!(
            primary,
            OnlineBackupFailure::Copy(CopyError::AuditArchiveLost)
        ),
        "the refusal must be the typed lost-audit-file error, got {primary:?}"
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
    assert!(
        !observed.exists(),
        "the cleanup guard removes the run directory the copy refused: {}",
        observed.display()
    );
    assert_eq!(
        fs::read_dir(to).expect("destination root").count(),
        0,
        "R4: the refused snapshot must not survive in the destination"
    );
}

/// An ENUMERATION failure of the data-directory root is the typed, path-free audit refusal — a different arm from failing to OPEN the root.
///
/// Phase 1 of the data-directory pass is the one listing whose root owns the audit chain files, so a dropped entry there could hide a rotation archive: the failure must be
/// `CopyError::AuditListing` (`copy-io-<kind>`, no path). Failing to open the root itself keeps the path-bearing `CopyError::Io` — the operator needs the directory name — which the arm
/// order in `list_files` pins (`read_dir` fails before any entry is yielded).
///
/// Mutant caught: classifying the iterator failure as `CopyError::Io` — the typed assertion fails and the rendered refusal carries the fixture path.
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
