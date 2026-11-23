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

use tempfile::TempDir;

use super::super::failure_injection_tests::write_file;
use super::audit_fixtures::archive_name;
use crate::sync::durability::audit_chain_files::{list_audit_files, AUDIT_CAPTURE_ROUNDS};
use crate::sync::durability::backup::online::copy_engine::test_seams;
use crate::sync::durability::backup::online::copy_engine::{
    run_data_dir_copy_pass, CopyError, CopyOptions,
};
use crate::sync::durability::backup::online::progress::CancelFlag;

/// One late rotation archive forces exactly one more round.
///
/// The `after_capture` hook fires once per captured file (phase-1 files
/// included), so it acts only on the first capture of the fixture's archive
/// and writes the NEXT archive into the source with no appender involved.
///
/// Mutant caught: a data-directory pass that captures ONE round and never
/// re-lists (the single-listing order). There the late archive is absent from
/// `result.files` and from the destination while the pass still returns `Ok`,
/// so the archive assertions fail.
#[test]
fn one_late_archive_forces_another_round_and_lands_in_the_snapshot() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    write_file(&source.join("repos/alpha/store.bin"), b"store-bytes");
    write_file(&source.join(archive_name(1)), b"archived-1\n");
    write_file(&source.join("audit.log"), b"active\n");
    let first = archive_name(1);
    let first_for_hook = first.clone();
    let created = Arc::new(AtomicBool::new(false));
    let created_for_hook = created.clone();
    let source_for_hook = source.to_path_buf();
    let seam = test_seams::set_after_capture(Box::new(move |path: &Path| {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if name != first_for_hook || created_for_hook.swap(true, Ordering::SeqCst) {
            return;
        }
        write_file(&source_for_hook.join(archive_name(2)), b"archived-2\n");
    }));
    let result = run_data_dir_copy_pass(
        source,
        to,
        &[],
        &CancelFlag::default(),
        &CopyOptions::default(),
    );
    drop(seam);
    let result = result.expect("the round that re-lists the late archive must succeed");
    assert!(
        created.load(Ordering::SeqCst),
        "the seam must create the late archive"
    );
    let captured: Vec<&str> = result
        .files
        .iter()
        .map(|file| file.rel_path.as_str())
        .collect();
    assert!(
        captured.contains(&archive_name(2).as_str()),
        "the second round must capture the late archive: {captured:?}"
    );
    assert!(
        captured.contains(&first.as_str()),
        "the first archive stays in the snapshot: {captured:?}"
    );
    assert_eq!(
        fs::read(to.join(archive_name(2))).expect("late archive copy"),
        b"archived-2\n",
        "the late archive must be copied to the destination"
    );
    let mut unique = captured.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        captured.len(),
        "no rel_path may be captured twice: {captured:?}"
    );
}

/// A source that keeps producing archives exhausts the round bound.
///
/// The hook writes the next archive after EVERY captured archive, so no round
/// is ever stable.
///
/// Mutant caught: an unstable listing tolerated as if it were stable (the
/// pass returns the last round and `Ok`), so `expect_err` and the variant
/// match fail.
#[test]
fn audit_files_that_keep_appearing_fail_the_pass_after_four_rounds() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    write_file(&source.join(archive_name(1)), b"archived-1\n");
    write_file(&source.join("audit.log"), b"active\n");
    let created = Arc::new(AtomicUsize::new(0));
    let created_for_hook = created.clone();
    let next = Arc::new(AtomicU64::new(2));
    let next_for_hook = next.clone();
    let source_for_hook = source.to_path_buf();
    let seam = test_seams::set_after_capture(Box::new(move |path: &Path| {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        if !name.starts_with("audit.log.") {
            return;
        }
        let index = next_for_hook.fetch_add(1, Ordering::SeqCst);
        created_for_hook.fetch_add(1, Ordering::SeqCst);
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
    assert!(
        created.load(Ordering::SeqCst) >= AUDIT_CAPTURE_ROUNDS,
        "the fixture must keep every round unstable, created {}",
        created.load(Ordering::SeqCst)
    );
    match error {
        CopyError::AuditFilesUnstable { rounds } => assert_eq!(
            rounds, AUDIT_CAPTURE_ROUNDS,
            "the refusal must report the round bound"
        ),
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
