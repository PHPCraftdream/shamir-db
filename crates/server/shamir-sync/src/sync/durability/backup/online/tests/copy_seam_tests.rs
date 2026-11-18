//! Failure-injection cases that need a point INSIDE the copy engine:
//! a genuine mid-file I/O error (after at least one destination
//! write), a source file deleted deterministically AFTER the listing
//! pass, a persistent re-pass hash mismatch forced at the source, and
//! a cancellation between the last WAL copy and its re-hash inside a
//! copy pass — points the port hooks cannot reach (design
//! `online-backup.md` 6.3, R4).
//!
//! The injections use the `#[cfg(test)]` scoped thread-local seams of
//! `copy_engine::test_seams` — the narrowest possible widening: no
//! production code path reads them, and every hook is cleared when its
//! guard drops. Everything else (fixture, R4 assertions, pin
//! counters) is the shared `failure_injection_tests` machinery; no
//! release behaviour is touched.
//!
//! Cases deliberately NOT added (reported, not worked around): the
//! engine's only cancellation checks are the per-file loop tops inside
//! `run_copy_pass` (before the exclusion check, and before the re-hash
//! of the same pass). Cancellation at the C→D, D→E and E→F boundaries
//! IS now covered via the `#[cfg(test)]` `before_boundary(BackupStage)`
//! seam, fired in `protocol.rs` immediately before the existing boundary
//! checks (see the `cancel_at_the_*` tests below) — no production code
//! path reads the seam. There is deliberately NO cancellation check
//! between successful self-verification (F) and Done/G: the snapshot is
//! already verified and published as a unit, so a cancellation raised
//! there still ends Done with valid output (`cancel_after_self_verify_`
//! `…ends_done` below pins this documented behaviour), and `release_pins`
//! is infallible, so no fault there is reportable.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use sha2::{Digest, Sha256};
use tempfile::TempDir;

use super::failure_injection_tests::{
    assert_source_untouched, run, snapshot_dir, write_file, Fixture, InjectPort,
};
use crate::sync::durability::backup::online::copy_engine::test_seams::{self, SeamGuard};
use crate::sync::durability::backup::online::copy_engine::{run_copy_pass, CopyError, CopyOptions};
use crate::sync::durability::backup::online::progress::{BackupProgress, BackupStage, CancelFlag};
use crate::sync::durability::backup::online::protocol::{OnlineBackupError, OnlineBackupFailure};
use crate::sync::durability::backup::verify_manifest;

/// Shared cancelled-run assertions: typed cancellation at `stage`,
/// destination root empty, pins released exactly once, progress
/// carries the cancelled stage as failed_stage and its display.
fn assert_cancelled_cleanup(
    to: &Path,
    port: &InjectPort,
    progress: &BackupProgress,
    stage: BackupStage,
    error: &OnlineBackupError,
) {
    assert!(
        matches!(error, OnlineBackupError::Cancelled { .. }),
        "expected Cancelled, got {error:?}"
    );
    assert_eq!(error.stage(), stage);
    assert_eq!(
        fs::read_dir(to).expect("to dir").count(),
        0,
        "R4: no partial output may survive a cancelled run"
    );
    assert_eq!(
        port.pins_currently_held(),
        0,
        "R4: the truncation pins must be released"
    );
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    assert_eq!(progress.stage, BackupStage::Failed);
    assert_eq!(progress.failed_stage, Some(stage));
    assert_eq!(
        progress.last_error.as_deref(),
        Some(format!("backup cancelled at stage {stage:?}").as_str()),
    );
}

/// Direct, OS-independent regression for the lock-artifact skip of
/// `run_copy_pass`: BOTH runtime lock names — the data-dir maintenance
/// lock `.shamir.lock` (`maintenance_lock.rs`, held by the live server,
/// so on Windows the copy of it can only ever fail with a sharing
/// violation) and fjall's `lock` — are excluded from the copy result's
/// file list AND from the destination, at the root AND nested, while
/// the real payload is copied byte-exact with its SHA-256. The skip is
/// by file NAME on all OSes, so the snapshot manifest cannot differ
/// across platforms (followup to brief 50, item 1).
///
/// This test does NOT use the shared `Fixture`: `Fixture::new()` seeds
/// both alpha/beta stores with WALs and sidecars, so the exact 3-payload
/// manifest assertion below could never hold. Two dedicated `TempDir`s
/// give a minimal, fully controlled source/destination layout instead.
#[test]
fn copy_pass_excludes_both_lock_artifacts_at_every_depth_and_copies_payload() {
    let source_dir = TempDir::new().expect("source tempdir");
    let to_dir = TempDir::new().expect("destination tempdir");
    let source = source_dir.path().to_path_buf();
    let to = to_dir.path().to_path_buf();
    // `source_dir`/`to_dir` stay bound for the whole test, so the
    // tempdirs are not deleted until the assertions have run.
    let _keep = (&source_dir, &to_dir);
    for rel in [
        "repos/alpha/store.bin",
        "repos/alpha/sub/store2.bin",
        "repos/alpha.shamirwal/00000000.wal",
        // The two skipped names, at the root and nested.
        ".shamir.lock",
        "lock",
        "repos/alpha/.shamir.lock",
        "repos/alpha/lock",
    ] {
        write_file(&source.join(rel), b"payload-bytes");
    }
    let cancel = CancelFlag::default();
    let result = run_copy_pass(&source, &to, &[], &[], &cancel, &CopyOptions::default())
        .expect("the copy pass must succeed");
    let expected_hash = hex::encode(Sha256::digest(b"payload-bytes"));
    let mut rel_paths: Vec<&str> = result.files.iter().map(|f| f.rel_path.as_str()).collect();
    rel_paths.sort_unstable();
    assert_eq!(
        rel_paths,
        vec![
            "repos/alpha.shamirwal/00000000.wal",
            "repos/alpha/store.bin",
            "repos/alpha/sub/store2.bin",
        ],
        "file list: exactly the payload, neither lock name at any depth"
    );
    for file in &result.files {
        assert_eq!(file.sha256, expected_hash, "real payload hashed");
        assert_eq!(file.size_bytes, b"payload-bytes".len() as u64);
    }
    assert_eq!(result.changed_files, 0);
    assert_eq!(result.missing_files, 0);
    // Destination: payload byte-exact, no lock artifact of either name
    // anywhere under the snapshot root.
    for rel in [
        "repos/alpha/store.bin",
        "repos/alpha/sub/store2.bin",
        "repos/alpha.shamirwal/00000000.wal",
    ] {
        assert_eq!(
            fs::read(to.join(rel)).expect("copied payload"),
            b"payload-bytes",
            "payload copied byte-exact: {rel}"
        );
    }
    let mut stack = vec![to.clone()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("dest dir") {
            let entry = entry.expect("dest entry");
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let name = entry.file_name().to_string_lossy().into_owned();
                assert!(
                    name != ".shamir.lock" && name != "lock",
                    "no lock artifact may reach the destination: {}",
                    path.display()
                );
            }
        }
    }
}

/// R4: a genuine mid-file I/O failure — injected AFTER the first
/// completed `write_all`, so real destination bytes already exist —
/// fails the run at Copying with a typed `CopyError::Io` naming the
/// SOURCE path, and the shared cleanup removes even the partially
/// written destination file.
#[test]
fn c_mid_file_io_error_after_a_write_fails_at_copying_and_cleans_up() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    let writes = Arc::new(AtomicUsize::new(0));
    let writes_for_hook = writes.clone();
    let seam: SeamGuard = test_seams::set_after_first_write(Box::new(move |path| {
        if path.ends_with(PathBuf::from("repos/alpha/store.bin")) {
            // This hook only runs after a completed `write_all`, so the
            // counter itself proves the failure fired after >= 1 write.
            writes_for_hook.fetch_add(1, Ordering::SeqCst);
            Err(std::io::Error::other("disk full mid-file"))
        } else {
            Ok(())
        }
    }));
    let port = InjectPort::single_repo(&source);
    let cancel = CancelFlag::default();
    let (result, progress) = run(&port, &source, &to, &cancel);
    drop(seam);
    let error = result.expect_err("must fail");
    assert!(
        writes.load(Ordering::SeqCst) >= 1,
        "the hook must have fired after a completed destination write"
    );
    let OnlineBackupError::Failed {
        stage,
        source: failure,
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Copying);
    let OnlineBackupFailure::Copy(CopyError::Io { path, source: io }) = failure else {
        panic!("the failure must be a typed mid-file copy error, got {failure:?}");
    };
    assert_eq!(
        path,
        &source.join("repos/alpha/store.bin"),
        "CopyError::Io must name the source path whose mid-file write failed"
    );
    assert!(
        io.to_string().contains("disk full mid-file"),
        "the typed error must carry the actual mid-file cause"
    );
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert_eq!(port.pins_currently_held(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    assert_eq!(progress.stage, BackupStage::Failed);
    assert_eq!(progress.failed_stage, Some(BackupStage::Copying));
    assert_source_untouched(&before, &source);
}

/// A source file deleted deterministically BEFORE its own copy (the
/// existing `file_deleted_mid_copy_is_absent_from_the_manifest` in
/// `protocol_tests.rs` deletes in the capture hook, BEFORE any
/// listing). The seam fires after `repos/alpha/store.bin` — the first
/// file of the step-D listing order — removes `store2.bin`, and the
/// copy of the already-listed path fails with NotFound, which the
/// engine counts as `missing_files` and EXCLUDES: the run SUCCEEDS,
/// the manifest describes only what exists, and the snapshot verifies.
/// That tolerance is the documented actual result. (Renamed to
/// distinguish it from the FAILURE case where the deletion happens
/// after the file's OWN copy and a stale destination file survives —
/// see `failure_injection_tests.rs`.)
#[test]
fn deleted_listed_source_file_before_its_copy_is_tolerated_and_absent_from_the_manifest() {
    let (_fixture, source, to) = Fixture::new();
    let store2 = Arc::new(source.join("repos/alpha/store2.bin"));
    write_file(&store2, b"extra-store-bytes");
    let deletions = Arc::new(AtomicUsize::new(0));
    let deletions_for_hook = deletions.clone();
    let seam: SeamGuard = test_seams::set_after_copy(Box::new(move |path| {
        if path.ends_with(PathBuf::from("repos/alpha/store.bin"))
            && deletions_for_hook.fetch_add(1, Ordering::SeqCst) == 0
        {
            fs::remove_file(store2.as_path()).expect("delete the listed source file");
        }
    }));
    let port = InjectPort::single_repo(&source);
    let cancel = CancelFlag::default();
    let (result, _progress) = run(&port, &source, &to, &cancel);
    drop(seam);
    let report = result.expect("a deleted listed file is tolerated, not a failure");
    let manifest = fs::read_to_string(report.dest_dir.join("manifest.json")).expect("manifest");
    assert!(
        manifest.contains("repos/alpha/store.bin"),
        "the surviving file must stay in the manifest"
    );
    assert!(
        !manifest.contains("repos/alpha/store2.bin"),
        "the deleted file must be excluded from the manifest"
    );
    assert!(
        !report.dest_dir.join("repos/alpha/store2.bin").exists(),
        "no stale destination copy of the deleted file may survive"
    );
    verify_manifest(&report.dest_dir).expect("snapshot must verify");
    assert_eq!(port.pins_currently_held(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
}

/// The FAILURE twin of the tolerance case above: the source file is
/// deleted AFTER ITS OWN copy completed (the seam fires on the
/// `store2.bin` copy itself), so its bytes are already on the
/// destination when the engine's re-hash loop hits NotFound at the
/// FIRST hash pass and excludes it from the pass result
/// (`missing_files`, not an error — copy/retry NotFound handling).
/// The destination copy of the deleted file now STALE-survives while
/// the manifest does not list it, and the step-F self-verification's
/// global file-set check refuses the snapshot as
/// `UnmanifestedFile("... store2.bin")` — the run fails at `Verifying`
/// with full R4 cleanup (output removed, pins released). Distinct from
/// the existing server-side whole-WAL-dir-removal fault case, which
/// deletes BEFORE any listing (nothing stale is ever copied).
#[test]
fn source_deleted_after_its_own_copy_leaves_a_stale_destination_refused_at_verifying() {
    let (_fixture, source, to) = Fixture::new();
    let store2 = Arc::new(source.join("repos/alpha/store2.bin"));
    write_file(&store2, b"extra-store-bytes");
    let before = snapshot_dir(&source);
    let deletions = Arc::new(AtomicUsize::new(0));
    let deletions_for_hook = deletions.clone();
    let store2_for_hook = store2.clone();
    let seam: SeamGuard = test_seams::set_after_copy(Box::new(move |path| {
        if path.ends_with(PathBuf::from("repos/alpha/store2.bin"))
            && deletions_for_hook.fetch_add(1, Ordering::SeqCst) == 0
        {
            fs::remove_file(store2_for_hook.as_path()).expect("delete the copied source file");
        }
    }));
    let port = InjectPort::single_repo(&source);
    let cancel = CancelFlag::default();
    let (result, progress) = run(&port, &source, &to, &cancel);
    drop(seam);
    let error = result.expect_err("the stale destination copy must refuse the snapshot");
    let OnlineBackupError::Failed {
        stage,
        source: failure,
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Verifying);
    let OnlineBackupFailure::Verify(message) = failure else {
        panic!("the failure must be a typed self-verification error, got {failure:?}");
    };
    assert!(
        message.contains("store2.bin") && message.contains("not listed in manifest"),
        "the verify failure must name the stale unmanifested file, got: {message}"
    );
    assert_eq!(
        fs::read_dir(&to).expect("to dir").count(),
        0,
        "R4: the stale destination copy must be removed with the output"
    );
    assert_eq!(port.pins_currently_held(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    assert_eq!(progress.failed_stage, Some(BackupStage::Verifying));
    // The injection deleted a source file: restore the fixture occupancy
    // so the before/after source comparison stays exact.
    write_file(&store2, b"extra-store-bytes");
    assert_source_untouched(&before, &source);
}

/// Persistent re-pass hash mismatch, forced at the SOURCE (no
/// destination tamper): `hash_file` for alpha's store file is
/// overridden to return a fixed, always-different digest, so the copy
/// hash, both bounded re-copies' hashes and the re-pass hashes never
/// agree. DOCUMENTED ACTUAL RESULT: `run_copy_pass` does NOT fail the
/// pass on a persistent mismatch (its doc text says it does) — it
/// records the final re-pass hash, and the divergence surfaces at the
/// step-F self-verification as `Verify("<path>: hash mismatch")`,
/// failing the run at Verifying with full R4 cleanup. The
/// doc-versus-behaviour divergence is reported to the orchestrator.
#[test]
fn persistent_re_pass_hash_mismatch_surfaces_at_self_verification() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    let source_for_hook = source.clone();
    let seam: SeamGuard = test_seams::set_hash_file(Box::new(move |path| {
        if path.starts_with(&source_for_hook)
            && path.ends_with(PathBuf::from("repos/alpha/store.bin"))
        {
            // Deterministic valid hex that can never equal the SHA-256
            // of the real bytes.
            Some(Ok(("ff".repeat(32), 11)))
        } else {
            // Fall through to the real `hash_file`.
            None
        }
    }));
    let port = InjectPort::two_repos(&source);
    let cancel = CancelFlag::default();
    let (result, progress) = run(&port, &source, &to, &cancel);
    drop(seam);
    let error = result.expect_err("must fail");
    let OnlineBackupError::Failed {
        stage,
        source: failure,
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Verifying);
    let OnlineBackupFailure::Verify(message) = failure else {
        panic!("the failure must be a typed self-verification error, got {failure:?}");
    };
    assert!(
        message.contains("repos/alpha/store.bin") && message.contains("hash mismatch"),
        "the verify failure must name the mismatching file, got: {message}"
    );
    assert_eq!(fs::read_dir(&to).expect("to dir").count(), 0);
    assert_eq!(port.pins_currently_held(), 0);
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    assert_eq!(progress.failed_stage, Some(BackupStage::Verifying));
    assert_source_untouched(&before, &source);
}

/// Cancellation inside a copy pass: the flag is set after the LAST
/// file copy of the step-C WAL pass (sorted order: `00000000.meta`,
/// then `00000000.wal`), so it is observed at the SAME pass's re-hash
/// loop top (the engine's cancellation check between the copy loop and
/// the second hash pass, before re-hashing) — cancelled at Copying.
#[test]
fn cancel_after_the_last_wal_copy_before_its_re_hash_reports_copying_and_cleans_up() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    let cancel = CancelFlag::default();
    let cancel_for_hook = cancel.clone();
    let seam: SeamGuard = test_seams::set_after_copy(Box::new(move |path| {
        if path.ends_with(PathBuf::from("repos/alpha.shamirwal/00000000.wal")) {
            cancel_for_hook.cancel();
        }
    }));
    let port = InjectPort::single_repo(&source);
    let (result, progress) = run(&port, &source, &to, &cancel);
    drop(seam);
    let error = result.expect_err("must fail");
    assert_cancelled_cleanup(&to, &port, &progress, BackupStage::Copying, &error);
    assert_source_untouched(&before, &source);
}

/// Cancellation raised EXACTLY at the D→E boundary: the full step-D
/// copy pass has already succeeded when the `before_boundary(Sealing)`
/// seam fires (in `protocol.rs`, immediately before the existing
/// E-entry cancellation check), so the run is cancelled at `Sealing`
/// — not at a `Copying` loop-top — with the shared R4 cleanup.
#[test]
fn cancel_at_the_d_to_e_boundary_reports_sealing_and_cleans_up() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    let cancel = CancelFlag::default();
    let cancel_for_hook = cancel.clone();
    let seam: SeamGuard = test_seams::set_before_boundary(Box::new(move |stage| {
        if stage == BackupStage::Sealing {
            cancel_for_hook.cancel();
        }
    }));
    let port = InjectPort::single_repo(&source);
    let (result, progress) = run(&port, &source, &to, &cancel);
    drop(seam);
    let error = result.expect_err("must fail");
    assert_cancelled_cleanup(&to, &port, &progress, BackupStage::Sealing, &error);
    assert_source_untouched(&before, &source);
}

/// Cancellation raised EXACTLY at the E→F boundary: every seal hold
/// has already been ended on the success path (`end_seal` per repo)
/// when the `before_boundary(Verifying)` seam fires (in
/// `protocol.rs`, immediately before the existing F-entry
/// cancellation check), so the run is cancelled at `Verifying` with
/// the shared R4 cleanup — pins released, no partial output.
#[test]
fn cancel_at_the_e_to_f_boundary_reports_verifying_and_cleans_up() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    let cancel = CancelFlag::default();
    let cancel_for_hook = cancel.clone();
    let seam: SeamGuard = test_seams::set_before_boundary(Box::new(move |stage| {
        if stage == BackupStage::Verifying {
            cancel_for_hook.cancel();
        }
    }));
    let port = InjectPort::single_repo(&source);
    let (result, progress) = run(&port, &source, &to, &cancel);
    drop(seam);
    let error = result.expect_err("must fail");
    assert_cancelled_cleanup(&to, &port, &progress, BackupStage::Verifying, &error);
    assert_source_untouched(&before, &source);
}

/// Cancellation raised EXACTLY at the C→D boundary: the step-C WAL
/// passes have already succeeded when the `before_boundary(Copying)`
/// seam fires (in `protocol.rs`, between the WAL passes and the first
/// store pass), so the run is cancelled at `Copying` by the store
/// pass's per-file loop top — the existing cancellation check the seam
/// sits before — with the shared R4 cleanup.
#[test]
fn cancel_at_the_c_to_d_boundary_reports_copying_and_cleans_up() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    let cancel = CancelFlag::default();
    let cancel_for_hook = cancel.clone();
    let seam: SeamGuard = test_seams::set_before_boundary(Box::new(move |stage| {
        if stage == BackupStage::Copying {
            cancel_for_hook.cancel();
        }
    }));
    let port = InjectPort::single_repo(&source);
    let (result, progress) = run(&port, &source, &to, &cancel);
    drop(seam);
    let error = result.expect_err("must fail");
    assert_cancelled_cleanup(&to, &port, &progress, BackupStage::Copying, &error);
    assert_source_untouched(&before, &source);
}

/// Cancellation raised after successful self-verification (F→G): the
/// seam fires in `protocol.rs` between `self_verify` and the success
/// return, and there is DELIBERATELY no cancellation check there (no
/// production change — documented behaviour). The run still ends Done:
/// valid published output on disk, pins released, source untouched —
/// there is no cancellation cleanup possible at that point, because
/// the destination is already final and about to be reported Done.
#[test]
fn cancel_after_self_verify_still_ends_done_with_valid_published_output() {
    let (_fixture, source, to) = Fixture::new();
    let before = snapshot_dir(&source);
    let cancel = CancelFlag::default();
    let cancel_for_hook = cancel.clone();
    let seam: SeamGuard = test_seams::set_before_boundary(Box::new(move |stage| {
        if stage == BackupStage::Done {
            cancel_for_hook.cancel();
        }
    }));
    let port = InjectPort::single_repo(&source);
    let (result, progress) = run(&port, &source, &to, &cancel);
    drop(seam);
    assert!(cancel.is_cancelled(), "late cancellation hook must fire");
    let report = result.expect("a late cancellation must NOT abort a verified run");
    assert_eq!(
        progress.stage,
        BackupStage::Done,
        "the run must finish Done even with the flag raised after self-verify"
    );
    assert_eq!(progress.failed_stage, None);
    verify_manifest(&report.dest_dir).expect("published snapshot must verify");
    assert_eq!(
        port.pins_currently_held(),
        0,
        "pins are released on the success path"
    );
    assert_eq!(port.pins_released.load(Ordering::SeqCst), 1);
    assert_source_untouched(&before, &source);
}
