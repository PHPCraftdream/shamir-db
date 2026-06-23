//! Builders for declarative schema DDL operations and the `field()` fluent API.

use shamir_query_types::admin::{
    AddSchemaRuleOp, CompareDto, ConstraintsDto, FieldRuleDto, GetTableSchemaOp, NumDto,
    RemoveSchemaRuleOp, SetTableSchemaOp,
};
use shamir_query_types::batch::BatchOp;

use crate::batch::IntoBatchOp;

// ── field() fluent API ─────────────────────────────────────────────────

/// Start building a [`FieldRuleDto`] for the given path segments.
///
/// ```text
/// field(["email"]).string().max(255).required()
/// field(["address", "zip"]).string().len(5)
/// field(["age"]).int().min(0).max(150)
/// ```
pub fn field<I, S>(path: I) -> FieldBuilder
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    FieldBuilder {
        path: path.into_iter().map(Into::into).collect(),
        ty: String::new(),
        constraints: ConstraintsDto::default(),
    }
}

/// Fluent builder for a single [`FieldRuleDto`].
pub struct FieldBuilder {
    path: Vec<String>,
    ty: String,
    constraints: ConstraintsDto,
}

impl FieldBuilder {
    // ── type setters ───────────────────────────────────────────────

    /// Set the type tag to `"string"`.
    pub fn string(mut self) -> Self {
        self.ty = "string".to_owned();
        self
    }

    /// Set the type tag to `"int"`.
    pub fn int(mut self) -> Self {
        self.ty = "int".to_owned();
        self
    }

    /// Set the type tag to `"f64"`.
    pub fn f64_type(mut self) -> Self {
        self.ty = "f64".to_owned();
        self
    }

    /// Set the type tag to `"dec"`.
    pub fn dec(mut self) -> Self {
        self.ty = "dec".to_owned();
        self
    }

    /// Set the type tag to `"bool"`.
    pub fn bool_type(mut self) -> Self {
        self.ty = "bool".to_owned();
        self
    }

    /// Set the type tag to `"bin"` (binary).
    pub fn bin(mut self) -> Self {
        self.ty = "bin".to_owned();
        self
    }

    /// Set the type tag to `"list"`.
    pub fn list(mut self) -> Self {
        self.ty = "list".to_owned();
        self
    }

    /// Set the type tag to `"map"`.
    pub fn map(mut self) -> Self {
        self.ty = "map".to_owned();
        self
    }

    /// Set the type tag to `"any"`.
    pub fn any(mut self) -> Self {
        self.ty = "any".to_owned();
        self
    }

    /// Set the type tag to an arbitrary string.
    pub fn type_tag(mut self, tag: impl Into<String>) -> Self {
        self.ty = tag.into();
        self
    }

    // ── constraint setters ─────────────────────────────────────────

    /// Mark the field as required.
    pub fn required(mut self) -> Self {
        self.constraints.required = Some(true);
        self
    }

    /// Mark the field as nullable.
    pub fn nullable(mut self) -> Self {
        self.constraints.nullable = Some(true);
        self
    }

    /// Mark the field as unsigned.
    pub fn unsigned(mut self) -> Self {
        self.constraints.unsigned = Some(true);
        self
    }

    /// Set an integer minimum bound.
    pub fn min(mut self, v: i64) -> Self {
        self.constraints.min = Some(NumDto::Int(v));
        self
    }

    /// Set a floating-point minimum bound.
    pub fn min_f64(mut self, v: f64) -> Self {
        self.constraints.min = Some(NumDto::F64(v));
        self
    }

    /// Set an integer maximum bound.
    pub fn max(mut self, v: i64) -> Self {
        self.constraints.max = Some(NumDto::Int(v));
        self
    }

    /// Set a floating-point maximum bound.
    pub fn max_f64(mut self, v: f64) -> Self {
        self.constraints.max = Some(NumDto::F64(v));
        self
    }

    /// Set an exact-length constraint.
    pub fn len(mut self, v: u64) -> Self {
        self.constraints.len = Some(v);
        self
    }

    /// Set a maximum-length constraint.
    pub fn max_len(mut self, v: u64) -> Self {
        self.constraints.max_len = Some(v);
        self
    }

    /// Set a minimum-length constraint.
    pub fn min_len(mut self, v: u64) -> Self {
        self.constraints.min_len = Some(v);
        self
    }

    /// Set the array element type constraint.
    pub fn array_of(mut self, tag: impl Into<String>) -> Self {
        self.constraints.array_of = Some(tag.into());
        self
    }

    // ── Phase B constraint setters ─────────────────────────────────

    /// Phase B — scalar-bridge: validate the field by calling the named
    /// registered scalar as a predicate.
    pub fn scalar(mut self, name: impl Into<String>) -> Self {
        self.constraints.scalar = Some(name.into());
        self
    }

    /// Phase B — named format check (`"email"` / `"url"` / `"uuid"` / `"date"`).
    pub fn format(mut self, kind: impl Into<String>) -> Self {
        self.constraints.format = Some(kind.into());
        self
    }

    /// Phase B — cross-field comparison against another path.
    ///
    /// `op` is the comparison operator as a string (`"<"`, `"<="`, `"=="`,
    /// `"!="`, `">="`, `">"`).
    pub fn compare<I, S>(mut self, other: I, op: impl Into<String>) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.constraints.compare = Some(CompareDto {
            other: other.into_iter().map(Into::into).collect(),
            op: op.into(),
        });
        self
    }

    /// Finalize into a [`FieldRuleDto`].
    pub fn build(self) -> FieldRuleDto {
        FieldRuleDto {
            path: self.path,
            r#type: self.ty,
            constraints: self.constraints,
        }
    }
}

impl From<FieldBuilder> for FieldRuleDto {
    fn from(b: FieldBuilder) -> Self {
        b.build()
    }
}

// ── set_table_schema ───────────────────────────────────────────────────

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

// ── add_schema_rule ────────────────────────────────────────────────────

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
    /// # Panics
    /// Panics if no rule was set.
    pub fn build(self) -> BatchOp {
        BatchOp::AddSchemaRule(AddSchemaRuleOp {
            add_schema_rule: self.table,
            repo: self.repo,
            rule: self.rule.expect("AddSchemaRuleBuilder: rule is required"),
        })
    }
}

impl From<AddSchemaRuleBuilder> for BatchOp {
    fn from(b: AddSchemaRuleBuilder) -> Self {
        b.build()
    }
}

impl IntoBatchOp for AddSchemaRuleBuilder {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

// ── remove_schema_rule ─────────────────────────────────────────────────

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

// ── get_table_schema ───────────────────────────────────────────────────

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
