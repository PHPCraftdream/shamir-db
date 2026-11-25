use std::collections::BTreeSet;
use std::fs;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::symlink;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

use tracing::{Event, Subscriber};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::Layer;

use crate::config::BackupRetentionConfig;
use crate::sync::durability::backup::online::manifest::{self, BACKUP_META_FILE_NAME};
use crate::sync::durability::backup::online::progress::CancelFlag;
use crate::sync::durability::backup::{
    try_snapshot_delete_lease, verify_manifest, MANIFEST_FILE_NAME, SNAPSHOT_LEASE_FILE_NAME,
};

use super::super::super::retention_pass::manifest_digest_hex;
use super::super::budget::{Budget, ROOT_ENTRIES};
use super::super::ownership::{tag_fresh_with_seam, STAGED_PREFIX};
use super::super::prune::prune_inner;
use super::super::{prune, RetentionOutcome};
#[cfg(unix)]
use super::fixture::private_root_in;
use super::fixture::{copy_with_payload, owner_tag, private_root, retain, sidecar, OWNER};

/// A v2 online copy with an empty WAL segment, untagged or owned by `owner`.
fn copy(root: &Path, name: &str, time: u128, owner: Option<&str>) -> PathBuf {
    copy_with_payload(root, name, time, owner, b"")
}

/// SHA-256 (lowercase hex) of the exact manifest bytes of a fixture copy:
/// what the registry would have captured for it at completion.
fn manifest_digest(dir: &Path) -> String {
    manifest_digest_hex(&fs::read(dir.join(MANIFEST_FILE_NAME)).expect("read manifest"))
}

fn tracked_name(dir: &Path) -> String {
    dir.file_name()
        .expect("snapshot dir name")
        .to_string_lossy()
        .into_owned()
}

fn retain_pending(
    root: &Path,
    fresh: &Path,
    keep_last: usize,
    pending: &[(String, u128, String)],
) -> RetentionOutcome {
    let pending: BTreeSet<(String, u128, String)> = pending.iter().cloned().collect();

    prune(
        root,
        fresh,
        OWNER,
        &BackupRetentionConfig { keep_last },
        &CancelFlag::default(),
        &pending,
    )
}

const VERIFY_TIMEOUT: Duration = Duration::from_secs(120);

struct MutateManifestBeforeAdoption {
    manifest: PathBuf,
    changed: Arc<AtomicBool>,
}

impl<S: Subscriber> Layer<S> for MutateManifestBeforeAdoption {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() == "shamir_server::retention_before_adoption" {
            fs::OpenOptions::new()
                .append(true)
                .open(&self.manifest)
                .expect("open scanned manifest")
                .write_all(b"\n")
                .expect("mutate manifest after scan");
            self.changed.store(true, Ordering::SeqCst);
        }
    }
}

struct PauseAfterManifest {
    loaded: mpsc::SyncSender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}

impl<S: Subscriber> Layer<S> for PauseAfterManifest {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() == "shamir_sync::snapshot_verify" {
            self.loaded.try_send(()).expect("signal manifest loaded");
            self.resume
                .lock()
                .expect("resume receiver")
                .recv_timeout(VERIFY_TIMEOUT)
                .expect("resume verifier after prune");
        }
    }
}

#[test]
fn retention_skips_snapshot_while_verify_is_reading() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let fresh = copy(&root, "20200101_000002", 2, None);

    // Scheduler semantics (readers never create the marker): retention
    // provisions the marker BEFORE the reader starts, so the reader leases
    // the snapshot instead of reading it lock-free as an export.
    let marker = root.join(SNAPSHOT_LEASE_FILE_NAME);
    crate::sync::durability::backup::provision_snapshot_lease(&root).expect("provision the marker");
    assert!(marker.is_file(), "marker exists before the reader");

    let (first, report) = thread::scope(|scope| {
        let (loaded_tx, loaded_rx) = mpsc::sync_channel(1);
        let (resume_tx, resume_rx) = mpsc::sync_channel(1);
        let snapshot = &old;
        let reader = scope.spawn(move || {
            let subscriber = tracing_subscriber::registry().with(PauseAfterManifest {
                loaded: loaded_tx,
                resume: Mutex::new(resume_rx),
            });
            tracing::subscriber::with_default(subscriber, || verify_manifest(snapshot))
        });

        let first = loaded_rx
            .recv_timeout(VERIFY_TIMEOUT)
            .map(|()| retain(&root, &fresh, 1));
        let resumed = resume_tx.try_send(());
        drop(resume_tx);
        let report = reader.join();

        let first = first.expect("verifier loaded the manifest before prune");
        resumed.expect("release paused verifier");
        let report = report.expect("verifier thread panicked");
        (first, report)
    });

    assert_eq!(
        (first.deleted, first.failed, first.skipped_in_use),
        (0, 0, 1),
        "retention must skip the snapshot while verification is paused"
    );
    assert_eq!(report.expect("snapshot still verifies").files_checked, 1);
    assert!(marker.is_file(), "marker remains after the skip");

    let next = retain(&root, &fresh, 1);
    assert_eq!((next.deleted, next.failed, next.skipped_in_use), (1, 0, 0));
    assert!(!old.exists());
    assert!(fresh.is_dir());
    assert!(marker.is_file());
}

#[test]
fn keeps_foreign_unverified_and_unknown_entries_even_when_old() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let other = copy(&root, "20200101_000002", 2, Some("other-owner"));
    let admin = copy(&root, "20200101_000003", 3, None);
    let wrong_name = copy(&root, "20200230_000004", 4, Some(OWNER));
    let corrupt = copy(&root, "20200101_000005", 5, Some(OWNER));
    fs::write(corrupt.join("store.shamirwal/00000000.wal"), b"corrupt").expect("damage WAL");
    assert!(verify_manifest(&corrupt).is_err());
    let unknown = copy(&root, "20200101_000006", 6, Some(OWNER));
    fs::create_dir(unknown.join("empty-foreign-subtree")).expect("unknown directory");
    verify_manifest(&unknown).expect("verifier ignores empty directories");
    let extra_file = copy(&root, "20200101_000007", 7, Some(OWNER));
    fs::write(extra_file.join("operator-notes"), b"notes").expect("extra file");
    let foreign = root.join("20200101_000008");
    fs::write(&foreign, b"foreign").expect("foreign file");
    let floor = copy(&root, "20200101_000009", 9, Some(OWNER));
    let fresh = copy(&root, "20200101_000010", 10, None);
    let original_sidecar = sidecar(&fresh);
    let original_manifest = fs::read(fresh.join(MANIFEST_FILE_NAME)).expect("read manifest");

    let outcome = retain(&root, &fresh, 2);
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (1, 0, Some(7))
    );
    assert!(!old.exists());
    for path in [
        other,
        admin,
        wrong_name,
        corrupt,
        unknown,
        extra_file,
        foreign,
        floor,
        fresh.clone(),
    ] {
        assert!(path.exists(), "preserved {path:?}");
    }
    assert_eq!(
        fs::read(fresh.join(MANIFEST_FILE_NAME)).expect("manifest"),
        original_manifest
    );
    let mut tagged = sidecar(&fresh);
    assert_eq!(
        tagged
            .as_object_mut()
            .expect("object")
            .remove("scheduled_retention"),
        Some(owner_tag(OWNER))
    );
    assert_eq!(tagged, original_sidecar);
    verify_manifest(&fresh).expect("still verifies after tagging");
}

#[test]
fn keeps_skewed_fresh_copy_outside_floor_and_breaks_ties_by_name() {
    let (_temp, root) = private_root();
    let keep = copy(&root, "20200101_000003", 2, Some(OWNER));
    let delete = copy(&root, "20200101_000002", 2, Some(OWNER));
    let fresh = copy(&root, "20200101_000001", 1, None);
    let outcome = retain(&root, &fresh, 1);
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (1, 0, Some(0))
    );
    assert!(!delete.exists());
    assert!(keep.exists());
    assert!(fresh.exists());
}

#[test]
fn unverifiable_fresh_copy_prevents_tagging_and_pruning() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let _newer = copy(&root, "20200101_000002", 2, Some(OWNER));
    let fresh = copy(&root, "20200101_000003", 3, None);
    fs::write(fresh.join("store.shamirwal/00000000.wal"), b"bad").expect("damage WAL");
    let outcome = retain(&root, &fresh, 1);
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 1, None),
        "the fresh copy is verified before the scan, so no classification was published"
    );
    assert!(old.exists());
    assert!(sidecar(&fresh).get("scheduled_retention").is_none());
}

#[test]
fn refuses_foreign_fresh_sidecar_and_preserves_original_fields() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let fresh = copy(&root, "20200101_000002", 2, Some("other-owner"));
    let original = sidecar(&fresh);
    let outcome = retain(&root, &fresh, 1);
    assert_eq!((outcome.deleted, outcome.failed), (0, 1));
    assert!(old.exists());
    assert_eq!(sidecar(&fresh), original);
}

#[test]
fn incomplete_root_scan_has_no_gauge_and_deletes_nothing() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let fresh = copy(&root, "20200101_000002", 2, None);
    for index in 0..ROOT_ENTRIES - 1 {
        fs::write(root.join(format!("foreign-{index}")), []).expect("foreign entry");
    }
    let outcome = retain(&root, &fresh, 1);
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 1, None)
    );
    assert!(old.exists());
}

#[test]
fn over_budget_manifest_aborts_the_pass_before_pruning() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let large = copy(&root, "20200101_000002", 2, Some(OWNER));
    let fresh = copy(&root, "20200101_000003", 3, None);
    fs::OpenOptions::new()
        .write(true)
        .open(large.join(MANIFEST_FILE_NAME))
        .expect("open manifest")
        .set_len((16 << 20) + 1)
        .expect("oversize manifest");
    let outcome = retain(&root, &fresh, 1);
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 1, None)
    );
    assert!(old.exists());
}

#[test]
fn pass_budget_does_not_charge_files_the_verifier_never_reads() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let unknown = copy(&root, "20200101_000002", 2, Some(OWNER));
    let fresh = copy(&root, "20200101_000003", 3, None);
    fs::write(unknown.join("unknown"), vec![0u8; 8193]).expect("unknown payload");
    let cancel = CancelFlag::default();
    let mut budget = Budget::with_bytes(&cancel, 8192);
    let mut outcome = RetentionOutcome::default();
    let refusal = prune_inner(
        &root,
        &fresh,
        OWNER,
        &BackupRetentionConfig { keep_last: 1 },
        &BTreeSet::new(),
        &mut budget,
        &mut outcome,
    );
    assert!(
        refusal.is_ok(),
        "an unread unknown file costs only its directory entry"
    );
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (1, 0, Some(1)),
        "the old copy is pruned and the unmanifested candidate stays untouched"
    );
    assert!(!old.exists());
    assert!(unknown.join("unknown").exists());
    assert_eq!(
        sidecar(&fresh).get("scheduled_retention"),
        Some(&owner_tag(OWNER))
    );
}

#[test]
fn cancellation_before_scan_does_not_tag_or_delete() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let fresh = copy(&root, "20200101_000002", 2, None);
    let cancel = CancelFlag::default();
    cancel.cancel();
    let outcome = prune(
        &root,
        &fresh,
        OWNER,
        &BackupRetentionConfig { keep_last: 1 },
        &cancel,
        &BTreeSet::new(),
    );
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 0, None)
    );
    assert!(old.exists());
    assert!(sidecar(&fresh).get("scheduled_retention").is_none());
}

#[cfg(unix)]
#[test]
fn rejects_linked_trees_control_files_and_sockets() {
    // AF_UNIX addresses have a small platform limit; TMPDIR on CI need not.
    let temp = tempfile::tempdir_in("/tmp").expect("short Unix tempdir");
    let root = private_root_in(temp.path());
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).expect("outside directory");
    let link = copy(&root, "20200101_000001", 1, Some(OWNER));
    symlink(&outside, link.join("out")).expect("symlink");
    let socket = copy(&root, "20200101_000002", 2, Some(OWNER));
    // The listening handle stays alive during the scan.
    let socket_path = socket.join("s");
    let listener = UnixListener::bind(&socket_path).expect("Unix socket");
    let control = root.join("20200101_000003");
    fs::create_dir(&control).expect("candidate directory");
    fs::write(
        control.join(BACKUP_META_FILE_NAME),
        fs::read(link.join(BACKUP_META_FILE_NAME)).expect("source tag"),
    )
    .expect("candidate tag");
    symlink(
        link.join(MANIFEST_FILE_NAME),
        control.join(MANIFEST_FILE_NAME),
    )
    .expect("manifest symlink");
    let fresh = copy(&root, "20200101_000004", 4, None);

    let outcome = retain(&root, &fresh, 1);
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 0, Some(3))
    );
    assert!(fs::symlink_metadata(link.join("out"))
        .expect("link remains")
        .is_symlink());
    assert!(fs::symlink_metadata(&socket_path).is_ok());
    assert!(fs::symlink_metadata(control.join(MANIFEST_FILE_NAME))
        .expect("manifest remains")
        .is_symlink());
    assert!(outside.exists());
    drop(listener);
}

#[test]
fn broken_marker_fails_the_pass_and_records_provenance_for_repair() {
    let (_temp, root) = private_root();
    let tracked = copy(&root, "20200101_000001", 1, None);
    let foreign = copy(&root, "20200101_000002", 2, None);
    let fresh = copy(&root, "20200101_000003", 3, None);
    let record = (tracked_name(&tracked), 1, manifest_digest(&tracked));
    // A DIRECTORY at the marker path: provisioning fails for the pass.
    let obstruction = root.join(SNAPSHOT_LEASE_FILE_NAME);
    fs::create_dir(&obstruction).expect("obstruction directory");

    let outcome = retain_pending(&root, &fresh, 1, std::slice::from_ref(&record));
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.skipped_in_use),
        (0, 1, 0),
        "a marker error is a retention failure and deletes nothing"
    );
    assert!(outcome.adopted.is_empty(), "nothing is adopted");
    assert_eq!(outcome.untouched, None, "the scan never completed");
    assert!(tracked.exists(), "the tracked output stays untagged");
    assert!(sidecar(&tracked).get("scheduled_retention").is_none());
    assert!(sidecar(&fresh).get("scheduled_retention").is_none());
    assert!(foreign.exists());

    // Repair: after the obstruction is removed the next pass adopts the
    // exact tracked output (tags it, deletes it under the unchanged
    // keep_last selection) and never touches the foreign untagged copy.
    fs::remove_dir_all(&obstruction).expect("remove obstruction");
    let repaired = retain_pending(&root, &fresh, 1, std::slice::from_ref(&record));
    assert_eq!(
        (repaired.deleted, repaired.failed, repaired.untouched),
        (1, 0, Some(1)),
        "exactly the tracked output is adopted and pruned; the foreign copy is untouched"
    );
    assert_eq!(
        repaired.adopted,
        [record],
        "the exact recorded (name, creation time, digest) triple is reported adopted"
    );
    assert!(!tracked.exists());
    assert!(
        foreign.exists(),
        "an unrecorded untagged copy is never adopted"
    );
    assert!(fresh.is_dir());
    assert_eq!(
        sidecar(&fresh)
            .get("scheduled_retention")
            .expect("fresh copy tagged"),
        &owner_tag(OWNER)
    );
}

#[test]
fn stale_provenance_with_swapped_foreign_tag_is_never_adopted() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let tracked = copy(&root, "20200101_000002", 2, None);
    // Another writer tagged the recorded copy between the record and the
    // repaired pass: ownership is decided on a FRESH sidecar read, so a
    // foreign tag refuses adoption and the copy stays untouched.
    let mut swapped = sidecar(&tracked);
    swapped["scheduled_retention"] = owner_tag("someone-else");
    fs::write(
        tracked.join(BACKUP_META_FILE_NAME),
        serde_json::to_vec(&swapped).expect("encode sidecar"),
    )
    .expect("write swapped tag");
    // A DIFFERENT output under a recorded name: recorded time 9, actual
    // manifest creation time 4 — never adoptable, and being untagged not
    // owned either, so untouched.
    let mismatch = copy(&root, "20200101_000004", 4, None);
    let fresh = copy(&root, "20200101_000003", 3, None);

    // keep_last=2 so the owned `old` copy is inside the floor and the
    // oracle isolates the provenance outcomes (deleted = 0).
    let outcome = retain_pending(
        &root,
        &fresh,
        2,
        &[
            (tracked_name(&tracked), 2, manifest_digest(&tracked)),
            (tracked_name(&mismatch), 9, manifest_digest(&mismatch)),
        ],
    );
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (0, 0, Some(2)),
        "foreign-tagged and time-mismatched copies untouched; nothing deleted"
    );
    assert!(outcome.adopted.is_empty());
    assert!(tracked.exists());
    assert_eq!(
        sidecar(&tracked).get("scheduled_retention"),
        Some(&owner_tag("someone-else")),
        "the foreign tag is preserved"
    );
    assert!(mismatch.exists());
    assert!(old.exists());
}

#[test]
fn already_tagged_provenance_match_reports_adopted_without_retagging() {
    let (_temp, root) = private_root();
    let tracked = copy(&root, "20200101_000001", 1, None);
    // Tag it with OUR owner out of band (an earlier repaired pass did it
    // before failing later): the next pass reports it adopted, keeps the
    // tag byte-identical, and treats it as a normal candidate.
    let mut tagged = sidecar(&tracked);
    tagged["scheduled_retention"] = owner_tag(OWNER);
    fs::write(
        tracked.join(BACKUP_META_FILE_NAME),
        serde_json::to_vec(&tagged).expect("encode sidecar"),
    )
    .expect("write our tag");
    let original = fs::read(tracked.join(BACKUP_META_FILE_NAME)).expect("read tag");
    let fresh = copy(&root, "20200101_000002", 2, None);

    let outcome = retain_pending(
        &root,
        &fresh,
        2,
        &[(tracked_name(&tracked), 1, manifest_digest(&tracked))],
    );
    assert_eq!((outcome.deleted, outcome.failed), (0, 0));
    assert_eq!(
        outcome.adopted,
        [(tracked_name(&tracked), 1, manifest_digest(&tracked))],
        "the exact recorded (name, creation time, digest) triple is reported adopted"
    );
    assert_eq!(
        fs::read(tracked.join(BACKUP_META_FILE_NAME)).expect("tag unchanged"),
        original
    );
    assert!(tracked.exists());
}

/// A tag tempfile under `dir` would appear unmanifested to the paused reader.
/// The real staged file must live in the root under the reserved shape, while
/// the reader's shared lease is held; it is released before asserting shape.
#[test]
fn tagging_under_held_read_lease_completes_without_visible_staged_file() {
    let (_temp, root) = private_root();
    let fresh = copy(&root, "20200101_000002", 2, None);
    let cancel = CancelFlag::default();

    // Scheduler semantics: the marker exists before any reader.
    crate::sync::durability::backup::provision_snapshot_lease(&root).expect("provision the marker");

    let (loaded_tx, loaded_rx) = mpsc::sync_channel(1);
    let (resume_tx, resume_rx) = mpsc::sync_channel(1);
    let (verified_tx, verified_rx) = mpsc::sync_channel::<bool>(1);

    let reader_snapshot = fresh.clone();
    thread::scope(|scope| {
        let reader = scope.spawn(move || {
            let subscriber = tracing_subscriber::registry().with(PauseAfterManifest {
                loaded: loaded_tx,
                resume: Mutex::new(resume_rx),
            });
            let report =
                tracing::subscriber::with_default(subscriber, || verify_manifest(&reader_snapshot));
            verified_tx
                .send(report.is_ok())
                .expect("report the verifier verdict");
        });

        // The verifier has loaded the manifest: its shared lease is held
        // for the whole walk still to come.
        loaded_rx
            .recv_timeout(VERIFY_TIMEOUT)
            .expect("verifier loaded the manifest (lease held)");

        let sidecar_map = sidecar(&fresh).as_object().expect("sidecar object").clone();
        let mut staged_suffix_valid = false;
        let mut on_staged = |staged: &Path| {
            assert!(
                staged.is_file(),
                "the staged sidecar file exists before the persist"
            );
            assert_eq!(
                staged.parent(),
                Some(root.as_path()),
                "staged in the ROOT, never inside the snapshot"
            );
            let staged_name = staged
                .file_name()
                .expect("staged name")
                .to_string_lossy()
                .into_owned();
            staged_suffix_valid = staged_name.len() == STAGED_PREFIX.len() + 16
                && staged_name
                    .strip_prefix(STAGED_PREFIX)
                    .is_some_and(|suffix| suffix.bytes().all(|byte| byte.is_ascii_alphanumeric()));
            assert!(
                !fresh
                    .join(staged.file_name().expect("staged name"))
                    .exists(),
                "no staged file inside the leased snapshot"
            );
            match try_snapshot_delete_lease(&root) {
                Ok(None) => {}
                other => {
                    panic!("the reader's shared lease must be held across the staging: {other:?}")
                }
            }
            resume_tx.send(()).expect("release the verifier");
            let verified = verified_rx
                .recv_timeout(VERIFY_TIMEOUT)
                .expect("verifier verdict while the staged file exists");
            assert!(
                verified,
                "verifier completes without seeing the staged file"
            );
        };
        tag_fresh_with_seam(
            &root,
            &fresh,
            sidecar_map,
            OWNER,
            &Budget::new(&cancel),
            &mut on_staged,
        )
        .expect("tagging succeeds under the held read lease");
        assert!(
            staged_suffix_valid,
            "staged suffix is exactly 16 ASCII alphanumeric characters"
        );

        reader.join().expect("verifier thread");
    });
}

/// Regression (reviewer P2, item 2 of the round-3 brief): a replacement
/// tree under the SAME name with the SAME recorded creation time but
/// DIFFERENT manifest bytes (a valid v2 online payload) must never be
/// adopted. Before the recorded manifest digest existed, adoption matched
/// only (pathname, creation time): the replacement acquired the
/// scheduler's tag and was deleted (with keep_last=1 it sits behind the
/// fresh winner, so the old behavior DELETES it). Without the digest
/// binding this test reports the triple adopted, tags the replacement and
/// deletes it.
#[test]
fn replaced_output_same_name_and_time_different_bytes_is_never_adopted() {
    let (_temp, root) = private_root();
    // The decoy supplies the recorded provenance bytes; the tree now
    // occupying the recorded name is an ACTUAL VALID REPLACEMENT: the
    // same decoded creation time with distinct (equally valid) manifest
    // bytes — trailing whitespace the v2 JSON decoder ignores.
    let decoy = copy(&root, "20200102_000001", 1, None);
    let tracked = copy(&root, "20200101_000001", 1, None);
    let fresh = copy(&root, "20200101_000002", 2, None);
    fs::OpenOptions::new()
        .append(true)
        .open(tracked.join(MANIFEST_FILE_NAME))
        .expect("open manifest")
        .write_all(b"\n")
        .expect("append whitespace to the manifest");
    let decoy_manifest = fs::read(decoy.join(MANIFEST_FILE_NAME)).expect("read decoy manifest");
    let tracked_manifest = fs::read(tracked.join(MANIFEST_FILE_NAME)).expect("read replacement");
    assert_ne!(
        tracked_manifest, decoy_manifest,
        "the replacement's raw manifest bytes differ from the recorded ones"
    );
    assert_ne!(
        manifest_digest(&tracked),
        manifest_digest(&decoy),
        "the digests the registry would record differ"
    );
    let created_at = |bytes: &[u8]| {
        manifest::decode(std::str::from_utf8(bytes).expect("utf8 manifest"))
            .expect("decode manifest")
            .created_at_unix_ns
    };
    assert_eq!(
        created_at(&tracked_manifest),
        created_at(&decoy_manifest),
        "same decoded manifest creation time"
    );
    verify_manifest(&tracked).expect("the replacement fixture is a valid snapshot");
    let record = (tracked_name(&tracked), 1, manifest_digest(&decoy));
    // keep_last=1: if the replacement were (wrongly) adopted it would be
    // the oldest owned copy and deleted by the unchanged selection.
    let outcome = retain_pending(&root, &fresh, 1, &[record]);
    assert_eq!(
        (outcome.deleted, outcome.failed),
        (0, 0),
        "the replacement is never adopted, so nothing is deleted"
    );
    assert!(
        outcome.adopted.is_empty(),
        "a different output is never adopted, not even with the same name and time"
    );
    assert_eq!(
        outcome.untouched,
        Some(2),
        "both unrecorded untagged copies"
    );
    assert!(tracked.exists());
    assert!(decoy.exists());
    assert!(
        sidecar(&tracked).get("scheduled_retention").is_none(),
        "the replacement is never tagged with the owner"
    );
    assert_eq!(
        sidecar(&fresh).get("scheduled_retention"),
        Some(&owner_tag(OWNER))
    );
}

#[test]
fn manifest_replaced_after_scan_is_not_adopted() {
    let (_temp, root) = private_root();
    let tracked = copy(&root, "20200101_000001", 1, None);
    let fresh = copy(&root, "20200101_000002", 2, None);
    let manifest = tracked.join(MANIFEST_FILE_NAME);
    let original = fs::read(&manifest).expect("read scanned manifest");
    let record = (tracked_name(&tracked), 1, manifest_digest(&tracked));
    let changed = Arc::new(AtomicBool::new(false));
    let subscriber = tracing_subscriber::registry().with(MutateManifestBeforeAdoption {
        manifest: manifest.clone(),
        changed: Arc::clone(&changed),
    });

    let outcome = tracing::subscriber::with_default(subscriber, || {
        retain_pending(&root, &fresh, 1, std::slice::from_ref(&record))
    });

    let replacement = fs::read(&manifest).expect("read replacement manifest");
    assert!(changed.load(Ordering::SeqCst), "the after-scan seam fired");
    assert_ne!(
        replacement, original,
        "the manifest bytes changed after scan"
    );
    assert_ne!(
        manifest_digest(&tracked),
        record.2,
        "the digest changed after scan"
    );
    assert_eq!((outcome.deleted, outcome.failed), (0, 0));
    assert!(outcome.adopted.is_empty());
    assert!(tracked.exists(), "the original output was not deleted");
    assert!(
        sidecar(&tracked).get("scheduled_retention").is_none(),
        "the changed output remains untagged"
    );
    assert_eq!(
        sidecar(&fresh).get("scheduled_retention"),
        Some(&owner_tag(OWNER))
    );
}

/// Regression (reviewer P2, item 3): a crash between staging and the
/// atomic sidecar replace leaves a plain root entry with the recognizable
/// staged prefix; the next pass removes it BEFORE charging entries, so the
/// root cannot be wedged just over [`ROOT_ENTRIES`]. Without the cleanup
/// this pass sees ROOT_ENTRIES + 1 entries and refuses with a budget
/// error, failing the pass and leaving the debris (and the old copy)
/// behind.
#[test]
fn leftover_staged_file_is_removed_and_not_counted_against_the_cap() {
    let (_temp, root) = private_root();
    let old = copy(&root, "20200101_000001", 1, Some(OWNER));
    let fresh = copy(&root, "20200101_000002", 2, None);
    // Exactly ROOT_ENTRIES counted entries remain after cleanup: the
    // preserved matching-name live file still occupies one of the cap slots.
    for index in 0..ROOT_ENTRIES - 3 {
        fs::write(root.join(format!("foreign-{index}")), []).expect("foreign entry");
    }
    let preserved = root.join(format!("{STAGED_PREFIX}fedcba9876543210"));
    fs::write(&preserved, b"live staged sidecar").expect("preserved live entry");
    fs::OpenOptions::new()
        .write(true)
        .open(&preserved)
        .expect("open preserved entry for timestamp update")
        .set_modified(
            std::time::SystemTime::now()
                .checked_sub(std::time::Duration::from_secs(60))
                .expect("fresh timestamp is representable"),
        )
        .expect("set fresh mtime");
    let debris = root.join(format!("{STAGED_PREFIX}0123456789abcdef"));
    fs::write(&debris, b"partial sidecar").expect("stage debris");
    fs::OpenOptions::new()
        .write(true)
        .open(&debris)
        .expect("open debris for timestamp update")
        .set_modified(
            std::time::SystemTime::now()
                .checked_sub(std::time::Duration::from_secs(15 * 60 + 1))
                .expect("old timestamp is representable"),
        )
        .expect("set old debris mtime");
    let outcome = retain(&root, &fresh, 1);
    assert_eq!(
        (outcome.deleted, outcome.failed, outcome.untouched),
        (
            1,
            0,
            Some(u64::try_from(ROOT_ENTRIES - 2).expect("count fits u64"))
        ),
        "old debris is excluded while the preserved live file counts"
    );
    assert!(
        !debris.exists(),
        "old exact-shape debris removed before the scan"
    );
    assert!(preserved.is_file(), "fresh matching entry stays counted");
    assert!(!old.exists());
    assert!(fresh.is_dir());
}

/// The cleanup removes only an old plain regular file with the exact
/// reserved name shape. Entries with that shape that are directories or
/// symlinks, a fresh matching file, a future-mtime matching file, a
/// prefix-only foreign name, or a malformed suffix remain counted.
#[test]
fn staged_prefix_directories_and_links_are_never_removed() {
    let (_temp, root) = private_root();
    let fresh = copy(&root, "20200101_000001", 1, None);
    let dir_debris = root.join(format!("{STAGED_PREFIX}aaaaaaaaaaaaaaaa"));
    fs::create_dir(&dir_debris).expect("debris directory");
    let fresh_debris = root.join(format!("{STAGED_PREFIX}0123456789abcdef"));
    fs::write(&fresh_debris, b"live staged sidecar").expect("fresh staged file");
    fs::OpenOptions::new()
        .write(true)
        .open(&fresh_debris)
        .expect("open fresh staged file for timestamp update")
        .set_modified(std::time::SystemTime::now())
        .expect("set fresh staged mtime");
    let prefix_only = root.join(format!("{STAGED_PREFIX}notes"));
    fs::write(&prefix_only, b"operator notes").expect("prefix-only foreign file");
    fs::OpenOptions::new()
        .write(true)
        .open(&prefix_only)
        .expect("open prefix-only file for timestamp update")
        .set_modified(
            std::time::SystemTime::now()
                .checked_sub(std::time::Duration::from_secs(60 * 60))
                .expect("old timestamp is representable"),
        )
        .expect("set old foreign mtime");
    let future_debris = root.join(format!("{STAGED_PREFIX}bbbbbbbbbbbbbbbb"));
    fs::write(&future_debris, b"future timestamp").expect("future staged file");
    fs::OpenOptions::new()
        .write(true)
        .open(&future_debris)
        .expect("open future staged file for timestamp update")
        .set_modified(
            std::time::SystemTime::now()
                .checked_add(std::time::Duration::from_secs(60 * 60))
                .expect("future timestamp is representable"),
        )
        .expect("set future staged mtime");
    let malformed = root.join(format!("{STAGED_PREFIX}0123456789abcde!"));
    fs::write(&malformed, b"malformed suffix").expect("malformed staged file");
    fs::OpenOptions::new()
        .write(true)
        .open(&malformed)
        .expect("open malformed suffix for timestamp update")
        .set_modified(
            std::time::SystemTime::now()
                .checked_sub(std::time::Duration::from_secs(60 * 60))
                .expect("old timestamp is representable"),
        )
        .expect("set old malformed mtime");
    let malformed_short = root.join(format!("{STAGED_PREFIX}0123456789abcde"));
    fs::write(&malformed_short, b"short suffix").expect("short staged file");
    fs::OpenOptions::new()
        .write(true)
        .open(&malformed_short)
        .expect("open short suffix for timestamp update")
        .set_modified(
            std::time::SystemTime::now()
                .checked_sub(std::time::Duration::from_secs(60 * 60))
                .expect("old timestamp is representable"),
        )
        .expect("set old short-name mtime");
    #[cfg(unix)]
    let link_debris = root.join(format!("{STAGED_PREFIX}cccccccccccccccc"));
    #[cfg(unix)]
    symlink(&fresh, &link_debris).expect("debris symlink");
    let outcome = retain(&root, &fresh, 1);
    assert_eq!(
        (outcome.deleted, outcome.failed),
        (0, 0),
        "neither debris entry blocks a pass below the cap"
    );
    let extra = if cfg!(unix) { 7 } else { 6 };
    assert_eq!(outcome.untouched, Some(extra));
    for path in [
        dir_debris,
        fresh_debris,
        prefix_only,
        future_debris,
        malformed,
        malformed_short,
    ] {
        assert!(path.exists(), "preserved {path:?}");
    }
    #[cfg(unix)]
    assert!(
        fs::symlink_metadata(&link_debris)
            .expect("link remains")
            .is_symlink(),
        "an exact-shape symlink is never removed or followed"
    );
}
