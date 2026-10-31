use crate::batch::IntoBatchOp;
use shamir_query_types::admin::ListOp;
use shamir_query_types::batch::BatchOp;

/// Builder for the `list tables` variant of [`ListOp`].
pub struct ListTables {
    repo: String,
}
/// List tables in a repo. `repo` defaults to `"main"`.
pub fn list_tables() -> ListTables {
    ListTables {
        repo: "main".to_owned(),
    }
}
impl ListTables {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }
    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::List(ListOp::Tables { repo: self.repo })
    }
}
impl From<ListTables> for BatchOp {
    fn from(value: ListTables) -> Self {
        value.build()
    }
}
impl IntoBatchOp for ListTables {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
