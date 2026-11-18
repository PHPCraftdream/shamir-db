//! Online-backup admin seam — the injected bridge between the engine's
//! admin dispatch (R2a step 3a, design `online-backup.md` §4.7 decision D6)
//! and the embedding server's `OnlineBackupRegistry`.
//!
//! The three node-local `BatchOp` variants (`StartBackup` / `BackupStatus` /
//! `CancelBackup`) are dispatched by `execute_admin` like every other admin
//! op, but the backup machinery lives in `shamir-server` (it needs the data
//! dir, the runtime handle and the in-process `BackupPermit`). This trait is
//! the narrow injected surface that keeps the dependency direction clean —
//! shamir-db MUST NOT depend on shamir-server (same invariant as
//! [`super::ports`]).
//!
//! The port is an `Option<Arc<dyn ...>>` field on [`ShamirDb`], defaulting
//! to `None` (embedded/test deployments answer `backup_unavailable`). The
//! server installs it once at boot, after `ShamirDb` is wrapped in an `Arc`
//! (the registry itself holds a clone of that `Arc`).
//!
//! Results are already-encoded `QueryValue` maps: the engine wraps them in
//! the standard single-record admin result (the `ApplicationSchema` status
//! convention) and never invents a backup-specific DTO.

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;
use shamir_types::types::value::QueryValue;

use crate::{DbResult, ShamirDb};

/// Failure causes of a node-local online-backup admin op. Each variant maps
/// to a stable wire error code via [`OnlineBackupAdminError::code`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnlineBackupAdminError {
    /// One online backup is already running (`start` only, design D9).
    AlreadyRunning,
    /// The operator destination is invalid (relative, inside the data dir).
    InvalidDestination(String),
    /// Destination setup or thread spawn failed on I/O.
    Io(String),
    /// No registry is installed (embedded/test deployment without a server).
    NotConfigured,
}

impl OnlineBackupAdminError {
    /// Stable wire error code carried in `BatchError::QueryError.code`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::AlreadyRunning => "backup_already_running",
            Self::InvalidDestination(_) => "backup_invalid_destination",
            Self::Io(_) => "backup_io_error",
            Self::NotConfigured => "backup_unavailable",
        }
    }
}

impl fmt::Display for OnlineBackupAdminError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => write!(f, "another online backup is already running"),
            Self::InvalidDestination(reason) => {
                write!(f, "invalid backup destination: {reason}")
            }
            Self::Io(reason) => write!(f, "io error in the online backup: {reason}"),
            Self::NotConfigured => {
                write!(f, "online backup is not available on this node")
            }
        }
    }
}

impl std::error::Error for OnlineBackupAdminError {}

/// Injected server-side backup registry seam. Implemented by the embedding
/// layer (`shamir-server` over `OnlineBackupRegistry`); each method returns
/// the fully encoded response record.
#[async_trait]
pub trait OnlineBackupAdminPort: Send + Sync {
    /// Start a backup into the operator destination root; returns the
    /// `{ backup_id, destination }` record.
    async fn start(&self, to: String) -> Result<QueryValue, OnlineBackupAdminError>;
    /// Snapshot of the running (or last) backup as a response record.
    async fn status(&self) -> Result<QueryValue, OnlineBackupAdminError>;
    /// Cooperative cancel; returns the `{ cancelled }` record.
    async fn cancel(&self) -> Result<QueryValue, OnlineBackupAdminError>;
}

impl ShamirDb {
    /// Install the online-backup registry before accepting admin work
    /// (server boot only; written once, read per admin op — the same
    /// startup-only discipline as `set_replication_resume_probe`).
    pub fn set_online_backup_admin_port(&self, port: Arc<dyn OnlineBackupAdminPort>) {
        *self
            .online_backup_port
            .write()
            .expect("online backup port lock poisoned") = Some(port);
    }

    /// Clone out the installed port, if any. No guard is held across an
    /// `.await` — the admin handler clones before calling into the port.
    pub(crate) fn online_backup_admin_port(&self) -> Option<Arc<dyn OnlineBackupAdminPort>> {
        self.online_backup_port
            .read()
            .expect("online backup port lock poisoned")
            .clone()
    }

    /// Capture point of the SYSTEM repository (the system store's own
    /// backing [`crate::engine::repo::RepoInstance`], resolved exactly as
    /// `SystemStore::system_repo()` does, under the repo name
    /// `"system"`).
    ///
    /// What is flushed: the SYSTEM repo's history and buffers via its
    /// capture barrier — the same `flush_all_history` semantics as the
    /// per-repo online-backup step B.
    /// What version is read: the highest fully-captured SYSTEM-repo WAL
    /// position, i.e. the catalogue write horizon — monotonic across
    /// catalogue writes.
    /// Under which quiesce: the bounded writer-drain quiesce inside
    /// [`crate::engine::repo::RepoInstance::capture_point`] (admission
    /// gate closed, leases to zero, settled drain; bounded inside the
    /// call, gate reopens on every outcome).
    ///
    /// This is the source of the backup manifest's
    /// `consistency.system_store_version` per design
    /// `online-backup.md` Q4 / protocol step B.
    pub async fn capture_system_store_point(&self) -> DbResult<u64> {
        let repo = self.system_store().system_repo()?;
        repo.capture_point()
            .await
            .map_err(|e| crate::DbError::Internal(e.to_string()))
    }
}
