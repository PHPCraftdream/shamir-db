//! Regression tests for the reader side of the snapshot read lease
//! (briefs 75 + 82, findings 1 / 4 / 5 sync portion). Pure file APIs, no
//! runtime, no database handle.
//!
//! Red mechanisms (why each test fails without its fix):
//! * `missing_at_decision_refuses_even_when_child_exists` — the old
//!   `read_snapshot` returned `Ok(None)` on a `NotFound` lookup, so
//!   `verify_manifest` went on to open the ORIGINAL pathname unleased and
//!   SUCCEEDED; here the child is created, completely, INSIDE the lookup
//!   seam — before any manifest open — and the verification entry must
//!   still refuse with `ManifestMissing`.
//! * `shared_lock_through_read_only_marker_handle_conflicts_exclusive` —
//!   proves the shared lock taken through the read-only marker handle
//!   still conflicts with retention's independent exclusive attempt on all
//!   OSes (`fs4` 0.12.0: `flock(LOCK_SH)` on Unix, shared `LockFileEx` on
//!   Windows — both need only read access). The marker's write permission
//!   is RESTORED (while the already-open reader is live) before the
//!   exclusive attempt: a normal CI user could not open a write-capable
//!   handle on a still read-only marker and would see PermissionDenied
//!   instead of the lock conflict.
//! * `invalid_marker_fails_closed` — a non-plain/non-file marker leaf must
//!   error the decision, never classify as unmanaged.
//! * `older_sibling_verify_holds_shared_lease_writer_self_check_does_not`
//!   — through the existing `shamir_sync::snapshot_verify` trace
//!   subscriber rendezvous, an INDEPENDENT `try_exclusive` taken inside
//!   the manifest-read callback must CONFLICT while the old sibling's
//!   verifier is live (it holds the shared lease) but SUCCEED for the
//!   writer's self-check (unleased). Under the removed thread-scoped
//!   writer bypass (or an unleased sibling verify) the sibling probe
//!   succeeds — the red. Deterministic, no negative sleeps, no polling.

use std::cell::Cell;
use std::fs;
use std::io;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use tempfile::TempDir;
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

use crate::sync::durability::backup::offline::{
    verify_manifest_with_lookup, verify_writer_snapshot, BackupError, Manifest, ManifestFileEntry,
};
use crate::sync::durability::backup::online::copy_engine::hash_file;
use crate::sync::durability::backup::snapshot_lease;
use crate::sync::durability::backup::verify_manifest;

/// The marker name, spelled out so the tests fail loudly if it moves.
const MARKER: &str = ".shamir-snapshot.lock";

/// Minimal VALID v1 offline snapshot (verify refuses any unmanifested file,
/// so the set must match exactly: the listed file + `manifest.json`).
fn build_v1_snapshot(snapshot: &Path) {
    fs::create_dir_all(snapshot.join("users")).expect("snapshot users dir");
    fs::write(snapshot.join("users/keeper.bin"), b"kept-bytes").expect("write listed file");

    let (sha256, size_bytes) = hash_file(&snapshot.join("users/keeper.bin")).expect("hash file");
    let manifest = Manifest {
        format_version: 1,
        created_at_unix_ns: 1_700_000_000_000,
        files: vec![ManifestFileEntry {
            path: "users/keeper.bin".to_string(),
            sha256,
            size_bytes,
        }],
    };
    let json = serde_json::to_vec_pretty(&manifest).expect("encode manifest");
    fs::write(snapshot.join("manifest.json"), json).expect("write manifest");
}

/// A valid timestamped snapshot child of `root` plus a provisioned marker
/// (a managed root). Returns the snapshot path.
fn managed_root_with_snapshot(root: &Path, name: &str) -> PathBuf {
    let snapshot = root.join(name);
    build_v1_snapshot(&snapshot);
    snapshot_lease::provision(root).expect("provision managed root marker");
    snapshot
}

/// Finding 1: the snapshot path is ABSENT under the marked parent when the
/// verification entry makes its canonicalizing decision. The seam calls the
/// REAL `fs::canonicalize` (capturing the genuine `NotFound`), and INSIDE
/// the seam — before any manifest open can happen — creates the COMPLETE
/// snapshot. The reader must refuse with `ManifestMissing` even though the
/// child fully exists by the time the manifest would be opened; without the
/// fix the unleased fallback read of the original pathname succeeds.
#[test]
fn missing_at_decision_refuses_even_when_child_exists() {
    let root = TempDir::new().expect("temp root");
    snapshot_lease::provision(root.path()).expect("provision managed root marker");
    let snapshot = root.path().join("20200101_000001");
    assert!(!snapshot.exists(), "precondition: the child is absent");

    let seam_saw_missing = Cell::new(false);
    let created_in_seam = |path: &Path| {
        let looked_up = fs::canonicalize(path);
        assert_eq!(
            looked_up.as_ref().unwrap_err().kind(),
            io::ErrorKind::NotFound,
            "the seam must capture a genuine NotFound"
        );
        // The REAL canonicalize missed. Build the complete fixture right
        // here, before any manifest open can happen.
        build_v1_snapshot(&snapshot);
        seam_saw_missing.set(true);
        looked_up
    };

    let error = verify_manifest_with_lookup(&snapshot, created_in_seam)
        .expect_err("an absent-at-decision child must refuse even if created in the seam");
    assert!(
        matches!(error, BackupError::ManifestMissing(_)),
        "the refusal must be ManifestMissing, got {error:?}"
    );
    assert!(
        seam_saw_missing.get(),
        "the seam must have actually run (real NotFound captured)"
    );
    assert!(
        snapshot.join("manifest.json").is_file(),
        "the snapshot must have been created COMPLETE inside the seam"
    );
}

/// Finding 4 + lock primitive: the shared lease is taken through a marker
/// handle opened READ-ONLY, and it still conflicts with retention's
/// independent exclusive attempt (separate handle, same process — per
/// handle locks on both OSes). The marker's write permission is restored
/// while the already-open reader is live (a read-only marker would make the
/// write-capable exclusive open fail with PermissionDenied for a normal CI
/// user — the conflict, not the permission error, is what this asserts).
/// After the reader drops, the exclusive attempt succeeds.
#[test]
fn shared_lock_through_read_only_marker_handle_conflicts_exclusive() {
    let root = TempDir::new().expect("temp root");
    let snapshot = managed_root_with_snapshot(root.path(), "20200101_000003");
    let marker = root.path().join(MARKER);

    let original_permissions = capture_permissions(&marker);
    set_file_read_only(&marker);
    let lease = snapshot_lease::read_snapshot(&snapshot)
        .expect("shared lock must work through a READ-ONLY marker handle");
    let canonical = match &lease {
        snapshot_lease::SourceSnapshot::Managed { canonical, .. } => canonical.clone(),
        other => panic!("managed snapshot must lease, got {other:?}"),
    };
    assert_eq!(
        fs::canonicalize(&snapshot).expect("canonicalize"),
        canonical,
        "the reader must hold the RESOLVED canonical path"
    );

    // Restore the ORIGINAL permissions while the reader is STILL live, so
    // the exclusive attempt below tests the lock conflict, not file
    // permissions (the saved `fs::Permissions` are written back verbatim —
    // no `set_readonly(false)`).
    restore_file_permissions(&marker, original_permissions);
    let conflict =
        snapshot_lease::try_exclusive(root.path()).expect("independent exclusive attempt");
    assert!(
        conflict.is_none(),
        "exclusive attempt must refuse while the read-only-handle shared lease is held"
    );
    drop(lease);

    let again = snapshot_lease::try_exclusive(root.path())
        .expect("try_exclusive must not error once released");
    assert!(
        again.is_some(),
        "exclusive lease must be acquirable after the reader dropped"
    );
}

/// Fail closed: a marker leaf that is not a plain regular file must refuse
/// the reader decision with an error, never classify as unmanaged.
#[test]
fn invalid_marker_fails_closed() {
    let root = TempDir::new().expect("temp root");
    let snapshot = managed_root_with_snapshot(root.path(), "20200101_000004");
    // Replace the plain marker with a directory.
    fs::remove_file(root.path().join(MARKER)).expect("remove marker");
    fs::create_dir(root.path().join(MARKER)).expect("obstruct marker");

    let error =
        snapshot_lease::resolve_source(&snapshot).expect_err("an invalid marker must fail closed");
    assert_eq!(
        error.kind(),
        std::io::ErrorKind::InvalidData,
        "invalid marker must be InvalidData, got {error}"
    );
}

/// Probe subscriber: at the verifier's `shamir_sync::snapshot_verify`
/// manifest-read trace, takes an INDEPENDENT exclusive attempt on the root
/// and reports whether it CONFLICTED (`Ok(None)` = a shared lease is
/// genuinely held by the live verifier). One probe per verify under test,
/// so the sibling and the writer self-check are distinguished exactly.
struct ExclusiveProbe {
    root: PathBuf,
    conflicted: mpsc::SyncSender<bool>,
}

impl<S: Subscriber> Layer<S> for ExclusiveProbe {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() != "shamir_sync::snapshot_verify" {
            return;
        }
        let conflicted = match snapshot_lease::try_exclusive(&self.root) {
            Ok(None) => true,
            Ok(Some(_)) => false,
            Err(error) => panic!("probe exclusive attempt must not error: {error}"),
        };
        self.conflicted
            .try_send(conflicted)
            .expect("probe must report exactly once per verify");
    }
}

/// Finding 5: the OLD SIBLING's `verify_manifest` really holds the shared
/// lease — at its manifest-read callback an independent exclusive attempt
/// conflicts — while the writer's self-check of the fresh child holds NO
/// lease (the exclusive attempt succeeds). Under the removed thread-scoped
/// writer bypass the sibling verify is unleased and its probe succeeds:
/// the red. Both verifies run on THIS thread under their own subscriber;
/// the probe's exclusive attempt is an independent handle on the same
/// thread, which is exactly what must conflict. The rendezvous is the
/// trace event itself; the bounded `sync_channel(1)` buffer makes each
/// single report deliverable without a receiver parked in `recv`.
#[test]
fn older_sibling_verify_holds_shared_lease_writer_self_check_does_not() {
    let root = TempDir::new().expect("temp root");
    let older = managed_root_with_snapshot(root.path(), "20200101_000007");
    let fresh = managed_root_with_snapshot(root.path(), "20200101_000008");

    // Old sibling: leased verify; the probe fires inside its manifest read.
    let (probe_tx, probe_rx) = mpsc::sync_channel(1);
    let subscriber = tracing_subscriber::registry().with(ExclusiveProbe {
        root: root.path().to_path_buf(),
        conflicted: probe_tx,
    });
    tracing::subscriber::with_default(subscriber, || verify_manifest(&older))
        .expect("older sibling verifies under the shared lease");
    let sibling_holds_lease = probe_rx
        .try_recv()
        .expect("sibling verify must reach its manifest read");
    assert!(
        sibling_holds_lease,
        "the older sibling's verifier must hold the shared lease: an independent \
         exclusive attempt at its manifest-read callback must conflict"
    );

    // Writer self-check of the EXACT fresh child: unleased — the probe's
    // exclusive attempt must succeed (no shared lease held).
    let (probe_tx, probe_rx) = mpsc::sync_channel(1);
    let subscriber = tracing_subscriber::registry().with(ExclusiveProbe {
        root: root.path().to_path_buf(),
        conflicted: probe_tx,
    });
    tracing::subscriber::with_default(subscriber, || verify_writer_snapshot(&fresh))
        .expect("fresh child self-check must verify");
    let writer_holds_lease = probe_rx
        .try_recv()
        .expect("self-check must reach its manifest read");
    assert!(
        !writer_holds_lease,
        "the writer self-check must be unleased: an exclusive attempt at its \
         manifest-read callback must succeed"
    );
}

/// Read-only marker FILE on all OSes: Unix mode bits, Windows READONLY
/// attribute (`Permissions::set_readonly` maps to it).
fn set_file_read_only(path: &Path) {
    let mut permissions = fs::metadata(path).expect("marker metadata").permissions();
    #[cfg(unix)]
    permissions.set_mode(0o444);
    #[cfg(windows)]
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions).expect("read-only marker");
}

fn capture_permissions(path: &Path) -> fs::Permissions {
    fs::metadata(path).expect("marker metadata").permissions()
}

fn restore_file_permissions(path: &Path, original: fs::Permissions) {
    fs::set_permissions(path, original).expect("marker permissions restored");
}
