use crate::batch::IntoBatchOp;
use shamir_query_types::admin::GetTableSchemaOp;
use shamir_query_types::batch::BatchOp;

/// Read a table's declarative schema (introspection).
pub fn get_table_schema(table: impl Into<String>) -> GetTableSchemaBuilder {
    GetTableSchemaBuilder {
        table: table.into(),
        repo: "main".to_owned(),
    }
}

/// Builder for [`GetTableSchemaOp`].
pub struct GetTableSchemaBuilder {
    table: String,
    repo: String,
}

impl GetTableSchemaBuilder {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::GetTableSchema(GetTableSchemaOp {
            get_table_schema: self.table,
            repo: self.repo,
        })
    }
}

impl From<GetTableSchemaBuilder> for BatchOp {
    fn from(b: GetTableSchemaBuilder) -> Self {
        b.build()
    }
}

impl IntoBatchOp for GetTableSchemaBuilder {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
