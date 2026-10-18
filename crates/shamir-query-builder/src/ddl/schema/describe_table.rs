//! Builder for the `DescribeTable` DDL operation.

use shamir_query_types::admin::DescribeTableOp;
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// Describe a table — returns full introspection (schema, indexes,
/// validators, retention, buffer, access meta) in one response.
pub fn describe_table(table: impl Into<String>) -> DescribeTableBuilder {
    DescribeTableBuilder {
        table: table.into(),
        repo: "main".to_owned(),
    }
}

/// Builder for [`DescribeTableOp`].
pub struct DescribeTableBuilder {
    table: String,
    repo: String,
}

impl DescribeTableBuilder {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DescribeTable(DescribeTableOp {
            describe_table: self.table,
            repo: self.repo,
        })
    }
}

impl From<DescribeTableBuilder> for BatchOp {
    fn from(b: DescribeTableBuilder) -> Self {
        b.build()
    }
}

impl IntoBatchOp for DescribeTableBuilder {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
