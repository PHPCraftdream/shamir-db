//! The online writer's lease behavior on a MARKED root (brief 82,
//! finding 5, CI only): a direct caller of `run_online_backup` must not
//! wait behind retention. Reuses the `protocol_tests` fixture (made
//! `pub(super)` for this file only).

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use tempfile::TempDir;

use crate::sync::durability::backup::online::copy_engine::test_seams;
use crate::sync::durability::backup::online::progress::{BackupStage, CancelFlag};
use crate::sync::durability::backup::snapshot_lease;
use crate::sync::durability::backup::verify_manifest;

use super::protocol_tests::{run, TestPort};

/// Bounded completion wait for a worker thread (generous: the Windows CI
/// runner is up to 10x slower) — never an unbounded join.
const LEASE_WAIT: Duration = Duration::from_secs(120);

/// The writer run COMPLETES while a real exclusive delete lease (an
/// independent handle on the marked root's marker) is held: the protocol's
/// private self-check is the unleased `verify_writer_snapshot`, so a
/// direct caller never waits behind retention. The run executes in a
/// scoped thread with a bounded completion wait; the `delete_lease` is
/// held as `Option<File>` and taken on EVERY release path (success and
/// timeout) so the lease is ALWAYS released before the scope joins — the
/// worker's self-check then completes and the scope can never wedge.
/// Red without the fix: the old un-bypassed self-check (`verify_manifest`)
/// blocks on the root's shared lock behind the held exclusive lease and
/// times out. Both directories are `TempDir` RAII OUTSIDE the scope,
/// borrowed into the closure via `.path()` — no manual recursive removal,
/// no leaked temp directories.
#[test]
fn writer_run_completes_while_exclusive_delete_lease_is_held() {
    let source = TempDir::new().expect("source temp dir");
    let to = TempDir::new().expect("destination temp dir");
    let source = source.path();
    let to = to.path();
    TestPort::source_with_data(source);
    snapshot_lease::provision(to).expect("provision the marked root marker");
    let mut delete_lease = Some(
        snapshot_lease::try_exclusive(to)
            .expect("exclusive attempt must not error")
            .expect("delete lease must be free at test start"),
    );
    let cancel = CancelFlag::default();
    let port = TestPort::single_repo(source);
    let (tx, rx) = mpsc::sync_channel(1);
    thread::scope(|scope| {
        let worker = scope.spawn(|| {
            let _ = tx.send(run(&port, source, to, &cancel));
        });
        let report = match rx.recv_timeout(LEASE_WAIT) {
            Ok(report) => report.expect("run_online_backup must succeed unleased"),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Always release the lease before joining on timeout: the
                // worker's self-check then completes and the scope ends.
                drop(delete_lease.take());
                panic!(
                    "run_online_backup must complete while an exclusive delete lease is held; \
                     after release the worker finished with {:?}",
                    rx.recv_timeout(LEASE_WAIT)
                );
            }
            Err(error) => panic!("worker channel failed: {error}"),
        };
        drop(delete_lease.take());
        // The published snapshot must verify through the NORMAL leased
        // entry once the exclusive lease is released (independent
        // classification, not the writer's unleased self-check).
        verify_manifest(&report.dest_dir)
            .expect("snapshot must verify once the delete lease is released");
        worker.join().expect("writer worker must not panic");
    });
}

/// Step-F self-verification must NOT pre-hash the manifest entries
/// through the copy engine before the offline verifier runs: the
/// verifier (`verify_writer_snapshot` -> `verify_decoded_manifest_v2`)
/// already re-hashes every `files` entry with the same size+sha256
/// comparison, so the removed pre-pass loop was a second full hash of
/// the same bytes. The `hash_file` seam fires ONLY for copy-engine
/// hashes — the verifier hashes through `hash_file_streaming`, which
/// has no seam — so counting seam hits after the `Verifying` boundary
/// observes exactly the removed loop: zero after the fix, one per
/// manifest entry (>= 1) on the parent commit.
#[test]
fn self_verify_hashes_manifest_entries_only_through_the_verifier() {
    let source = TempDir::new().expect("source temp dir");
    let to = TempDir::new().expect("destination temp dir");
    let (source, to) = (source.path(), to.path());
    TestPort::source_with_data(source);
    let port = TestPort::single_repo(source);
    let cancel = CancelFlag::default();
    let in_verify = Arc::new(AtomicBool::new(false));
    let verify_hashes = Arc::new(AtomicUsize::new(0));
    let _boundary: test_seams::SeamGuard = test_seams::set_before_boundary({
        let in_verify = Arc::clone(&in_verify);
        Box::new(move |stage: BackupStage| {
            if matches!(stage, BackupStage::Verifying) {
                in_verify.store(true, Ordering::SeqCst);
            }
        })
    });
    let _hashes: test_seams::SeamGuard = test_seams::set_hash_file({
        let in_verify = Arc::clone(&in_verify);
        let verify_hashes = Arc::clone(&verify_hashes);
        Box::new(
            move |_path: &Path| -> Option<std::io::Result<(String, u64)>> {
                if in_verify.load(Ordering::SeqCst) {
                    verify_hashes.fetch_add(1, Ordering::SeqCst);
                }
                None
            },
        )
    });
    let report = run(&port, source, to, &cancel).expect("run");
    verify_manifest(&report.dest_dir).expect("snapshot must self-verify");
    assert_eq!(
        verify_hashes.load(Ordering::SeqCst),
        0,
        "the verify stage must not re-hash manifest entries through the copy engine \
         before the offline verifier"
    );
}
