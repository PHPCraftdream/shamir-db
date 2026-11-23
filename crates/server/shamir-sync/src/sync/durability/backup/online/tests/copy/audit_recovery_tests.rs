//! Audit-consistency fixtures of the online copy: a copy taken while the
//! audit chain is live restores and boots, the anchor never points past the
//! captured log tail, and both trusted anchors the copy must carry —
//! `audit_checkpoint` and `server_meta` — are captured before the active log.
//! The restore side is a plain snapshot-tree copy plus the Fjall `lock`
//! recreation the production restore performs; manifest verification,
//! staging and the swap have their own tests, and every assertion here
//! checks a manifest entry against the bytes it opens.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use sha2::{Digest, Sha256};
use shamir_connect::server::audit_chain::{AuditChain, AuditChainWriter};
use shamir_server_security::audit_appender::FjallAuditAppender;
use tempfile::TempDir;

#[cfg(unix)]
use std::os::unix::fs::symlink;

use super::super::failure_injection_tests::{run, write_file, InjectPort};
use super::audit_fixtures;
use super::audit_fixtures::{budget_left, helper_deadline, JoinOnDrop};
use crate::sync::durability::audit_chain_files::reconcile_destination_audit_files;
use crate::sync::durability::backup::online::copy_engine::test_seams;
use crate::sync::durability::backup::online::copy_engine::{
    run_data_dir_copy_pass, CopiedFile, CopyError, CopyOptions, CopyPassResult,
};
use crate::sync::durability::backup::online::manifest;
use crate::sync::durability::backup::online::progress::CancelFlag;
use crate::sync::durability::backup::verify_manifest;
use crate::sync::durability::backup::MANIFEST_FILE_NAME;

/// Per-fixture audit chain key seeds (never leave the fixture): every
/// recovery fixture picks its own, and `KEY_WRONG` is the key no fixture
/// uses, so a mutant that ignores the key cannot pass them all.
const KEY_ORDER: u8 = 0x11;
const KEY_PAUSED: u8 = 0x22;
const KEY_QUIET: u8 = 0x33;
const KEY_WRONG: u8 = 0x44;

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
///
/// BOTH trusted anchors are persisted: the appender wrote its own row at the
/// checkpoint, and the `server_meta` anchor then receives the same pair — the
/// order the server's meta hook uses (after the appender anchor). The live
/// appender holds the checkpoint store, so the pair is read from the chain
/// itself instead of reopening that store.
fn open_audit(
    dir: &Path,
    events: usize,
    key: [u8; 32],
) -> (Arc<FjallAuditAppender>, AuditChainWriter) {
    let appender = FjallAuditAppender::open_strict(dir).expect("open the durable audit appender");
    let writer = AuditChainWriter::new(AuditChain::new(key), appender.clone());
    append_cycles(&writer, 1, events);
    let anchor = writer.chain().checkpoint();
    audit_fixtures::persist_server_meta_anchor(dir, anchor);
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
/// This is NOT the production restore (`restore_with_probe`): manifest
/// verification, staging and the swap have their own tests. The audit tests
/// below compensate by asserting the manifest entry against the bytes they
/// open.
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
    // The snapshot omits every Fjall `lock`; the production restore recreates
    // it before opening a staged store, so the copy gets it back the same way.
    test_seams::recreate_fjall_locks(to);
}

/// The manifest's entry for one snapshot-relative path: (sha256, size).
fn manifest_entry(dest: &Path, rel: &str) -> (String, u64) {
    let raw = fs::read_to_string(dest.join(MANIFEST_FILE_NAME)).expect("read the manifest");
    let decoded = manifest::decode(&raw).expect("decode the manifest");
    decoded
        .files
        .iter()
        .find(|entry| entry.path == rel)
        .map(|entry| (entry.sha256.clone(), entry.size_bytes))
        .unwrap_or_else(|| panic!("the manifest must list {rel}"))
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
    let (appender, writer) = open_audit(&source, 2, audit_fixtures::audit_key(KEY_ORDER));
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

/// Shared body of the paused-copy cases: a copy taken while audit events keep
/// arriving restores and boots, and its manifest describes the log bytes the
/// copy actually carries.
///
/// The copy is paused DETERMINISTICALLY (channel rendezvous, no sleeps) in the
/// snapshot-defining data-directory pass, at the `after_capture` seam when
/// `pause_after_capture` is set and at the `after_copy` seam otherwise. While
/// it is paused three real append + checkpoint cycles run through the real
/// appender; the copy is then released, its bytes moved into a fresh directory
/// and opened through the real boot recovery FIRST. The restored log must
/// carry the appended records when the pause was before the log's snapshot
/// hash (the pass re-copies it) and may not carry them when the pause was
/// after the log's capture. The test models the restore as a plain tree copy
/// plus the Fjall lock files and opens recovery with the COPIED `server_meta`
/// anchor, so BOTH trusted anchors are exercised: the appender's
/// `audit_checkpoint` row and the server's metadata anchor the copy carries.
fn paused_audit_copy_restores_and_boots(pause_after_capture: bool) {
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
    let key = audit_fixtures::audit_key(KEY_PAUSED);
    let (appender, writer) = open_audit(&source, 2, key);

    let (paused_tx, paused_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let copies = Arc::new(AtomicUsize::new(0));
    let copies_for_hook = copies.clone();
    let audit_log_path = source.join(AUDIT_LOG_FILE);
    let released = Arc::new(AtomicBool::new(false));
    let released_for_hook = released.clone();
    // ONE absolute deadline, taken BEFORE the helper is spawned: both
    // rendezvous receives and the teardown join draw on it, so a wedged helper
    // can never spend a budget per waiting step.
    let deadline = helper_deadline();
    let pause: Box<dyn FnMut(&Path)> = Box::new(move |path| {
        if path != audit_log_path {
            return;
        }
        // The protocol copies the data dir twice (pass D and the
        // snapshot-defining residual pass E); the pause must land in the
        // latter, the copy the manifest describes.
        if copies_for_hook.fetch_add(1, Ordering::SeqCst) != 1 {
            return;
        }
        paused_tx.send(()).expect("signal the copy pause");
        released_for_hook.store(
            release_rx.recv_timeout(budget_left(deadline)).is_ok(),
            Ordering::SeqCst,
        );
    });
    // The copy runs on THIS thread (the seams are thread-local); the audit
    // activity runs on a helper thread while the copy is paused.
    let activity = JoinOnDrop::with_deadline(
        std::thread::Builder::new()
            .name("audit-activity".to_string())
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                if paused_rx.recv_timeout(budget_left(deadline)).is_ok() {
                    append_cycles(&writer, 3, 2);
                }
                let _ = release_tx.send(());
            })
            .expect("spawn the audit activity thread"),
        deadline,
    );
    // The seam is registered AFTER `activity`: on unwind locals drop in reverse
    // order, so this hook (which owns both channel ends the helper waits on)
    // closes them BEFORE the teardown join — the helper fails its rendezvous at
    // once instead of waiting out the shared budget.
    let seam = if pause_after_capture {
        test_seams::set_after_capture(pause)
    } else {
        test_seams::set_after_copy(pause)
    };
    let port = InjectPort::with_repos(&source, &["alpha"]);
    let cancel = CancelFlag::default();
    let (result, _progress) = run(&port, &source, &to, &cancel);
    drop(seam);
    activity.finish();
    assert_eq!(
        copies.load(Ordering::SeqCst),
        2,
        "the pause must land in the snapshot-defining copy (pass D, then pass E)"
    );
    assert!(
        released.load(Ordering::SeqCst),
        "the audit activity must finish while the copy is paused"
    );
    let report = result.expect("the copy must succeed");

    copy_tree(&report.dest_dir, &restored);
    // Recovery FIRST: a typed refusal must be observed, never pre-empted. The
    // recovered appender holds the checkpoint store exclusively, so it is
    // released before the anchor is read back through a second open.
    // The copied `server_meta` anchor is handed to recovery as its third
    // argument: `read_server_meta_anchor` panics when the copy dropped that
    // store, so a phase-1 mutant that skips it fails here.
    let meta = audit_fixtures::read_server_meta_anchor(&restored);
    let position = {
        let (recovered, position) =
            FjallAuditAppender::open_recovered_strict(&restored, &key, Some(meta))
                .expect("the restored copy must pass audit recovery (no AnchorPastTail)");
        drop(recovered);
        position
    };
    // Negative control: the chain is authentic only under the fixture's key.
    assert!(
        FjallAuditAppender::open_recovered_strict(
            &restored,
            &audit_fixtures::audit_key(KEY_WRONG),
            Some(meta),
        )
        .is_err(),
        "a foreign audit chain key must fail recovery"
    );
    let captured_meta = audit_fixtures::manifest_paths(&report.dest_dir)
        .into_iter()
        .find(|path| path.starts_with("server_meta/keyspaces/"));
    assert!(
        captured_meta.is_some(),
        "phase 1 must capture the real server_meta keyspace files, not just its name"
    );

    let anchor = FjallAuditAppender::load_checkpoint(&restored)
        .expect("read the restored anchor")
        .expect("the restored copy must carry the audit anchor");
    let entries =
        FjallAuditAppender::read_log_for_verify(&restored).expect("read the restored audit log");
    AuditChain::verify_chain(&key, &entries).expect("the restored log must verify");
    let live = FjallAuditAppender::read_log_for_verify(&source)
        .expect("read the live audit log")
        .len();
    if pause_after_capture {
        assert!(
            entries.len() < live,
            "records appended after the log's capture are not in the snapshot"
        );
    } else {
        assert_eq!(
            entries.len(),
            live,
            "the pass must re-copy the log that grew before its hash"
        );
    }
    let tail = entries.last().expect("the restored log must not be empty");
    assert!(
        anchor.0 <= tail.seq + 1,
        "the captured anchor {:?} must not point past the captured log tail {} (recovery resumed at {})",
        anchor.0,
        tail.seq + 1,
        position.0
    );
    let covered = entries
        .iter()
        .find(|entry| entry.seq + 1 == anchor.0)
        .expect("the captured log must reach the anchor position");
    assert_eq!(
        covered.hmac, anchor.1,
        "the anchor must authenticate the captured log at its own position"
    );
    assert!(
        position.0 >= anchor.0,
        "recovery resumes at the verified tail {} at or after the anchor {}",
        position.0,
        anchor.0
    );
    let bytes = fs::read(restored.join(AUDIT_LOG_FILE)).expect("restored log bytes");
    assert_eq!(
        manifest_entry(&report.dest_dir, AUDIT_LOG_FILE),
        (hex::encode(Sha256::digest(&bytes)), bytes.len() as u64),
        "the manifest must describe the log bytes the copy carries, not an earlier read"
    );
    drop(appender);
}

/// Pause right after the destination write of the final `audit.log` copy and
/// BEFORE that file's snapshot hash: the log grows between its copy and its
/// hash and the pass must re-copy and hash the FINAL bytes.
///
/// Mutant caught: a manifest (or hash pass) that records the digest read at
/// copy time, before those appends — the run's own step-F self-verification
/// or the manifest assertion fails; a pass that skips the re-copy — the
/// restored log lacks the appended records.
#[test]
fn copy_paused_between_the_audit_log_copy_and_its_hash_restores_and_boots() {
    paused_audit_copy_restores_and_boots(false);
}

/// Pause right after the log's snapshot entry is FINAL: the appends and their
/// checkpoints land after the log was captured, so only an anchor captured
/// BEFORE the log is consistent with it.
///
/// Mutant caught: a capture order that takes `audit.log` before the anchor
/// stores (the sorted-path order of the previous engine): the stores' later
/// capture takes the anchor of the appended cycles while the log keeps its
/// earlier tail, and recovery refuses with `AnchorPastTail`. The order itself
/// is also pinned by `audit_log_is_captured_after_its_anchor_store`.
#[test]
fn copy_paused_after_the_audit_log_capture_restores_and_boots() {
    paused_audit_copy_restores_and_boots(true);
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
    let key = audit_fixtures::audit_key(KEY_QUIET);
    let (appender, writer) = open_audit(&source, 2, key);
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
    let bytes = fs::read(restored.join(AUDIT_LOG_FILE)).expect("restored log bytes");
    assert_eq!(
        manifest_entry(&report.dest_dir, AUDIT_LOG_FILE),
        (hex::encode(Sha256::digest(&bytes)), bytes.len() as u64),
        "the manifest must describe the copied log bytes exactly"
    );
    let entries =
        FjallAuditAppender::read_log_for_verify(&restored).expect("read the restored audit log");
    AuditChain::verify_chain(&key, &entries).expect("the restored log must verify");
    let tail = entries.last().expect("the audit log must not be empty");
    let anchor = FjallAuditAppender::load_checkpoint(&restored)
        .expect("read the restored anchor")
        .expect("the restored copy must carry the audit anchor");
    assert_eq!(
        anchor,
        (tail.seq + 1, tail.hmac),
        "the captured anchor must be exactly the captured log's tail"
    );
    let meta = audit_fixtures::read_server_meta_anchor(&restored);
    assert!(
        FjallAuditAppender::open_recovered_strict(
            &restored,
            &audit_fixtures::audit_key(KEY_WRONG),
            Some(meta),
        )
        .is_err(),
        "a foreign audit chain key must fail recovery"
    );
    let (recovered, position) =
        FjallAuditAppender::open_recovered_strict(&restored, &key, Some(meta))
            .expect("the restored copy must pass audit recovery");
    assert_eq!(
        position.0, anchor.0,
        "recovery resumes at the captured anchor"
    );
    drop(recovered);
    let captured_meta = audit_fixtures::manifest_paths(&report.dest_dir)
        .into_iter()
        .find(|path| path.starts_with("server_meta/keyspaces/"));
    assert!(
        captured_meta.is_some(),
        "phase 1 must capture the real server_meta keyspace files, not just its name"
    );
    drop(appender);
}

/// A rotation that renamed `audit.log` and has not reopened it leaves the
/// source with archives and no active file, while the destination still holds
/// the active log an earlier pass of the same copy made. That stale file has
/// no manifest entry and would fail the self-verification of an otherwise
/// healthy copy, so the audit phase removes it.
///
/// Mutant caught: a pass that keeps the earlier destination copy.
#[test]
fn audit_phase_removes_the_destination_active_log_the_source_no_longer_has() {
    let source_dir = TempDir::new().expect("source temp dir");
    let to_dir = TempDir::new().expect("destination temp dir");
    let (source, to) = (source_dir.path(), to_dir.path());
    let archive = "audit.log.00000000000000000001";
    write_file(&source.join("repos/alpha/store.bin"), b"store-bytes");
    write_file(&source.join(archive), b"archived\n");
    write_file(&to.join(AUDIT_LOG_FILE), b"stale-active\n");
    let cancel = CancelFlag::default();
    let result = run_data_dir_copy_pass(source, to, &[], &cancel, &CopyOptions::default())
        .expect("the data directory pass must succeed");
    let captured: Vec<&str> = result
        .files
        .iter()
        .map(|file| file.rel_path.as_str())
        .collect();
    assert!(captured.contains(&archive), "archive missing: {captured:?}");
    assert!(
        !captured.contains(&AUDIT_LOG_FILE),
        "no active log was captured: {captured:?}"
    );
    assert!(
        !to.join(AUDIT_LOG_FILE).exists(),
        "the stale destination active log must be removed"
    );
    assert_eq!(
        fs::read(to.join(archive)).expect("archive copy"),
        b"archived\n"
    );
}

/// The tightened fixture fingerprint rejects an IMPOSTOR store that the old
/// `version`-exists check accepted, and still accepts a genuine one.
///
/// Mutant caught: the previous fingerprint (`version` file + `keyspaces`
/// directory, no header check) stamps a `lock` into the impostor, so the
/// negative assertion fails.
#[test]
fn the_fjall_fixture_fingerprint_rejects_an_impostor_store() {
    let root_dir = TempDir::new().expect("fixture temp dir");
    let root = root_dir.path();
    fs::create_dir_all(root.join("fake/keyspaces")).expect("impostor keyspaces");
    fs::write(root.join("fake/version"), b"FJL\x02not-a-store").expect("impostor version");
    fs::create_dir_all(root.join("real/keyspaces")).expect("genuine keyspaces");
    fs::write(root.join("real/version"), b"FJL\x03payload").expect("genuine version");
    test_seams::recreate_fjall_locks(root);
    assert!(
        root.join("real/lock").is_file(),
        "a genuine Fjall store must get its lock artifact"
    );
    assert!(
        !root.join("fake/lock").exists(),
        "an impostor must be rejected by the tightened fingerprint"
    );
}

/// One captured file of a synthetic accepted snapshot (the hash is not read by
/// reconciliation).
fn copied(rel_path: &str) -> CopiedFile {
    CopiedFile {
        rel_path: rel_path.to_string(),
        sha256: String::new(),
        size_bytes: 0,
    }
}

/// Reconciliation is the ONE step that deletes from the destination, so the
/// run directory must itself be a plain directory: a symlinked run directory
/// would redirect every removal outside the snapshot, and is refused BEFORE
/// any removal happens.
///
/// Unix-only: the fixture needs a real symlink (`std::os::unix::fs::symlink`).
/// A Windows junction is the same threat through the same `symlink_metadata`
/// check, but creating one needs a privilege the CI user may not hold.
///
/// Mutant caught: removing by `fs::read_dir(dest_root)` without checking
/// `dest_root` itself — the stale archive inside the link target is deleted
/// and the call returns `Ok`.
#[cfg(unix)]
#[test]
fn a_symlinked_run_directory_is_refused_before_any_removal() {
    let target = TempDir::new().expect("link target");
    let parent = TempDir::new().expect("link parent");
    let stale = target.path().join(audit_fixtures::archive_name(1));
    write_file(&stale, b"stale\n");
    let run_dir = parent.path().join("run");
    symlink(target.path(), &run_dir).expect("symlinked run directory");

    let error = reconcile_destination_audit_files(&run_dir, &CopyPassResult::default(), &[])
        .expect_err("a symlinked run directory must be refused");

    assert!(
        matches!(error, CopyError::AuditReconcileRefused { .. }),
        "the refusal must be the typed reconcile error, got {error:?}"
    );
    assert!(
        stale.exists(),
        "nothing may be removed through the symlinked run directory"
    );
}

/// A captured path is NEVER removed, even when it carries a reserved audit
/// name: the protection covers the WHOLE accepted snapshot (the files of the
/// earlier phases), not only the accepted audit round's result.
///
/// Mutant caught: comparing the destination against the accepted audit round
/// alone (the previous `captured.files`-only protection) — the captured file
/// is deleted and the published snapshot loses a file its own manifest lists,
/// which self-verification turns into a failed run at `Verifying`.
#[test]
fn a_captured_phase_one_file_is_never_removed_by_reconciliation() {
    let dest = TempDir::new().expect("destination");
    let archive = audit_fixtures::archive_name(1);
    write_file(&dest.path().join(&archive), b"phase-one\n");

    reconcile_destination_audit_files(dest.path(), &CopyPassResult::default(), &[copied(&archive)])
        .expect("a captured path must never be removed");

    assert_eq!(
        fs::read(dest.path().join(&archive)).expect("the captured file must survive"),
        b"phase-one\n"
    );
}

/// A case-folding destination that maps a destination audit name onto a
/// captured path of another spelling is refused, not guessed at.
///
/// Phase 1 can capture a case-renamed archive (`AUDIT.LOG.` + 20 digits) as an
/// ordinary file beside the archive phase 2 lists; on a destination that folds
/// case those two manifest paths are ONE file, so "delete what the accepted
/// round did not capture" would delete a file the manifest lists.
///
/// Mutant caught: deleting on an exact-name miss alone — the reserved
/// destination name is removed and the call returns `Ok`.
#[test]
fn a_case_folded_destination_name_collision_is_refused() {
    let dest = TempDir::new().expect("destination");
    let archive = audit_fixtures::archive_name(1);
    write_file(&dest.path().join(&archive), b"stale\n");
    let renamed = archive.to_ascii_uppercase();

    let error = reconcile_destination_audit_files(
        dest.path(),
        &CopyPassResult::default(),
        &[copied(&renamed)],
    )
    .expect_err("a case-folded collision must be refused");

    assert!(
        matches!(error, CopyError::AuditReconcileRefused { .. }),
        "the refusal must be the typed reconcile error, got {error:?}"
    );
    assert!(
        dest.path().join(&archive).exists(),
        "nothing may be removed when the destination name is ambiguous"
    );
}

/// A captured set that folds two reserved audit spellings onto one name cannot
/// be satisfied on any case-folding destination, so reconciliation refuses
/// before it enumerates anything.
///
/// Mutant caught: building the folded map by overwriting the earlier spelling —
/// no refusal, and one of the two captured files is deleted from the
/// destination afterwards.
#[test]
fn two_captured_spellings_of_one_audit_name_are_refused() {
    let dest = TempDir::new().expect("destination");
    let archive = audit_fixtures::archive_name(1);
    let lowered = archive.to_ascii_uppercase();

    let error = reconcile_destination_audit_files(
        dest.path(),
        &CopyPassResult::default(),
        &[copied(&archive), copied(&lowered)],
    )
    .expect_err("two captured spellings of one audit name must be refused");

    assert!(
        matches!(error, CopyError::AuditReconcileRefused { .. }),
        "the refusal must be the typed reconcile error, got {error:?}"
    );
    assert_eq!(
        fs::read_dir(dest.path()).expect("destination").count(),
        0,
        "a refused reconciliation must touch nothing"
    );
}
