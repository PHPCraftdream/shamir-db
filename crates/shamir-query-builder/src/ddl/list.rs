use shamir_query_types::admin::ListOp;
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

/// List databases.
pub fn list_databases() -> BatchOp {
    BatchOp::List(ListOp::Databases)
}

/// List repos in the current database.
pub fn list_repos() -> BatchOp {
    BatchOp::List(ListOp::Repos)
}

/// List tables in a repo. `repo` defaults to `"main"`.
pub fn list_tables() -> ListTables {
    ListTables {
        repo: "main".to_owned(),
    }
}

/// Builder for the `list tables` variant of [`ListOp`].
pub struct ListTables {
    repo: String,
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
    fn from(b: ListTables) -> Self {
        b.build()
    }
}

impl IntoBatchOp for ListTables {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// List indexes on a table. `repo` defaults to `"main"`.
pub fn list_indexes(table: impl Into<String>) -> ListIndexes {
    ListIndexes {
        table: table.into(),
        repo: "main".to_owned(),
    }
}

/// Builder for the `list indexes` variant of [`ListOp`].
pub struct ListIndexes {
    table: String,
    repo: String,
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
    fn from(b: ListIndexes) -> Self {
        b.build()
    }
}

impl IntoBatchOp for ListIndexes {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// List users.
pub fn list_users() -> BatchOp {
    BatchOp::List(ListOp::Users)
}

/// List roles.
pub fn list_roles() -> BatchOp {
    BatchOp::List(ListOp::Roles)
}

/// List all registered functions (catalogue-wide). Optionally filter by folder.
pub fn list_functions() -> ListFunctions {
    ListFunctions { folder: None }
}

/// Builder for the `list functions` variant of [`ListOp`].
pub struct ListFunctions {
    folder: Option<String>,
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
    fn from(b: ListFunctions) -> Self {
        b.build()
    }
}

impl IntoBatchOp for ListFunctions {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// List all registered validators (catalogue-wide: id + name + bound tables).
///
/// NOTE: This is different from `list_validators(table)` which lists
/// per-table bindings via `ListValidatorsOp`.
pub fn list_all_validators() -> BatchOp {
    BatchOp::List(ListOp::Validators)
}

/// List explicitly created function folders. Optionally filter by parent.
pub fn list_function_folders() -> ListFunctionFolders {
    ListFunctionFolders { parent: None }
}

/// Builder for the `list function_folders` variant of [`ListOp`].
pub struct ListFunctionFolders {
    parent: Option<String>,
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
    fn from(b: ListFunctionFolders) -> Self {
        b.build()
    }
}

impl IntoBatchOp for ListFunctionFolders {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
