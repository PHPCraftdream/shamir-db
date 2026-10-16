use shamir_query_types::admin::{InternerDumpOp, InternerTouchOp};
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Dump a repo's interner dictionary (id → name). Returns a builder for
/// the optional `since` delta cursor.
pub fn interner_dump() -> InternerDump {
    InternerDump {
        repo: "main".to_owned(),
        since: None,
    }
}

/// Builder for [`InternerDumpOp`].
pub struct InternerDump {
    repo: String,
    since: Option<u64>,
}

impl InternerDump {
    /// Override the repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Only return entries with id > `epoch` (delta refresh).
    pub fn since(mut self, epoch: u64) -> Self {
        self.since = Some(epoch);
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::InternerDump(InternerDumpOp {
            interner_dump: self.repo,
            since: self.since,
        })
    }
}

impl From<InternerDump> for BatchOp {
    fn from(b: InternerDump) -> Self {
        b.build()
    }
}

impl IntoBatchOp for InternerDump {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Register field NAMES, returning the (name → id) mapping. Idempotent:
/// a name already present returns its existing id.
pub fn interner_touch(names: impl IntoIterator<Item = impl Into<String>>) -> InternerTouch {
    InternerTouch {
        repo: "main".to_owned(),
        names: names.into_iter().map(Into::into).collect(),
    }
}

/// Builder for [`InternerTouchOp`].
pub struct InternerTouch {
    repo: String,
    names: Vec<String>,
}

impl InternerTouch {
    /// Override the repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::InternerTouch(InternerTouchOp {
            interner_touch: self.repo,
            names: self.names,
        })
    }
}

impl From<InternerTouch> for BatchOp {
    fn from(b: InternerTouch) -> Self {
        b.build()
    }
}

impl IntoBatchOp for InternerTouch {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
