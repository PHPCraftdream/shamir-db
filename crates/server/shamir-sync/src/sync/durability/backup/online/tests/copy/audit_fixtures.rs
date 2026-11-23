//! Shared audit fixtures of the online-copy tests: the two trusted anchors a
//! copy must carry (`audit_checkpoint` and `server_meta`), the rotation
//! archive name, the plain snapshot-tree copy the restore-side assertions use,
//! and the bounded join of a fixture's helper thread with its own tests.
//!
//! The `server_meta` anchor is a REAL Fjall store in the production layout
//! (`<data_dir>/server_meta`, keyspace `server_meta_v1`, row
//! `audit_checkpoint` encoded with `rmp_serde::to_vec_named` and a
//! `serde_bytes` digest — the shape `shamir-server`'s `ServerMetaStore`
//! writes). Only the Rust type is local: the server's row type is private to
//! that crate. A fixture whose writes all land OUTSIDE the copy window closes
//! the store before the copy and reopens it after the restore, so a phase-1
//! mutant that skips `server_meta/` fails on the missing store, not on a
//! missing text file; a fixture that has to write WHILE the copy runs uses
//! [`LiveServerMeta`] and keeps one handle open, exactly as the server does.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fjall::{Database, KeyspaceCreateOptions};

use super::super::failure_injection_tests::write_file;
use crate::sync::durability::backup::online::copy_engine::test_seams;
use crate::sync::durability::backup::online::manifest;

/// Directory of the SERVER metadata store inside `data_dir` — the second
/// trusted audit anchor (`server_meta`), persisted by the server's meta hook
/// after the appender's own `audit_checkpoint` anchor.
pub(crate) const SERVER_META_DIR: &str = "server_meta";

/// Keyspace of that store (production `ServerMetaStore`).
const SERVER_META_KEYSPACE: &str = "server_meta_v1";

/// Key of the audit anchor row inside [`SERVER_META_KEYSPACE`].
const SERVER_META_ANCHOR_KEY: &str = "audit_checkpoint";

/// The production row shape of the `server_meta` audit anchor
/// (`PersistedAuditCheckpoint`): named fields, digest as bytes.
#[derive(serde::Serialize, serde::Deserialize)]
struct ServerMetaAnchorRow {
    last_audit_seq: u64,
    #[serde(with = "serde_bytes")]
    last_audit_hmac: Vec<u8>,
}

/// A per-fixture audit chain key: every fixture picks its own seed, so a
/// mutant that ignores the key cannot pass them all.
pub(crate) fn audit_key(seed: u8) -> [u8; 32] {
    let mut key = [0u8; 32];
    for (index, byte) in key.iter_mut().enumerate() {
        *byte = seed.wrapping_add(index as u8).wrapping_mul(3);
    }
    key
}

/// The exact rotation archive name of the audit chain: `audit.log.` + 20
/// ASCII decimal digits.
pub(crate) fn archive_name(index: u64) -> String {
    format!("audit.log.{index:020}")
}

/// Persist the `server_meta` anchor the way the server's meta hook does:
/// AFTER the appender's `audit_checkpoint` anchor, with the same
/// `(next_seq, hmac)` pair. The store is created, filled, synced and CLOSED
/// again, so the following copy sees closed files. Fixtures whose write lands
/// OUTSIDE the copy window use this; a write that has to happen WHILE the copy
/// runs must go through [`LiveServerMeta`], because closing and reopening a
/// Fjall store is a transition production never makes then.
pub(crate) fn persist_server_meta_anchor(dir: &Path, position: (u64, [u8; 32])) {
    let path = dir.join(SERVER_META_DIR);
    fs::create_dir_all(&path).expect("mkdirs server_meta");
    let store = Database::builder(&path)
        .open()
        .expect("open the fixture server_meta store");
    let keyspace = store
        .keyspace(SERVER_META_KEYSPACE, KeyspaceCreateOptions::default)
        .expect("server_meta keyspace");
    let row = ServerMetaAnchorRow {
        last_audit_seq: position.0,
        last_audit_hmac: position.1.to_vec(),
    };
    let encoded = rmp_serde::to_vec_named(&row).expect("encode the server_meta anchor");
    keyspace
        .insert(SERVER_META_ANCHOR_KEY.as_bytes(), encoded.as_slice())
        .expect("insert the server_meta anchor");
    store
        .persist(fjall::PersistMode::SyncAll)
        .expect("persist the server_meta store");
    drop(keyspace);
    drop(store);
}

/// The LIVE `server_meta` store of a fixture: opened ONCE and written through
/// its open handle, exactly as the running server writes it
/// (`server/engine/server_meta.rs::store_audit_checkpoint` inserts the anchor
/// row and then persists the database with `SyncAll`).
///
/// The handle is held for as long as the copy runs, because that is what
/// production holds: `server/engine/server/server_launcher/support/durable_stores.rs`
/// keeps one open `ServerMetaStore` for the life of the server and writes the
/// anchor through it at every rotation boundary. Closing and reopening a Fjall
/// store instead RECOVERS it, and recovery deletes superseded LSM version files
/// (`lsm-tree` `TreeInner::cleanup_orphaned_version`), a transition the live
/// hook never causes — a fixture that reopens mid-copy models something
/// production does not do and makes the copy see files appear and vanish for
/// the wrong reason.
#[derive(Clone)]
pub(crate) struct LiveServerMeta {
    store: Database,
    keyspace: fjall::Keyspace,
}

impl LiveServerMeta {
    /// Open (or create) the fixture's `server_meta` store and keep it open.
    pub(crate) fn open(dir: &Path) -> Self {
        let path = dir.join(SERVER_META_DIR);
        fs::create_dir_all(&path).expect("mkdirs server_meta");
        let store = Database::builder(&path)
            .open()
            .expect("open the fixture server_meta store");
        let keyspace = store
            .keyspace(SERVER_META_KEYSPACE, KeyspaceCreateOptions::default)
            .expect("server_meta keyspace");
        Self { store, keyspace }
    }

    /// Write the anchor row through the OPEN handle and make it durable.
    pub(crate) fn write_anchor(&self, position: (u64, [u8; 32])) {
        let row = ServerMetaAnchorRow {
            last_audit_seq: position.0,
            last_audit_hmac: position.1.to_vec(),
        };
        let encoded = rmp_serde::to_vec_named(&row).expect("encode the server_meta anchor");
        self.keyspace
            .insert(SERVER_META_ANCHOR_KEY.as_bytes(), encoded.as_slice())
            .expect("insert the server_meta anchor");
        self.store
            .persist(fjall::PersistMode::SyncAll)
            .expect("persist the server_meta store");
    }
}

/// Read the anchor back through a REOPENED Fjall store, panicking with the
/// phase-1 requirement when the store is absent: the data-directory copy's
/// phase 1 captures every anchor store, so a restored tree without this
/// store means the copy dropped a whole anchor.
pub(crate) fn read_server_meta_anchor(dir: &Path) -> (u64, [u8; 32]) {
    let path = dir.join(SERVER_META_DIR);
    assert!(
        path.join("keyspaces").is_dir(),
        "the copy must carry the server_meta anchor store (phase 1 captures it): {} has no keyspaces",
        path.display()
    );
    let store = Database::builder(&path)
        .open()
        .expect("reopen the restored server_meta store");
    let keyspace = store
        .keyspace(SERVER_META_KEYSPACE, KeyspaceCreateOptions::default)
        .expect("server_meta keyspace");
    let entry = keyspace
        .get(SERVER_META_ANCHOR_KEY.as_bytes())
        .expect("read the server_meta anchor row")
        .expect("the restored server_meta store must carry the audit anchor row");
    let row: ServerMetaAnchorRow =
        rmp_serde::from_slice(&entry).expect("decode the server_meta anchor row");
    let hmac: [u8; 32] = row
        .last_audit_hmac
        .try_into()
        .expect("the server_meta anchor digest is 32 bytes");
    (row.last_audit_seq, hmac)
}

/// Every snapshot-relative path the published manifest lists.
pub(crate) fn manifest_paths(dest_dir: &Path) -> Vec<String> {
    let raw = fs::read_to_string(dest_dir.join("manifest.json")).expect("read the manifest");
    let decoded = manifest::decode(&raw).expect("decode the manifest");
    decoded.files.into_iter().map(|entry| entry.path).collect()
}

/// The published manifest's entry for one snapshot-relative path:
/// `(sha256, size_bytes)`.
pub(crate) fn manifest_entry(dest_dir: &Path, rel: &str) -> (String, u64) {
    let raw = fs::read_to_string(dest_dir.join("manifest.json")).expect("read the manifest");
    let decoded = manifest::decode(&raw).expect("decode the manifest");
    decoded
        .files
        .into_iter()
        .find(|entry| entry.path == rel)
        .map(|entry| (entry.sha256, entry.size_bytes))
        .unwrap_or_else(|| panic!("the manifest must list {rel}"))
}

/// Seed the one file-backed repo the protocol's step C expects: its store
/// file AND a valid WAL segment with its sidecar. `InjectPort::with_repos`
/// always declares `repos/alpha.shamirwal`, and `copy_engine::list_files`
/// fails the WAL pass with `Io { NotFound }` when that directory is absent —
/// so a fixture that drives the FULL protocol and seeds only the store dies in
/// step C, before any audit hook.
pub(crate) fn seed_alpha_repo(source: &Path) {
    write_file(&source.join("repos/alpha/store.bin"), b"store-bytes");
    write_file(&source.join("repos/alpha.shamirwal/00000000.wal"), b"wal-0");
    write_file(
        &source.join("repos/alpha.shamirwal/00000000.meta"),
        b"meta-0",
    );
}

/// Plain file-tree copy of a snapshot into a fresh directory: restoring an
/// online copy moves the snapshot tree into the target `data_dir` (engine
/// stores are replayed separately; the audit files are carried as bytes).
/// NOT the production restore: manifest verification, staging and the swap
/// have their own tests, and the audit tests compensate by asserting each
/// manifest entry against the bytes they open. The snapshot omits every
/// Fjall `lock`, so the copy recreates them where the production restore
/// does, before any staged store is opened.
pub(crate) fn copy_snapshot_tree(from: &Path, to: &Path) {
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
    test_seams::recreate_fjall_locks(to);
}

/// Absolute budget for ONE helper thread, measured from a deadline taken
/// BEFORE the thread is spawned: both rendezvous receives and the teardown join
/// draw on that one deadline, so a wedged helper cannot spend a budget per
/// waiting step and overrun the 180 s per-test kill.
const JOIN_BUDGET: Duration = Duration::from_secs(150);

/// The ONE absolute deadline of a helper thread: call it BEFORE spawning the
/// thread and hand the result to every wait that involves it (the rendezvous
/// receives through [`budget_left`], the join through
/// [`JoinOnDrop::with_deadline`]). Two independently started budgets would add
/// up — 120 s of rendezvous plus 150 s of join overruns the per-test kill.
pub(super) fn helper_deadline() -> Instant {
    Instant::now() + JOIN_BUDGET
}

/// What is left of `deadline`. An expired deadline yields `Duration::ZERO`,
/// which `recv_timeout` reports as its own timeout instead of waiting again —
/// no budget is ever restarted.
pub(super) fn budget_left(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// Recorded outcome of the bounded join, readable AFTER the guard is gone.
pub(super) const OUTCOME_PENDING: u8 = 0;
pub(super) const OUTCOME_JOINED: u8 = 1;
pub(super) const OUTCOME_TIMED_OUT: u8 = 2;
pub(super) const OUTCOME_PANICKED: u8 = 3;

/// Outcome of a bounded join. `Joined` is the only success; `TimedOut` means
/// the helper is still running and was NOT joined (it is left behind on
/// purpose — a thread wedged in uninterruptible I/O is isolated by the test
/// process, never "joined"); `Panicked` carries the helper's payload so only
/// `JoinOnDrop::finish` decides to re-raise it.
enum JoinOutcome {
    Joined,
    TimedOut,
    Panicked(Box<dyn std::any::Any + Send>),
}

/// Joins `handle` within the ABSOLUTE `deadline`: `std::thread` has no timed
/// join, so the join runs on a throwaway thread and the caller waits on a
/// channel with the remaining budget.
fn join_within(handle: std::thread::JoinHandle<()>, deadline: Instant) -> JoinOutcome {
    let (done_tx, done_rx) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("join-within".to_string())
        .spawn(move || {
            let _ = done_tx.send(handle.join());
        });
    if spawned.is_err() {
        return JoinOutcome::TimedOut;
    }
    match done_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(Ok(())) => JoinOutcome::Joined,
        Ok(Err(payload)) => JoinOutcome::Panicked(payload),
        Err(_) => JoinOutcome::TimedOut,
    }
}

/// Joins a helper thread on EVERY path, within ONE absolute deadline shared
/// with the fixture's rendezvous receives. A helper that overruns fails the
/// test in `finish`; a helper panic is re-raised ONLY from `finish`, never
/// from `Drop`, where a second panic during unwinding would abort the whole
/// test binary.
pub(super) struct JoinOnDrop {
    handle: Option<std::thread::JoinHandle<()>>,
    deadline: Instant,
    outcome: Arc<AtomicU8>,
}

impl JoinOnDrop {
    /// Guard whose join draws on a deadline taken BEFORE the thread was
    /// spawned (`helper_deadline`), so the rendezvous and the join share it.
    pub(super) fn with_deadline(handle: std::thread::JoinHandle<()>, deadline: Instant) -> Self {
        Self {
            handle: Some(handle),
            deadline,
            outcome: Arc::new(AtomicU8::new(OUTCOME_PENDING)),
        }
    }

    /// The observed outcome (`OUTCOME_*`), readable after this guard is gone —
    /// the only way a test can assert that a wedged helper was NOT joined.
    pub(super) fn outcome(&self) -> Arc<AtomicU8> {
        self.outcome.clone()
    }

    /// Join eagerly within the remaining budget, re-raise the helper's panic
    /// and fail the test when the helper is still running at the deadline.
    pub(super) fn finish(mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        match join_within(handle, self.deadline) {
            JoinOutcome::Joined => {
                self.outcome.store(OUTCOME_JOINED, Ordering::SeqCst);
            }
            JoinOutcome::Panicked(payload) => {
                self.outcome.store(OUTCOME_PANICKED, Ordering::SeqCst);
                std::panic::resume_unwind(payload);
            }
            JoinOutcome::TimedOut => {
                self.outcome.store(OUTCOME_TIMED_OUT, Ordering::SeqCst);
                panic!(
                    "the helper thread must finish within {JOIN_BUDGET:?} of the shared deadline"
                )
            }
        }
    }
}

impl Drop for JoinOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // Never panics here: this drop can run while unwinding, and a
            // resumed helper panic would abort the binary. An overrunning
            // helper is left behind; `finish` is what reports it.
            match join_within(handle, self.deadline) {
                JoinOutcome::Joined => {
                    self.outcome.store(OUTCOME_JOINED, Ordering::SeqCst);
                }
                JoinOutcome::TimedOut => {
                    self.outcome.store(OUTCOME_TIMED_OUT, Ordering::SeqCst);
                }
                JoinOutcome::Panicked(payload) => {
                    self.outcome.store(OUTCOME_PANICKED, Ordering::SeqCst);
                    // The guard must not run the destructor of an arbitrary
                    // panic payload HERE: this drop can run while the test
                    // already unwinds (a second panic aborts the whole binary)
                    // and, even without an unwind, a payload whose `Drop`
                    // panics would turn a reporting path into a new failure.
                    // Leaking one payload on a path that is about to fail the
                    // test anyway is the cheaper trade.
                    std::mem::forget(payload);
                }
            }
        }
    }
}

/// A helper still running at the shared deadline is reported as NOT joined, and
/// the guard's `Drop` never panics.
///
/// The helper blocks on a channel this test never signals, and the guard gets a
/// deadline that has already passed (`Instant::now()`), so both the join and
/// the drop take the timeout path with no sleep.
///
/// Mutant caught: a guard whose teardown treats an expired budget as a join (or
/// restarts the budget) — the outcome reads "joined" for a helper that is still
/// running.
#[test]
fn a_timed_out_helper_reports_not_joined_and_its_drop_never_panics() {
    let (keep_open, waiting) = mpsc::channel::<()>();
    let handle = std::thread::Builder::new()
        .name("wedged-helper".to_string())
        .spawn(move || {
            let _ = waiting.recv();
        })
        .expect("spawn the wedged helper");
    let guard = JoinOnDrop::with_deadline(handle, Instant::now());
    let outcome = guard.outcome();
    drop(guard);
    assert_eq!(
        outcome.load(Ordering::SeqCst),
        OUTCOME_TIMED_OUT,
        "a helper still running at the deadline must be reported as not joined"
    );
    drop(keep_open);
}

/// A panic payload the guard joins is LEAKED by `Drop`, never dropped there.
///
/// The payload's own `Drop` fails this test if it ever runs, so the assertion
/// is the rule itself: the teardown path never executes an arbitrary payload's
/// destructor (this drop can run while the test already unwinds, where a
/// destructor that panics aborts the whole binary).
#[test]
fn a_joined_panic_payload_is_leaked_by_drop() {
    struct NeverDropped;
    impl Drop for NeverDropped {
        fn drop(&mut self) {
            if !std::thread::panicking() {
                panic!("the guard must not drop a joined panic payload");
            }
        }
    }
    let handle = std::thread::Builder::new()
        .name("panicking-helper".to_string())
        .spawn(|| std::panic::panic_any(NeverDropped))
        .expect("spawn the panicking helper");
    let guard = JoinOnDrop::with_deadline(handle, helper_deadline());
    let outcome = guard.outcome();
    drop(guard);
    assert_eq!(
        outcome.load(Ordering::SeqCst),
        OUTCOME_PANICKED,
        "the guard must record the helper's panic without re-raising it"
    );
}
