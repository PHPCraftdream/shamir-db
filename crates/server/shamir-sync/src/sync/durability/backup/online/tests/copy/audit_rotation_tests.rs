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
    let source_dir = Arc::new(TempDir::new().expect("source temp dir"));
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
    let source_dir_for_helper = source_dir.clone();
    let activity = JoinOnDrop::with_deadline(
        std::thread::Builder::new()
            .name("audit-rotation".to_string())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                // Keep the source tree alive even if the caller detaches this helper.
                let _source_dir = source_dir_for_helper;
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

#[test]
fn cancellation_during_active_log_reopen_check_wins_over_audit_archive_lost() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    audit_fixtures::seed_alpha_repo(source);
    let log_path = source.join(ROTATION_AUDIT_LOG);
    write_file(&log_path, b"active-before-rotation\n");
    let archive_path = source.join(audit_fixtures::archive_name(1));
    let log_for_hook = log_path.clone();
    let archive_for_hook = archive_path.clone();
    let armed = Arc::new(AtomicBool::new(false));
    let armed_for_listing = armed.clone();
    let listings = Arc::new(AtomicUsize::new(0));
    let listings_for_hook = listings.clone();
    let arm = test_seams::set_after_listing(Box::new(move |_root| {
        if listings_for_hook.fetch_add(1, Ordering::SeqCst) == 1 {
            armed_for_listing.store(true, Ordering::SeqCst);
        }
    }));
    let armed_for_hash = armed.clone();
    let rotated = Arc::new(AtomicBool::new(false));
    let rotated_for_hash = rotated.clone();
    let hash = test_seams::set_hash_file(Box::new(move |path| {
        if path == log_for_hook.as_path()
            && armed_for_hash.load(Ordering::SeqCst)
            && !rotated_for_hash.swap(true, Ordering::SeqCst)
        {
            fs::rename(&log_for_hook, &archive_for_hook).expect("rotate the active audit log");
            Some(Ok(("0".repeat(64), 0)))
        } else {
            None
        }
    }));
    let cancel = CancelFlag::default();
    let cancel_for_scan = cancel.clone();
    let scans = Arc::new(AtomicUsize::new(0));
    let scans_for_hook = scans.clone();
    let rotated_for_scan = rotated.clone();
    let destination_scan =
        test_seams::set_destination_audit_order(Box::new(move |_root, paths| {
            let saw_active_log = paths.iter().any(|path| {
                path.file_name().and_then(|name| name.to_str()) == Some(ROTATION_AUDIT_LOG)
            });
            // Only scans AFTER the rotation count: pass D's ordinary scan also
            // lists the active log. The fourth post-rotation scan is the
            // terminal stale-active attempt (three extra reopen rounds are
            // spent), so only the check before `AuditArchiveLost` can report it.
            if saw_active_log
                && rotated_for_scan.load(Ordering::SeqCst)
                && scans_for_hook.fetch_add(1, Ordering::SeqCst) == 3
            {
                cancel_for_scan.cancel();
            }
            paths
        }));
    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run(&port, source, to, &cancel);
    drop(destination_scan);
    drop(hash);
    drop(arm);
    assert!(rotated.load(Ordering::SeqCst), "the rotation hook must run");
    assert_eq!(
        scans.load(Ordering::SeqCst),
        4,
        "three reopen retries plus the terminal stale-active attempt scan after the rotation"
    );
    let error = result.expect_err("cancellation must win over the stale active-log refusal");
    assert!(
        matches!(
            error,
            OnlineBackupError::Cancelled {
                stage: BackupStage::Sealing,
                ..
            }
        ),
        "expected cancellation at Sealing, got {error:?}"
    );
}

/// The corrective-open miss is TOLERATED and the backup still publishes when
/// the appender's reopen closes the rotation window.
///
/// `rotate_locked` renames the active log and reopens it a moment later. Here
/// the rename lands between the active log's copy and its snapshot hash, so the
/// corrective open genuinely misses it (counted `missing`), and the reopened
/// file appears only AFTER the next round's listing: that round is stable and
/// still cannot list the active name, so the pass's bounded reopen attempts
/// (`ACTIVE_REOPEN_ROUNDS`) re-list, capture the reopened file and publish.
///
/// Mutant caught: refusing on the first stale-active observation (the previous
/// behaviour) — the run ends in `AuditArchiveLost` and `Done` is never reached;
/// dropping the reopen attempts' re-list — the refused run also never publishes
/// the reopened log, so the manifest assertion fails; propagating the vanished
/// source at the corrective OPEN — `CopyError::Io { NotFound }` instead.
#[test]
fn a_corrective_open_miss_publishes_when_the_rotation_reopens_the_active_log() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    audit_fixtures::seed_alpha_repo(source);
    let log_path = source.join(ROTATION_AUDIT_LOG);
    let log_bytes = b"active-before-rotation\n";
    write_file(&log_path, log_bytes);
    let archive = audit_fixtures::archive_name(1);
    let archive_path = source.join(&archive);
    let log_for_hook = log_path.clone();
    let archive_for_hook = archive_path.clone();
    let source_for_hook = source.to_path_buf();
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
    let reopened = Arc::new(AtomicBool::new(false));
    let armed_for_hash = armed.clone();
    let seam = test_seams::set_hash_file(Box::new(move |path| {
        if path != log_for_hook.as_path() || !armed_for_hash.load(Ordering::SeqCst) {
            return None;
        }
        if !rotated_for_hook.swap(true, Ordering::SeqCst) {
            // The rotation window: the active log becomes its archive and is
            // NOT reopened yet, and the copy's digest no longer matches.
            fs::rename(&log_for_hook, &archive_for_hook).expect("rotate the active audit log");
            return Some(Ok(("0".repeat(64), 0)));
        }
        None
    }));
    // The appender's reopen: it lands only after the NEXT audit round's
    // listing, so that round is stable and still cannot list the active name.
    let archive_for_capture = archive.clone();
    let reopened_for_capture = reopened.clone();
    let reopen = test_seams::set_after_capture(Box::new(move |path| {
        if path.file_name().and_then(|name| name.to_str()) != Some(archive_for_capture.as_str()) {
            return;
        }
        if reopened_for_capture.swap(true, Ordering::SeqCst) {
            return;
        }
        write_file(
            &source_for_hook.join(ROTATION_AUDIT_LOG),
            b"reopened-active\n",
        );
    }));
    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run(&port, source, to, &CancelFlag::default());
    drop(seam);
    drop(reopen);
    drop(arm);
    let report = result.expect("the reopened active log must let the backup finish");
    assert!(
        rotated.load(Ordering::SeqCst),
        "the fixture must take the corrective-open branch"
    );
    assert!(
        reopened.load(Ordering::SeqCst),
        "the appender's reopen must land after the next round's listing"
    );
    assert_eq!(
        listings.load(Ordering::SeqCst),
        2,
        "the protocol lists the data dir twice (pass D, then the residual pass E)"
    );
    let paths = audit_fixtures::manifest_paths(&report.dest_dir);
    assert!(
        paths.contains(&archive),
        "the archive the rotation created must be published: {paths:?}"
    );
    assert!(
        paths.contains(&ROTATION_AUDIT_LOG.to_string()),
        "the reopened active log must be published: {paths:?}"
    );
    assert_eq!(
        audit_fixtures::manifest_entry(&report.dest_dir, &archive),
        (
            hex::encode(Sha256::digest(log_bytes)),
            log_bytes.len() as u64
        ),
        "the manifest must describe the archived bytes exactly"
    );
    assert_eq!(
        fs::read(report.dest_dir.join(ROTATION_AUDIT_LOG)).expect("reopened log copy"),
        b"reopened-active\n",
        "the destination must carry the reopened active log, not the stale copy"
    );
}

#[test]
fn an_active_file_lost_before_the_accepted_listing_without_a_new_archive_is_lost() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    audit_fixtures::seed_alpha_repo(source);

    // Current guard contract is name-based: an accepted replacement named
    // `audit.log` is captured over D's file, so it cannot detect continuity
    // loss from differing bytes alone. Exercise its supported refusal case: the
    // captured D active file disappears before E's accepted audit listing.
    let log_path = source.join(ROTATION_AUDIT_LOG);
    let key = audit_fixtures::audit_key(ROTATION_KEY_SEED);
    let appender = FjallAuditAppender::open_strict(source).expect("open the real audit appender");
    let writer = AuditChainWriter::new(AuditChain::new(key), appender.clone());
    append_rotation_event(&writer, "append anchored audit history");
    writer
        .checkpoint_now()
        .expect("persist the audit checkpoint");
    audit_fixtures::persist_server_meta_anchor(source, writer.chain().checkpoint());
    let stale_active = fs::read(&log_path).expect("read the anchored active audit history");
    let archive_path = source.join(audit_fixtures::archive_name(1));
    assert!(!archive_path.exists(), "the source must have no archive");
    // Release the live writer and store lock before the protocol's source-file
    // mutation; the persisted checkpoint and server_meta anchors remain real.
    drop(writer);
    drop(appender);

    let listings = Arc::new(AtomicUsize::new(0));
    let listings_for_hook = listings.clone();
    let log_for_listing = log_path.clone();
    let remove_before_accepted_listing = test_seams::set_after_listing(Box::new(move |_root| {
        // Phase 1 of pass D, then phase 1 of residual pass E. Remove the
        // active source after E's phase-1 listing and before its audit listing.
        if listings_for_hook.fetch_add(1, Ordering::SeqCst) == 1 {
            fs::remove_file(&log_for_listing).expect("lose active source before E audit listing");
        }
    }));

    let stale_destination_preserved = Arc::new(AtomicBool::new(false));
    let stale_destination_preserved_for_scan = stale_destination_preserved.clone();
    let log_for_scan = log_path.clone();
    let replaced = Arc::new(AtomicBool::new(false));
    let replaced_for_scan = replaced.clone();
    let observe_destination =
        test_seams::set_destination_audit_order(Box::new(move |root, paths| {
            if !log_for_scan.exists() && !replaced_for_scan.swap(true, Ordering::SeqCst) {
                // E's guard scan sees the missing source after its phase-1
                // listing. Reappear under the same name without an archive.
                write_file(&log_for_scan, b"");
            }
            let destination_bytes =
                fs::read(root.join(ROTATION_AUDIT_LOG)).expect("D's destination active file");
            stale_destination_preserved_for_scan
                .store(destination_bytes == stale_active, Ordering::SeqCst);
            paths
        }));

    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run(&port, source, to, &CancelFlag::default());
    drop(observe_destination);
    drop(remove_before_accepted_listing);

    assert_eq!(
        listings.load(Ordering::SeqCst),
        2,
        "pass D and residual pass E"
    );
    assert!(
        stale_destination_preserved.load(Ordering::SeqCst),
        "the guard must observe D's real stale active bytes, without synthesizing an entry"
    );
    assert!(
        replaced.load(Ordering::SeqCst),
        "the E guard scan must re-create the active name without an archive"
    );
    let error = result.expect_err("the uncaptured D active file must refuse the run");
    assert!(
        matches!(
            &error,
            OnlineBackupError::Failed {
                stage: BackupStage::Sealing,
                source: OnlineBackupFailure::Copy(CopyError::AuditArchiveLost),
                ..
            }
        ),
        "an unlisted stale active destination file with no archive evidence must refuse: {error:?}"
    );
}
/// the reopen lands before the next listing.
///
/// The corrective copy itself succeeds; its follow-up hash is where the
/// rotation renames the active log and reports it gone (`Ok(None)` in
/// `recopy_until_stable`), so the file is counted `missing` for that round. The
/// reopened file exists by the next listing, which captures both the new
/// archive and the active log, and the run publishes.
///
/// Mutant caught: restoring `NotFound` propagation in the follow-up-hash arm —
/// the run returns `CopyError::Io { NotFound }`; dropping the reopen — the run
/// ends in `AuditArchiveLost`.
#[test]
fn a_corrective_re_hash_miss_publishes_when_the_rotation_reopens_the_active_log() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    audit_fixtures::seed_alpha_repo(source);
    let log_path = source.join(ROTATION_AUDIT_LOG);
    let log_bytes = b"active-before-rotation\n";
    write_file(&log_path, log_bytes);
    let archive = audit_fixtures::archive_name(1);
    let archive_path = source.join(&archive);
    let readings = Arc::new(AtomicUsize::new(0));
    let readings_for_hook = readings.clone();
    let log_for_hook = log_path.clone();
    let archive_for_hook = archive_path.clone();
    let source_for_hook = source.to_path_buf();
    let armed = Arc::new(AtomicBool::new(false));
    let armed_for_listing = armed.clone();
    let listings = Arc::new(AtomicUsize::new(0));
    let listings_for_hook = listings.clone();
    let arm = test_seams::set_after_listing(Box::new(move |_root| {
        if listings_for_hook.fetch_add(1, Ordering::SeqCst) == 1 {
            armed_for_listing.store(true, Ordering::SeqCst);
        }
    }));
    let armed_for_hash = armed.clone();
    let seam = test_seams::set_hash_file(Box::new(move |path| {
        if path != log_for_hook.as_path() || !armed_for_hash.load(Ordering::SeqCst) {
            return None;
        }
        match readings_for_hook.fetch_add(1, Ordering::SeqCst) {
            // The copy's snapshot hash no longer matches: corrective re-copy.
            0 => Some(Ok(("0".repeat(64), 0))),
            1 => {
                // The corrective re-hash: the rotation renames the log, and
                // the appender reopens it immediately.
                fs::rename(&log_for_hook, &archive_for_hook).expect("rotate the audit log");
                write_file(
                    &source_for_hook.join(ROTATION_AUDIT_LOG),
                    b"reopened-active\n",
                );
                Some(Err(std::io::Error::from(std::io::ErrorKind::NotFound)))
            }
            _ => None,
        }
    }));
    let port = InjectPort::with_repos(source, &["alpha"]);
    let (result, _progress) = run(&port, source, to, &CancelFlag::default());
    drop(seam);
    drop(arm);
    let report = result.expect("the reopened active log must let the backup finish");
    assert_eq!(
        readings.load(Ordering::SeqCst),
        3,
        "the residual pass must reach its two injected copy/re-hash hits and hash the reopened log in round 2"
    );
    assert_eq!(
        listings.load(Ordering::SeqCst),
        2,
        "the protocol lists the data dir twice (pass D, then the residual pass E)"
    );
    let paths = audit_fixtures::manifest_paths(&report.dest_dir);
    for expected in [archive.clone(), ROTATION_AUDIT_LOG.to_string()] {
        assert!(
            paths.contains(&expected),
            "the published manifest must list {expected}: {paths:?}"
        );
    }
    assert_eq!(
        audit_fixtures::manifest_entry(&report.dest_dir, &archive),
        (
            hex::encode(Sha256::digest(log_bytes)),
            log_bytes.len() as u64
        ),
        "the manifest must describe the archived bytes exactly"
    );
    assert_eq!(
        fs::read(report.dest_dir.join(ROTATION_AUDIT_LOG)).expect("reopened log copy"),
        b"reopened-active\n",
        "the destination must carry the reopened active log, not the stale copy"
    );
}
