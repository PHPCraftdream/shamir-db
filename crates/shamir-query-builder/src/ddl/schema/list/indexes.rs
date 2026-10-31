use crate::batch::IntoBatchOp;
use shamir_query_types::admin::ListOp;
use shamir_query_types::batch::BatchOp;

/// Builder for the `list indexes` variant of [`ListOp`].
pub struct ListIndexes {
    table: String,
    repo: String,
}
/// List indexes on a table. `repo` defaults to `"main"`.
pub fn list_indexes(table: impl Into<String>) -> ListIndexes {
    ListIndexes {
        table: table.into(),
        repo: "main".to_owned(),
    }
}
impl ListIndexes {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }
    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::List(ListOp::Indexes {
            table: self.table,
            repo: self.repo,
        })
    }
}
impl From<ListIndexes> for BatchOp {
    fn from(value: ListIndexes) -> Self {
        value.build()
    }
}
impl IntoBatchOp for ListIndexes {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
