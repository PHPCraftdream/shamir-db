use shamir_query_types::admin::DescribeRepoOp;
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Build a DESCRIBE REPO request.
pub fn describe_repo(name: impl Into<String>) -> DescribeRepo {
    DescribeRepo {
        name: name.into(),
        repo: None,
    }
}

/// Builder for DESCRIBE REPO.
pub struct DescribeRepo {
    name: String,
    repo: Option<String>,
}

impl DescribeRepo {
    /// Select the database context value returned as `repo`.
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = Some(repo.into());
        self
    }

    /// Build the query operation.
    pub fn build(self) -> BatchOp {
        BatchOp::DescribeRepo(DescribeRepoOp {
            describe_repo: self.name,
            repo: self.repo.unwrap_or_else(|| "main".to_owned()),
        })
    }
}

impl From<DescribeRepo> for BatchOp {
    fn from(builder: DescribeRepo) -> Self {
        builder.build()
    }
}

impl IntoBatchOp for DescribeRepo {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
