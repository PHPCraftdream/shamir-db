//! Progress reporting and cancellation for the online backup engine.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Protocol stage of an online backup run (design `online-backup.md` 4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackupStage {
    Preparing,
    Captured,
    Copying,
    Sealing,
    Verifying,
    Done,
    Failed,
}

/// Hash progress of one repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoProgress {
    pub repo: String,
    pub bytes_hashed: u64,
    pub files_hashed: u64,
}

/// Snapshot of an online backup run's progress, returned by the status
/// operation (server wiring lands in a later step).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupProgress {
    pub stage: BackupStage,
    pub failed_stage: Option<BackupStage>,
    pub started_at_unix_ns: u128,
    pub per_repo: Vec<RepoProgress>,
    pub last_error: Option<String>,
}

impl BackupProgress {
    pub fn new() -> Self {
        let started_at_unix_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        Self {
            stage: BackupStage::Preparing,
            failed_stage: None,
            started_at_unix_ns,
            per_repo: Vec::new(),
            last_error: None,
        }
    }

    pub fn set_stage(&mut self, stage: BackupStage) {
        self.stage = stage;
    }

    /// Upsert one repository's hashed-so-far counters.
    pub fn note_repo_progress(&mut self, repo: &str, bytes_hashed: u64, files_hashed: u64) {
        if let Some(entry) = self.per_repo.iter_mut().find(|p| p.repo == repo) {
            entry.bytes_hashed = bytes_hashed;
            entry.files_hashed = files_hashed;
        } else {
            self.per_repo.push(RepoProgress {
                repo: repo.to_string(),
                bytes_hashed,
                files_hashed,
            });
        }
    }

    pub fn record_failure(&mut self, stage: BackupStage, error: impl fmt::Display) {
        self.stage = BackupStage::Failed;
        self.failed_stage = Some(stage);
        self.last_error = Some(error.to_string());
    }
}

impl Default for BackupProgress {
    fn default() -> Self {
        Self::new()
    }
}

/// Cooperative cancellation flag shared with the copy engine. Always a
/// shutdown token: `from_shutdown` constructs a per-run CHILD token from
/// the server's shutdown token, so an admin cancel cancels only the run
/// while a server shutdown cancels it too; the engine-local default is
/// a fresh, never-cancelled token.
#[derive(Debug, Default, Clone)]
pub struct CancelFlag(tokio_util::sync::CancellationToken);

impl CancelFlag {
    /// A flag cancelled when the given parent token (the server's
    /// shutdown token) is cancelled; the child is stored directly so
    /// `cancelled` can await it.
    pub fn from_shutdown(parent: &tokio_util::sync::CancellationToken) -> Self {
        Self(parent.child_token())
    }

    pub fn cancel(&self) {
        self.0.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }

    /// Await cancellation through the token's wakeup — no polling, no
    /// sleep. The server's production override wraps the engine's async
    /// seal in `timeout_at` + `select`, never the blocking copy.
    pub async fn cancelled(&self) {
        self.0.cancelled().await;
    }
}
