use crate::write::BuilderError;
use shamir_query_types::admin::{AddSchemaRuleOp, FieldRuleDto};
use shamir_query_types::batch::BatchOp;

/// Add (or replace by path) a single rule in a table's schema.
pub fn add_schema_rule(table: impl Into<String>) -> AddSchemaRuleBuilder {
    AddSchemaRuleBuilder {
        table: table.into(),
        repo: "main".to_owned(),
        rule: None,
    }
}

/// Builder for [`AddSchemaRuleOp`].
pub struct AddSchemaRuleBuilder {
    table: String,
    repo: String,
    rule: Option<FieldRuleDto>,
}

impl AddSchemaRuleBuilder {
    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Set the rule to add/replace.
    pub fn rule(mut self, rule: impl Into<FieldRuleDto>) -> Self {
        self.rule = Some(rule.into());
        self
    }

    /// Finalize into a [`BatchOp`].
    ///
    /// Returns [`Err(BuilderError::MissingRule)`](BuilderError::MissingRule)
    /// if [`AddSchemaRuleBuilder::rule`] was never called.
    pub fn build(self) -> Result<BatchOp, BuilderError> {
        Ok(BatchOp::AddSchemaRule(AddSchemaRuleOp {
            add_schema_rule: self.table,
            repo: self.repo,
            rule: self.rule.ok_or(BuilderError::MissingRule)?,
        }))
    }
}
