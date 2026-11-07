use crate::batch::IntoBatchOp;
use shamir_query_types::admin::ListOp;
use shamir_query_types::batch::BatchOp;

/// Builder for the `list function_folders` variant of [`ListOp`].
pub struct ListFunctionFolders {
    parent: Option<String>,
}
/// List explicitly created function folders. Optionally filter by parent.
pub fn list_function_folders() -> ListFunctionFolders {
    ListFunctionFolders { parent: None }
}
impl ListFunctionFolders {
    /// Filter by parent folder.
    pub fn parent(mut self, parent: impl Into<String>) -> Self {
        self.parent = Some(parent.into());
        self
    }
    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::List(ListOp::FunctionFolders {
            parent: self.parent,
        })
    }
}
impl From<ListFunctionFolders> for BatchOp {
    fn from(value: ListFunctionFolders) -> Self {
        value.build()
    }
}

impl IntoBatchOp for ListFunctionFolders {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
