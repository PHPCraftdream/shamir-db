use crate::batch::IntoBatchOp;
use shamir_query_types::admin::RemoveSchemaRuleOp;
use shamir_query_types::batch::BatchOp;

/// Remove a rule from a table's schema by path.
pub fn remove_schema_rule<I, S>(table: impl Into<String>, path: I) -> RemoveSchemaRuleBuilder
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    RemoveSchemaRuleBuilder {
        table: table.into(),
        repo: "main".to_owned(),
        path: path.into_iter().map(Into::into).collect(),
    }
}

/// Builder for [`RemoveSchemaRuleOp`].
pub struct RemoveSchemaRuleBuilder {
    table: String,
    repo: String,
    path: Vec<String>,
}

impl RemoveSchemaRuleBuilder {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::RemoveSchemaRule(RemoveSchemaRuleOp {
            remove_schema_rule: self.table,
            repo: self.repo,
            path: self.path,
        })
    }
}

impl From<RemoveSchemaRuleBuilder> for BatchOp {
    fn from(b: RemoveSchemaRuleBuilder) -> Self {
        b.build()
    }
}

impl IntoBatchOp for RemoveSchemaRuleBuilder {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
