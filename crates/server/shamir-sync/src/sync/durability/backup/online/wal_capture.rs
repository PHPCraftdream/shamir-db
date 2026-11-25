//! Consistency-point port between the online copy engine and the storage
//! engine (design `online-backup.md` 4.2). Step 1 of the plan implements
//! this port in `shamir-engine`; step 3 connects it. The protocol calls it
//! at steps A, B, E and G.

use std::path::PathBuf;
use std::time::Instant;

use super::manifest::RepoConsistency;
use super::progress::CancelFlag;

/// One file-backed repository in the captured set. `wal_dir` is `None` for
/// an in-memory repo, which is reported in the manifest but not copied
/// (design Q8).
#[derive(Debug, Clone)]
pub struct RepoSource {
    pub name: String,
    pub store_dir: PathBuf,
    pub wal_dir: Option<PathBuf>,
}

/// A repo's flushed position reported by a barrier (design 4.5
/// `RepoConsistency` data).
#[derive(Debug, Clone)]
pub struct CapturedPoint {
    /// Highest fully-captured WAL version for the repo.
    pub wal_position: u64,
    /// Store directory relative to the data dir, forward-slash separated.
    pub store_dir: String,
    /// Per-repo schema version (manifest v2 field).
    pub schema_version: u32,
}

impl CapturedPoint {
    pub fn to_consistency(&self, _repo: &str) -> RepoConsistency {
        RepoConsistency {
            wal_position: self.wal_position,
            store_dir: self.store_dir.clone(),
            schema_version: self.schema_version,
        }
    }
}

/// Errors the port can return while driving a barrier or pin.
#[derive(Debug, thiserror::Error)]
pub enum PortError {
    #[error("WAL truncation pin acquisition failed")]
    PinAcquire,
    #[error("capture barrier failed for repo {repo}")]
    Capture {
        repo: String,
        #[source]
        source: std::io::Error,
    },
    #[error("seal barrier failed for repo {repo}")]
    Seal {
        repo: String,
        #[source]
        source: std::io::Error,
    },
    #[error("system store flush failed")]
    SystemStore(#[from] std::io::Error),
}

/// The port the online backup protocol calls (design 4.2).
pub trait OnlineBackupPort {
    /// The captured repo set, fixed at step A (design Q9: catalogue
    /// snapshot; later catalogue mutations are not picked up).
    fn catalogue(&self) -> Result<Vec<RepoSource>, PortError>;

    /// Step A: acquire the WAL truncation pin on every catalogue repo;
    /// held from step A through step G.
    fn acquire_pins(&self) -> Result<(), PortError>;

    /// Step B (capture barrier): flush `repo` and report its flushed WAL
    /// position `V_r`.
    fn capture_point(&self, repo: &str) -> Result<CapturedPoint, PortError>;

    /// Step B (capture barrier): flush the system store and report its
    /// version.
    fn capture_system_store(&self) -> Result<u64, PortError>;

    /// Step E (seal barrier): re-flush `repo` and report the consistency
    /// point `V*`.
    fn seal_point(&self, repo: &str) -> Result<CapturedPoint, PortError>;

    /// Step E, deadline-aware variant the protocol actually calls:
    /// `deadline` is the repo's seal re-copy budget boundary (the
    /// per-repo monotonic start is taken immediately before this call,
    /// so the barrier itself is INSIDE the budget); `cancel` is checked by the
    /// engine at its safe step boundaries — it never preempts or drops
    /// the mutating seal future. The
    /// default ignores both and delegates to
    /// [`OnlineBackupPort::seal_point`] (existing impls keep working).
    /// The engine barrier is COOPERATIVELY controlled (R2a follow-up 3):
    /// implementations hand the deadline and the flag to the engine's
    /// controlled seal, which observes them only at its own step
    /// boundaries — the mutating seal future is never dropped, wrapped in
    /// `timeout_at`, or raced in a `select` from outside (in-flight
    /// blocking I/O is not preempted; a fired checkpoint fails closed).
    fn seal_point_before(
        &self,
        repo: &str,
        deadline: Option<Instant>,
        cancel: &CancelFlag,
    ) -> Result<CapturedPoint, PortError> {
        let _ = (deadline, cancel);
        self.seal_point(repo)
    }

    /// Called by the protocol right before the first WAL pass of step
    /// C/D (after the capture barrier and its cancellation check) so the
    /// server can publish the real phase `Copying`. Default: no-op.
    fn begin_copy(&self) {}

    /// Called at the start of step F (verify + manifest)
    /// so the server can publish the real phase `Verifying`. Default: no-op.
    fn begin_verify(&self) {}

    /// Validate copied mutable stores before publication. Implementations must
    /// perform any recovery-capable open against a throw-away copy. This
    /// validation is not cancellable mid-flight (its scratch clone is
    /// proportional to `server_meta` plus the audit files); the protocol checks
    /// cancellation after it returns.
    fn validate_copied_data_dir(
        &self,
        _copied_data_dir: &std::path::Path,
    ) -> Result<(), PortError> {
        Ok(())
    }

    /// Ends the seal hold for `repo` after that repo's seal re-copy
    /// (design 1d (a): the hold returned by `seal_point` keeps the stores
    /// frozen until this drops it). Idempotent: ending an unheld or
    /// already-ended seal is a no-op. Called on every failure and
    /// cancellation path — including unwind — so it MUST NOT panic:
    /// `Drop`-based run cleanup invokes it while
    /// a panic is already in flight. For every repo sealed but not yet ended.
    fn end_seal(&self, repo: &str);

    /// Step G and failure cleanup: release every truncation pin. Idempotent.
    /// Called from `Drop`-based cleanup, so it MUST NOT panic.
    fn release_pins(&self);
}
