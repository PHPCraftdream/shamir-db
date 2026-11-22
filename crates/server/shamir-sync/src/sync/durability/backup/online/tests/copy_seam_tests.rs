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
//!
//! The audit-consistency cases at the end of this file use the
//! `after_capture` seam (fired once per file when a copy pass's view of
//! that file is final) together with the REAL durable audit appender and
//! the REAL boot recovery: they pin the snapshot invariant "the audit
//! anchor never points past the captured log tail", which holds only
//! because the residual pass captures `audit.log` after the anchor
//! stores.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sha2::{Digest, Sha256};
use shamir_connect::server::audit_chain::{AuditChain, AuditChainWriter};
use shamir_server_security::audit_appender::FjallAuditAppender;
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

/// Audit chain key of the test appender (never leaves the fixture).
const AUDIT_KEY: [u8; 32] = [0x5a; 32];

/// Bounded rendezvous guard: a wedged copy or activity thread fails the
/// test in two minutes instead of hanging the runner.
const AUDIT_HANG_GUARD: Duration = Duration::from_secs(120);

/// Active audit log file name inside `data_dir`.
const AUDIT_LOG_FILE: &str = "audit.log";

/// Audit anchor store directory inside `data_dir` (forward-slash prefix of
/// the snapshot-relative paths).
const AUDIT_ANCHOR_DIR: &str = "audit_checkpoint/";

/// The audit event the fixture appends.
const AUDIT_EVENT: &str = "auth_success";

/// One live audit store in `dir`: the REAL durable appender (fixed-format
/// `audit.log` + fjall `audit_checkpoint`) driven through the REAL chain
/// writer, seeded with `events` appended records and one durable anchor.
fn open_audit(dir: &Path, events: usize) -> (Arc<FjallAuditAppender>, AuditChainWriter) {
    let appender = FjallAuditAppender::open_strict(dir).expect("open the durable audit appender");
    let writer = AuditChainWriter::new(AuditChain::new(AUDIT_KEY), appender.clone());
    append_cycles(&writer, 1, events);
    (appender, writer)
}

/// `cycles` rounds of `events_per_cycle` appends, each round followed by a
/// durable checkpoint — the flush-then-anchor order the server uses
/// (`checkpoint_locked` drains the log before the anchor row is written).
fn append_cycles(writer: &AuditChainWriter, cycles: usize, events_per_cycle: usize) {
    for _ in 0..cycles {
        for _ in 0..events_per_cycle {
            writer
                .append(
                    AUDIT_EVENT,
                    "test",
                    "user",
                    "127.0.0.0/24",
                    [7; 8],
                    "ok",
                    vec![0xc0],
                    42,
                )
                .expect("append an audit event");
        }
        writer.checkpoint_now().expect("persist the audit anchor");
    }
}

/// Plain file-tree copy of the snapshot into a fresh directory: restoring an
/// online copy moves the snapshot tree into the target `data_dir` (engine
/// stores are replayed separately; the audit files are carried as bytes).
fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).expect("create the restored directory");
    let mut stack = vec![from.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).expect("read the snapshot directory") {
            let entry = entry.expect("snapshot entry");
            let path = entry.path();
            let target = to.join(path.strip_prefix(from).expect("under the snapshot root"));
            if path.is_dir() {
                fs::create_dir_all(&target).expect("create the restored subdirectory");
                stack.push(path);
            } else {
                fs::copy(&path, &target).expect("copy a snapshot file");
            }
        }
    }
}

/// A source data dir with one file-backed repo (`alpha`) plus a real audit
/// store, in a fresh temp dir kept alive by the returned handles.
fn audit_source() -> (TempDir, TempDir, PathBuf, PathBuf, Arc<FjallAuditAppender>) {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let source = source_dir.path().to_path_buf();
    let to = to_dir.path().to_path_buf();
    write_file(&source.join("repos/alpha/store.bin"), b"store-bytes");
    write_file(&source.join("repos/alpha.shamirwal/00000000.wal"), b"wal-0");
    write_file(
        &source.join("repos/alpha.shamirwal/00000000.meta"),
        b"meta-0",
    );
    let (appender, writer) = open_audit(&source, 2);
    drop(writer);
    (source_dir, to_dir, source, to, appender)
}

/// Snapshot-relative (forward-slash) form of a captured source path.
fn captured_rel(source: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(source).ok().map(|rel| {
        rel.components()
            .map(|component| component.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    })
}

/// The snapshot-defining capture order: `audit.log` is captured AFTER every
/// file of its anchor store, so a checkpoint written to the anchor store
/// between two captures can never end up newer than the captured log.
///
/// Mutant caught: the previous sorted-path order (`audit.log` first,
/// `audit_checkpoint/**` last) — this assertion fails on the observed capture
/// sequence itself, with no timing involved.
#[test]
fn audit_log_is_captured_after_its_anchor_store() {
    let (_source_dir, _to_dir, source, to, _appender) = audit_source();
    let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let order_for_hook = order.clone();
    let source_for_hook = source.clone();
    let seam = test_seams::set_after_capture(Box::new(move |path| {
        if let Some(rel) = captured_rel(&source_for_hook, path) {
            order_for_hook.lock().expect("order lock").push(rel);
        }
    }));
    let port = InjectPort::with_repos(&source, &["alpha"]);
    let cancel = CancelFlag::default();
    let (result, _progress) = run(&port, &source, &to, &cancel);
    drop(seam);
    let report = result.expect("the copy must succeed");
    verify_manifest(&report.dest_dir).expect("the published snapshot must verify");
    let order = order.lock().expect("order lock").clone();
    let log_index = order
        .iter()
        .rposition(|rel| rel == AUDIT_LOG_FILE)
        .expect("the audit log must be captured in the run");
    let anchor_indices: Vec<usize> = order
        .iter()
        .enumerate()
        .filter(|(_, rel)| rel.starts_with(AUDIT_ANCHOR_DIR))
        .map(|(index, _)| index)
        .collect();
    assert!(
        !anchor_indices.is_empty(),
        "the anchor store must contribute captured files: {order:?}"
    );
    assert_eq!(
        Some(log_index),
        order.len().checked_sub(1),
        "the audit log must be the LAST capture of the run: {order:?}"
    );
    assert!(
        anchor_indices.iter().all(|index| *index < log_index),
        "every audit_checkpoint file must be captured before the audit log: {order:?}"
    );
}

/// A copy taken while audit events keep arriving restores and boots.
///
/// The copy is paused DETERMINISTICALLY (channel rendezvous, no sleeps) right
/// after the snapshot-defining capture of `audit.log` — exactly the window in
/// which the old order captured the anchor store afterwards. While it is
/// paused, three real append + checkpoint cycles run through the real
/// appender, then the copy is released, the snapshot restored into a fresh
/// directory and opened through the real boot recovery.
///
/// Mutant caught: the previous capture order (log first, anchor store last).
/// There the pause-time checkpoint is re-copied by the hash pass AFTER the
/// captured log, so the transactional copy carries an anchor past the tail of
/// its own log and this test's recovery call fails with
/// `AuditRecoveryError::AnchorPastTail`. Mutant caught: a manifest that
/// records the pre-capture hash — step F re-hashes the destination against
/// the manifest, so the run itself fails instead of returning `Ok`.
#[test]
fn copy_paused_after_the_audit_log_capture_restores_and_boots() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let restored_dir = TempDir::new().expect("restored temp dir");
    let source = source_dir.path().to_path_buf();
    let to = to_dir.path().to_path_buf();
    let restored = restored_dir.path().join("data");
    // Fixture: one file-backed repo plus the real audit store, seeded with
    // two durable records and one anchor before the copy starts.
    write_file(&source.join("repos/alpha/store.bin"), b"store-bytes");
    write_file(&source.join("repos/alpha.shamirwal/00000000.wal"), b"wal-0");
    write_file(
        &source.join("repos/alpha.shamirwal/00000000.meta"),
        b"meta-0",
    );
    let (appender, writer) = open_audit(&source, 2);

    let (paused_tx, paused_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let captures = Arc::new(AtomicUsize::new(0));
    let captures_for_hook = captures.clone();
    let audit_log_path = source.join(AUDIT_LOG_FILE);
    let seam = test_seams::set_after_capture(Box::new(move |path| {
        if path != audit_log_path {
            return;
        }
        // The protocol captures the data dir twice (the copy pass D and the
        // snapshot-defining residual pass E); the pause must land in the
        // latter, the capture the manifest describes.
        if captures_for_hook.fetch_add(1, Ordering::SeqCst) != 1 {
            return;
        }
        paused_tx.send(()).expect("signal the capture pause");
        release_rx
            .recv_timeout(AUDIT_HANG_GUARD)
            .expect("the test must release the copy");
    }));
    // The copy runs on THIS thread (the seams are thread-local); the audit
    // activity runs on a helper thread while the copy is paused.
    let activity = std::thread::Builder::new()
        .name("audit-activity".to_string())
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            paused_rx
                .recv_timeout(AUDIT_HANG_GUARD)
                .expect("the copy must reach the capture pause");
            append_cycles(&writer, 3, 2);
            release_tx.send(()).expect("release the copy");
        })
        .expect("spawn the audit activity thread");
    let port = InjectPort::with_repos(&source, &["alpha"]);
    let cancel = CancelFlag::default();
    let (result, _progress) = run(&port, &source, &to, &cancel);
    drop(seam);
    activity
        .join()
        .expect("the audit activity thread must finish");
    assert_eq!(
        captures.load(Ordering::SeqCst),
        2,
        "the pause must land in the snapshot-defining capture (pass D, then pass E)"
    );
    let report = result.expect("the copy must succeed");

    copy_tree(&report.dest_dir, &restored);
    let anchor = FjallAuditAppender::load_checkpoint(&restored)
        .expect("read the restored anchor")
        .expect("the restored copy must carry the audit anchor");
    let entries =
        FjallAuditAppender::read_log_for_verify(&restored).expect("read the restored audit log");
    AuditChain::verify_chain(&AUDIT_KEY, &entries).expect("the restored log must verify");
    let tail = entries.last().expect("the restored log must not be empty");
    assert!(
        anchor.0 <= tail.seq + 1,
        "the captured anchor {:?} must not point past the captured log tail {}",
        anchor.0,
        tail.seq + 1
    );
    let covered = entries
        .iter()
        .find(|entry| entry.seq + 1 == anchor.0)
        .expect("the captured log must reach the anchor position");
    assert_eq!(
        covered.hmac, anchor.1,
        "the anchor must authenticate the captured log at its own position"
    );
    let (recovered, position) =
        FjallAuditAppender::open_recovered_strict(&restored, &AUDIT_KEY, None)
            .expect("the restored copy must pass audit recovery (no AnchorPastTail)");
    assert!(
        position.0 >= anchor.0,
        "recovery resumes at the verified tail {} at or after the anchor {}",
        position.0,
        anchor.0
    );
    drop(recovered);
    drop(appender);
}

/// A copy taken while NO audit activity happens still restores and boots, and
/// carries the anchor of exactly the log it captured.
///
/// Mutant caught: a pass that defers the log but loses it (or its anchor) from
/// the snapshot — the byte-exact log comparison and the exact anchor position
/// both fail. Mutant caught: a manifest that records a hash from an earlier
/// read — the run fails its own step-F self-verification instead of returning
/// `Ok`.
#[test]
fn quiet_audit_log_copy_restores_and_boots() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let restored_dir = TempDir::new().expect("restored temp dir");
    let source = source_dir.path().to_path_buf();
    let to = to_dir.path().to_path_buf();
    let restored = restored_dir.path().join("data");
    write_file(&source.join("repos/alpha/store.bin"), b"store-bytes");
    write_file(&source.join("repos/alpha.shamirwal/00000000.wal"), b"wal-0");
    write_file(
        &source.join("repos/alpha.shamirwal/00000000.meta"),
        b"meta-0",
    );
    let (appender, writer) = open_audit(&source, 2);
    let port = InjectPort::with_repos(&source, &["alpha"]);
    let cancel = CancelFlag::default();
    let (result, _progress) = run(&port, &source, &to, &cancel);
    let report = result.expect("the copy of a quiet audit store must succeed");
    drop(writer);

    copy_tree(&report.dest_dir, &restored);
    assert_eq!(
        fs::read(restored.join(AUDIT_LOG_FILE)).expect("restored log bytes"),
        fs::read(source.join(AUDIT_LOG_FILE)).expect("source log bytes"),
        "a copy with no audit activity must carry the log byte-exact"
    );
    let entries =
        FjallAuditAppender::read_log_for_verify(&restored).expect("read the restored audit log");
    AuditChain::verify_chain(&AUDIT_KEY, &entries).expect("the restored log must verify");
    let tail = entries.last().expect("the audit log must not be empty");
    let anchor = FjallAuditAppender::load_checkpoint(&restored)
        .expect("read the restored anchor")
        .expect("the restored copy must carry the audit anchor");
    assert_eq!(
        anchor,
        (tail.seq + 1, tail.hmac),
        "the captured anchor must be exactly the captured log's tail"
    );
    let (recovered, position) =
        FjallAuditAppender::open_recovered_strict(&restored, &AUDIT_KEY, None)
            .expect("the restored copy must pass audit recovery");
    assert_eq!(
        position.0, anchor.0,
        "recovery resumes at the captured anchor"
    );
    drop(recovered);
    drop(appender);
}
