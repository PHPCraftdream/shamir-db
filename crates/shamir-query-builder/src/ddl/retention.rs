use shamir_query_types::admin::{
    ChangesSinceOp, PurgeHistoryOp, PurgeScope, Retention, SetRetentionOp,
};
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Change a live table's history-retention policy. `repo` defaults to `"main"`.
pub fn set_retention(table: impl Into<String>, retention: Retention) -> SetRetention {
    SetRetention {
        table: table.into(),
        repo: "main".to_owned(),
        retention,
    }
}

/// Builder for [`SetRetentionOp`].
pub struct SetRetention {
    table: String,
    repo: String,
    retention: Retention,
}

impl SetRetention {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::SetRetention(SetRetentionOp {
            set_retention: self.table,
            repo: self.repo,
            retention: self.retention,
        })
    }
}

impl From<SetRetention> for BatchOp {
    fn from(b: SetRetention) -> Self {
        b.build()
    }
}

impl IntoBatchOp for SetRetention {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Imperative history purge. `repo` defaults to `"main"`.
///
/// ```ignore
/// // purge versions older than 86_400 seconds (1 day)
/// ddl::purge_history("users", PurgeScope::OlderThanAge { age_secs: 86_400 })
/// ```
pub fn purge_history(table: impl Into<String>, scope: PurgeScope) -> PurgeHistory {
    PurgeHistory {
        table: table.into(),
        repo: "main".to_owned(),
        scope,
    }
}

/// Builder for [`PurgeHistoryOp`].
pub struct PurgeHistory {
    table: String,
    repo: String,
    scope: PurgeScope,
}

impl PurgeHistory {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::PurgeHistory(PurgeHistoryOp {
            purge_history: self.table,
            repo: self.repo,
            scope: self.scope,
        })
    }
}

impl From<PurgeHistory> for BatchOp {
    fn from(b: PurgeHistory) -> Self {
        b.build()
    }
}

impl IntoBatchOp for PurgeHistory {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// One-shot "changes since version V" journal read (temporal T4-changes-since).
/// `repo` defaults to `"main"`.
///
/// Returns the durable-journal events with `commit_version > from`, plus the
/// CF-1 `gap_at` re-sync marker. Read-only — the queryable foundation of #201.
///
/// ```ignore
/// // fetch everything after the client's cursor v=42
/// ddl::changes_since(42)
/// // override the repo and cap the result
/// ddl::changes_since(42).repo("archive").limit(500)
/// ```
pub fn changes_since(from: u64) -> ChangesSince {
    ChangesSince {
        from,
        repo: "main".to_owned(),
        limit: None,
    }
}

/// Builder for [`ChangesSinceOp`].
pub struct ChangesSince {
    from: u64,
    repo: String,
    limit: Option<u64>,
}

impl ChangesSince {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Cap the number of returned events (default 1000 at execute time).
    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::ChangesSince(ChangesSinceOp {
            changes_since: self.from,
            repo: self.repo,
            limit: self.limit,
        })
    }
}

impl From<ChangesSince> for BatchOp {
    fn from(b: ChangesSince) -> Self {
        b.build()
    }
}

impl IntoBatchOp for ChangesSince {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
