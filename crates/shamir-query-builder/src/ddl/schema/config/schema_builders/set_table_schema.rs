use crate::batch::IntoBatchOp;
use shamir_query_types::admin::{FieldRuleDto, SetTableSchemaOp};
use shamir_query_types::batch::BatchOp;

/// Whole-replace a table's declarative schema.
pub fn set_table_schema(table: impl Into<String>) -> SetTableSchemaBuilder {
    SetTableSchemaBuilder {
        table: table.into(),
        repo: "main".to_owned(),
        schema: Vec::new(),
        expected_version: None,
    }
}

/// Builder for [`SetTableSchemaOp`].
pub struct SetTableSchemaBuilder {
    table: String,
    repo: String,
    schema: Vec<FieldRuleDto>,
    expected_version: Option<u64>,
}

impl SetTableSchemaBuilder {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Set the schema rules (complete replacement).
    pub fn rules(mut self, rules: impl IntoIterator<Item = FieldRuleDto>) -> Self {
        self.schema = rules.into_iter().collect();
        self
    }

    /// Set the expected schema_version for optimistic concurrency.
    pub fn expected_version(mut self, v: u64) -> Self {
        self.expected_version = Some(v);
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::SetTableSchema(SetTableSchemaOp {
            set_table_schema: self.table,
            repo: self.repo,
            schema: self.schema,
            expected_version: self.expected_version,
        })
    }
}

impl From<SetTableSchemaBuilder> for BatchOp {
    fn from(b: SetTableSchemaBuilder) -> Self {
        b.build()
    }
}

impl IntoBatchOp for SetTableSchemaBuilder {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
