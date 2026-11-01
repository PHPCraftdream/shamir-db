use crate::batch::IntoBatchOp;
use shamir_query_types::admin::ListOp;
use shamir_query_types::batch::BatchOp;

/// Builder for the `list functions` variant of [`ListOp`].
pub struct ListFunctions {
    folder: Option<String>,
}
/// List all registered functions (catalogue-wide). Optionally filter by folder.
pub fn list_functions() -> ListFunctions {
    ListFunctions { folder: None }
}
impl ListFunctions {
    /// Filter by folder prefix.
    pub fn folder(mut self, folder: impl Into<String>) -> Self {
        self.folder = Some(folder.into());
        self
    }
    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::List(ListOp::Functions {
            folder: self.folder,
        })
    }
}
impl From<ListFunctions> for BatchOp {
    fn from(value: ListFunctions) -> Self {
        value.build()
    }
}

impl IntoBatchOp for ListFunctions {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
