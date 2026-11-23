//! Rotation-boundary cases of the online backup's audit chain capture: a
//! rotation that lands between the data-directory pass listing and its
//! anchor-store capture, and a rotation that lands inside the corrective
//! re-copy of the audit phase.
//!
//! The copy runs on the test thread (the seam hooks are thread-local); the
//! audit activity runs on a helper thread while the copy is paused, and every
//! rendezvous is a bounded channel wait (no sleeps). The fixtures are the
//! shared `failure_injection_tests` port/fixture machinery plus the
//! `audit_fixtures` helpers for the key seed, the archive names, the
//! `server_meta` anchor and the snapshot-tree copy.

use std::fs;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use shamir_connect::server::audit_chain::{AuditChain, AuditChainWriter};
use shamir_server_security::audit_appender::FjallAuditAppender;
use tempfile::TempDir;

use super::super::failure_injection_tests::{run, write_file, InjectPort};
use super::audit_fixtures;
use super::audit_fixtures::{budget_left, helper_deadline, JoinOnDrop};
use crate::sync::durability::backup::online::copy_engine::{test_seams, CopyError};
use crate::sync::durability::backup::online::progress::{BackupStage, CancelFlag};
use crate::sync::durability::backup::online::protocol::{OnlineBackupError, OnlineBackupFailure};

/// Audit chain key seed of the rotating fixture (never leaves the test).
const ROTATION_KEY_SEED: u8 = 0x33;

/// Active audit log file name inside `data_dir`.
const ROTATION_AUDIT_LOG: &str = "audit.log";

/// One fixture audit append through the REAL chain writer.
fn append_rotation_event(writer: &AuditChainWriter, expect: &str) {
    writer
        .append(
            "auth_success",
            "test",
            "user",
            "127.0.0.0/24",
            [7; 8],
            "ok",
            vec![0xc0],
            42,
        )
        .expect(expect);
}

/// A rotation that lands BETWEEN the pass listing and the anchor-store
/// capture still yields a copy whose anchor covers its own log.
///
/// The copy is paused DETERMINISTICALLY (channel rendezvous, no sleeps) right
/// after the data-directory pass's listing, and while it is paused one event
/// is appended through the REAL appender configured to rotate on every append
/// (1-byte cap, so `rotate_locked` runs: fsync, rename, reopen a fresh empty
/// active log, dir sync, THEN both anchors). The fixture persists both anchors
/// itself — the appender checkpoint at its own initial checkpoint, and the
/// `server_meta` anchor after it — and, while the copy is paused, persists the
/// advanced `server_meta` anchor the way the server's meta hook does (after
/// the appender anchor). The copy's bytes are then moved into a fresh
/// directory and opened through the REAL boot recovery FIRST, with the copied
/// `server_meta` anchor as its third argument.
///
/// Mutant caught: the single-listing order (the audit directory listed once, at
/// the top of the pass, before the anchor capture). There the new archive is
/// absent from the listing while the advanced anchor and the fresh empty
/// active log ARE copied, so recovery refuses with `AnchorPastTail`. Mutant
/// caught: a copy that drops the `server_meta` anchor store or the advanced
/// anchor — `read_server_meta_anchor` or the anchor-pair equality fails.
#[test]
fn rotation_between_the_pass_listing_and_the_anchor_capture_restores_and_boots() {
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
    let key = audit_fixtures::audit_key(ROTATION_KEY_SEED);
    // Rotation on EVERY append: the append under the pause is a real rotation.
    let appender = FjallAuditAppender::open_strict_with_rotation(&source, Some(1), 0)
        .expect("open the rotating audit appender");
    let writer = AuditChainWriter::new(AuditChain::new(key), appender.clone());
    for _ in 0..2 {
        append_rotation_event(&writer, "append an audit event");
    }
    writer.checkpoint_now().expect("persist the audit anchor");
    // The server's meta hook persists the second anchor AFTER the appender
    // anchor. The live appender holds the checkpoint store, so the chain's own
    // checkpoint is read instead of reopening the store. The meta store is
    // opened ONCE and STAYS open (production keeps one handle for the life of
    // the server and writes every anchor through it), so the copy below sees a
    // store that only gains journal rows — never a close/reopen recovery.
    let anchor = writer.chain().checkpoint();
    let live_meta = audit_fixtures::LiveServerMeta::open(&source);
    live_meta.write_anchor(anchor);
    let live_meta_for_hook = live_meta.clone();

    let (paused_tx, paused_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    // ONE absolute deadline, taken BEFORE the helper is spawned: both
    // rendezvous receives and the teardown join draw on it, so a wedged helper
    // can never spend a budget per waiting step.
    let deadline = helper_deadline();
    let activity = JoinOnDrop::with_deadline(
        std::thread::Builder::new()
            .name("audit-rotation".to_string())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                if paused_rx.recv_timeout(budget_left(deadline)).is_ok() {
                    append_rotation_event(&writer, "append the rotating event");
                    writer.checkpoint_now().expect("persist the rotated anchor");
                    // The hook runs after the appender anchor at every
                    // rotation, so the advanced `server_meta` anchor is written
                    // through the OPEN handle, exactly as the server does.
                    let anchor = writer.chain().checkpoint();
                    live_meta_for_hook.write_anchor(anchor);
                } else {
                    panic!("the copy never reached the listing pause");
                }
                release_tx.send(()).expect("release the paused copy");
            })
            .expect("spawn the audit rotation thread"),
        deadline,
    );
    let listings = Arc::new(AtomicUsize::new(0));
    let listings_for_hook = listings.clone();
    let released = Arc::new(AtomicBool::new(false));
    let released_for_hook = released.clone();
    // Registered AFTER `activity`: on unwind locals drop in reverse order, so
    // this hook (which owns both channel ends the helper waits on) closes them
    // BEFORE the teardown join — the helper fails its rendezvous at once
    // instead of waiting out the shared budget.
    let seam = test_seams::set_after_listing(Box::new(move |_root| {
        // The protocol lists the data dir twice (pass D, then the residual
        // pass E); the rotation must land in the manifest-defining latter.
        if listings_for_hook.fetch_add(1, Ordering::SeqCst) != 1 {
            return;
        }
        paused_tx.send(()).expect("signal the listing pause");
        let released = release_rx.recv_timeout(budget_left(deadline)).is_ok();
        released_for_hook.store(released, Ordering::SeqCst);
    }));
    let port = InjectPort::with_repos(&source, &["alpha"]);
    let (result, _progress) = run(&port, &source, &to, &CancelFlag::default());
    drop(seam);
    activity.finish();
    assert_eq!(listings.load(Ordering::SeqCst), 2, "second listing");
    assert!(
        released.load(Ordering::SeqCst),
        "the listing-pause rendezvous must be released before the shared helper deadline"
    );
    let report = result.expect("the copy must succeed");
    // The copy is finished: release the live handle BEFORE any store is opened
    // again (the live source keeps the fjall lock, and the restore-side check
    // must read the COPIED tree, not this one).
    drop(live_meta);
    audit_fixtures::copy_snapshot_tree(&report.dest_dir, &restored);
    // Recovery FIRST: on the single-listing order the copy lacks the archive
    // the rotation created while it was paused, and the typed refusal
    // (`AnchorPastTail`) is what this expectation reports. The copied
    // `server_meta` anchor is handed to recovery; a copy that dropped
    // `server_meta/` panics inside `read_server_meta_anchor`.
    let meta = audit_fixtures::read_server_meta_anchor(&restored);
    let position = {
        let (recovered, position) =
            FjallAuditAppender::open_recovered_strict(&restored, &key, Some(meta)).expect(
                "the copied anchor must not be past the copied log tail (no AnchorPastTail)",
            );
        drop(recovered);
        position
    };
    // The recovered appender held the checkpoint store exclusively, so it was
    // released above before the anchor is read back through a second open.
    let anchor = FjallAuditAppender::load_checkpoint(&restored)
        .expect("read the restored anchor")
        .expect("the restored copy must carry the audit anchor");
    assert!(
        position.0 >= anchor.0,
        "recovery resumes at the verified tail {} at or after the anchor {}",
        position.0,
        anchor.0
    );
    // The restored chain is authentic only under the fixture's own key.
    assert!(
        FjallAuditAppender::open_recovered_strict(
            &restored,
            &audit_fixtures::audit_key(ROTATION_KEY_SEED + 1),
            Some(meta),
        )
        .is_err(),
        "a foreign audit chain key must fail recovery"
    );
    assert_eq!(
        audit_fixtures::read_server_meta_anchor(&source),
        meta,
        "the copy must carry the live source's advanced `server_meta` anchor"
    );
    let mut archives = 0usize;
    for entry in fs::read_dir(&restored).expect("restored directory") {
        let name = entry.expect("restored entry").file_name();
        let name = name.to_string_lossy().into_owned();
        if name
            .strip_prefix(ROTATION_AUDIT_LOG)
            .is_some_and(|suffix| suffix.starts_with('.'))
        {
            archives += 1;
        }
    }
    assert_eq!(
        archives, 3,
        "the copy must carry every rotation archive, including the one created while the copy was paused"
    );
    // The PUBLICATION must describe every archive the copy carried: the exact
    // path, digest and size in the decoded manifest.
    for entry in fs::read_dir(&restored).expect("restored directory") {
        let name = entry
            .expect("restored entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        let is_archive = name
            .strip_prefix(ROTATION_AUDIT_LOG)
            .is_some_and(|suffix| suffix.starts_with('.'));
        if !is_archive {
            continue;
        }
        let bytes = fs::read(restored.join(&name)).expect("restored archive bytes");
        assert_eq!(
            audit_fixtures::manifest_entry(&report.dest_dir, &name),
            (hex::encode(Sha256::digest(&bytes)), bytes.len() as u64),
            "the manifest must describe the archive {name} the copy carried"
        );
    }
    drop(appender);
}

/// A rotation inside the audit phase's corrective re-copy (the OPEN of the
/// re-copy) is counted MISSING for that round, the stability check re-lists,
/// the next round captures the new archive — and the run then refuses, because
/// the destination still holds the active-log copy the rotation made stale.
///
/// The rotation is armed only after the SECOND data-directory listing
/// (phase 1 of pass D, then of the residual pass E): the fault must land in
/// the pass whose result defines the manifest, not in pass D. The hash seam
/// reports a digest no copy can match, so the pass takes its corrective
/// re-copy; the source is gone when that copy opens it.
///
/// Mutant caught: propagating the vanished source's `CopyError::Io
/// { NotFound }` at the corrective OPEN — the run returns the I/O variant
/// instead of the typed lost-audit-file refusal below. Mutant caught: a pass
/// that publishes despite the stale destination copy — the run returns a report
/// and the refusal assertions never run.
#[test]
fn rotation_during_the_corrective_re_copy_is_missing_for_that_round_and_refuses_the_run() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let source = source_dir.path().to_path_buf();
    let to = to_dir.path().to_path_buf();
    audit_fixtures::seed_alpha_repo(&source);
    let log_for_hook = source.join(ROTATION_AUDIT_LOG);
    write_file(&log_for_hook, b"active-before-rotation\n");
    let archive = audit_fixtures::archive_name(1);
    let archive_for_hook = source.join(&archive);
    let armed = Arc::new(AtomicBool::new(false));
    let armed_for_listing = armed.clone();
    let listings = Arc::new(AtomicUsize::new(0));
    let listings_for_hook = listings.clone();
    let arm = test_seams::set_after_listing(Box::new(move |_root| {
        // Phase 1 of pass D, then phase 1 of the residual pass E.
        if listings_for_hook.fetch_add(1, Ordering::SeqCst) == 1 {
            armed_for_listing.store(true, Ordering::SeqCst);
        }
    }));
    let rotated = Arc::new(AtomicBool::new(false));
    let rotated_for_hook = rotated.clone();
    let armed_for_hash = armed.clone();
    let seam = test_seams::set_hash_file(Box::new(move |path| {
        if path != log_for_hook.as_path() || !armed_for_hash.load(Ordering::SeqCst) {
            return None;
        }
        if rotated_for_hook.swap(true, Ordering::SeqCst) {
            return None;
        }
        // The rotation: the active log is renamed into its archive and NOT
        // reopened (the window the appender closes with its reopen).
        fs::rename(&log_for_hook, &archive_for_hook).expect("rotate the active audit log");
        // The bytes that would have changed the file between its copy and
        // its snapshot hash: a digest no copy can match.
        Some(Ok(("0".repeat(64), 0)))
    }));
    let port = InjectPort::with_repos(&source, &["alpha"]);
    let (result, _progress) = run(&port, &source, &to, &CancelFlag::default());
    drop(seam);
    drop(arm);
    assert_eq!(
        listings.load(Ordering::SeqCst),
        2,
        "the fault must land in the residual pass (the second data-directory listing)"
    );
    assert!(
        rotated.load(Ordering::SeqCst),
        "the corrective branch must be reached in the residual pass"
    );
    let error = result.expect_err("a stale destination audit file must refuse the run");
    let OnlineBackupError::Failed {
        stage,
        source: primary,
        ..
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Sealing);
    assert!(
        matches!(
            primary,
            OnlineBackupFailure::Copy(CopyError::AuditArchiveLost)
        ),
        "the tolerated-vanish path must end in the typed lost-audit-file refusal, got {primary:?}"
    );
    assert_eq!(
        error.stage_and_reason(),
        "sealing: copy-audit-archive-lost",
        "the scheduled token must stay in the path-free copy vocabulary"
    );
    assert_eq!(
        fs::read_dir(&to).expect("destination root").count(),
        0,
        "R4: the refused snapshot must not survive in the destination"
    );
}

/// A rotation that lands between the corrective COPY and its follow-up RE-HASH
/// is tolerated in the audit phase, the next round captures the new archive —
/// and the run then refuses, because the destination still holds the
/// active-log copy the rotation made stale.
///
/// The `hash_file` seam stands for the append that changed the log between
/// its copy and its snapshot hash (an unmatchable digest), and on the SECOND
/// read — the re-hash inside the corrective loop — it renames the active log
/// into its archive and reports it gone.
///
/// Mutant caught: restoring `NotFound` propagation in the follow-up-hash arm
/// of the corrective re-copy (`copy_engine::recopy_until_stable`). That arm
/// is never reached by the source-open test, so this case is its own oracle:
/// the run returns `CopyError::Io { NotFound }` instead of the typed
/// lost-audit-file refusal below.
#[test]
fn rotation_between_the_corrective_copy_and_its_re_hash_refuses_the_run() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    // The port declares `repos/alpha.shamirwal`; without a WAL segment and its
    // sidecar the run dies in step C (opening the missing WAL directory) before
    // any audit hook fires.
    audit_fixtures::seed_alpha_repo(source);
    let log_path = source.join(ROTATION_AUDIT_LOG);
    write_file(&log_path, b"active-before-rotation\n");
    let archive = audit_fixtures::archive_name(1);
    let archive_path = source.join(&archive);
    // 0: the snapshot hash of the copy (mismatch forced); 1: the re-hash.
    let reads = Arc::new(AtomicUsize::new(0));
    let reads_for_hook = reads.clone();
    let log_for_hook = log_path.clone();
    let archive_for_hook = archive_path.clone();
    let armed = Arc::new(AtomicBool::new(false));
    let armed_for_listing = armed.clone();
    let listings = Arc::new(AtomicUsize::new(0));
    let listings_for_hook = listings.clone();
    let arm = test_seams::set_after_listing(Box::new(move |_root| {
        // Phase 1 of pass D, then phase 1 of the residual pass E: the fault
        // must land in the pass whose result defines the manifest.
        if listings_for_hook.fetch_add(1, Ordering::SeqCst) == 1 {
            armed_for_listing.store(true, Ordering::SeqCst);
        }
    }));
    let armed_for_hash = armed.clone();
    let seam = test_seams::set_hash_file(Box::new(move |path| {
        if path != log_for_hook.as_path() || !armed_for_hash.load(Ordering::SeqCst) {
            return None;
        }
        match reads_for_hook.fetch_add(1, Ordering::SeqCst) {
            0 => Some(Ok(("0".repeat(64), 0))),
            1 => {
                fs::rename(&log_for_hook, &archive_for_hook).expect("rotate the audit log");
                Some(Err(std::io::Error::from(std::io::ErrorKind::NotFound)))
            }
            _ => None,
        }
    }));
    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run(&port, source, to, &CancelFlag::default());
    drop(seam);
    drop(arm);
    assert_eq!(
        listings.load(Ordering::SeqCst),
        2,
        "the fault must land in the residual pass (the second data-directory listing)"
    );
    assert_eq!(
        reads.load(Ordering::SeqCst),
        2,
        "the residual pass must reach the copy hash and the corrective re-hash"
    );
    let error = result.expect_err("a stale destination audit file must refuse the run");
    let OnlineBackupError::Failed {
        stage,
        source: primary,
        ..
    } = &error
    else {
        panic!("expected Failed, got {error:?}");
    };
    assert_eq!(*stage, BackupStage::Sealing);
    assert!(
        matches!(
            primary,
            OnlineBackupFailure::Copy(CopyError::AuditArchiveLost)
        ),
        "the tolerated re-hash vanish must end in the typed lost-audit-file refusal, got {primary:?}"
    );
    assert_eq!(
        error.stage_and_reason(),
        "sealing: copy-audit-archive-lost",
        "the scheduled token must stay in the path-free copy vocabulary"
    );
    assert_eq!(
        fs::read_dir(to).expect("destination root").count(),
        0,
        "R4: the refused snapshot must not survive in the destination"
    );
}
