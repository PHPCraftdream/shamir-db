use shamir_query_types::admin::{
    CommitMigrationOp, MigrationStatusOp, RollbackMigrationOp, StartMigrationOp,
};
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Start an online table migration. Returns a builder for optional fields.
pub fn start_migration(
    table: impl Into<String>,
    dst_repo: impl Into<String>,
    dst_engine: impl Into<String>,
) -> StartMigration {
    StartMigration {
        table: table.into(),
        repo: "main".to_owned(),
        dst_repo: dst_repo.into(),
        dst_engine: dst_engine.into(),
        dst_path: None,
        hmac: None,
    }
}

/// Builder for [`StartMigrationOp`].
pub struct StartMigration {
    table: String,
    repo: String,
    dst_repo: String,
    dst_engine: String,
    dst_path: Option<String>,
    hmac: Option<String>,
}

impl StartMigration {
    /// Override the source repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Set the destination data path.
    pub fn dst_path(mut self, path: impl Into<String>) -> Self {
        self.dst_path = Some(path.into());
        self
    }

    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::StartMigration(StartMigrationOp {
            start_migration: self.table,
            repo: self.repo,
            dst_repo: self.dst_repo,
            dst_engine: self.dst_engine,
            dst_path: self.dst_path,
            hmac: self.hmac,
        })
    }
}

impl From<StartMigration> for BatchOp {
    fn from(b: StartMigration) -> Self {
        b.build()
    }
}

impl IntoBatchOp for StartMigration {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Commit a running migration by ID.
pub fn commit_migration(id: impl Into<String>) -> CommitMig {
    CommitMig {
        id: id.into(),
        hmac: None,
    }
}

/// Builder for [`CommitMigrationOp`].
pub struct CommitMig {
    id: String,
    hmac: Option<String>,
}

impl CommitMig {
    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CommitMigration(CommitMigrationOp {
            commit_migration: self.id,
            hmac: self.hmac,
        })
    }
}

impl From<CommitMig> for BatchOp {
    fn from(b: CommitMig) -> Self {
        b.build()
    }
}

impl IntoBatchOp for CommitMig {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Rollback a running migration by ID.
pub fn rollback_migration(id: impl Into<String>) -> RollbackMig {
    RollbackMig {
        id: id.into(),
        hmac: None,
    }
}

/// Builder for [`RollbackMigrationOp`].
pub struct RollbackMig {
    id: String,
    hmac: Option<String>,
}

impl RollbackMig {
    /// Attach the hex-encoded HMAC tag.
    pub fn hmac(mut self, hmac: impl Into<String>) -> Self {
        self.hmac = Some(hmac.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::RollbackMigration(RollbackMigrationOp {
            rollback_migration: self.id,
            hmac: self.hmac,
        })
    }
}

impl From<RollbackMig> for BatchOp {
    fn from(b: RollbackMig) -> Self {
        b.build()
    }
}

impl IntoBatchOp for RollbackMig {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Query the status of a migration by ID.
pub fn migration_status(id: impl Into<String>) -> BatchOp {
    BatchOp::MigrationStatus(MigrationStatusOp {
        migration_status: id.into(),
    })
}
