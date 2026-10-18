use shamir_query_types::admin::{
    BindValidatorOp, CreateValidatorOp, DropValidatorOp, ListValidatorsOp, RenameValidatorOp,
    UnbindValidatorOp,
};
use shamir_query_types::batch::BatchOp;
use shamir_query_types::WriteOp;

use crate::batch::IntoBatchOp;

/// Create (or replace) a validator. Returns a builder for source/wasm.
pub fn create_validator(name: impl Into<String>) -> CreateValidator {
    CreateValidator {
        name: name.into(),
        source: None,
        wasm: None,
        replace: false,
    }
}

/// Builder for [`CreateValidatorOp`].
pub struct CreateValidator {
    name: String,
    source: Option<String>,
    wasm: Option<String>,
    replace: bool,
}

impl CreateValidator {
    /// Set the Rust source code to compile.
    pub fn source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Set the pre-compiled WASM bytes (base64-encoded).
    pub fn wasm(mut self, wasm: impl Into<String>) -> Self {
        self.wasm = Some(wasm.into());
        self
    }

    /// Enable replace-if-exists semantics.
    pub fn replace(mut self) -> Self {
        self.replace = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateValidator(CreateValidatorOp {
            create_validator: self.name,
            source: self.source,
            wasm: self.wasm,
            replace: self.replace,
        })
    }
}

impl From<CreateValidator> for BatchOp {
    fn from(b: CreateValidator) -> Self {
        b.build()
    }
}

impl IntoBatchOp for CreateValidator {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Drop a validator by name. Returns a builder for optional flags.
pub fn drop_validator(name: impl Into<String>) -> DropValidator_ {
    DropValidator_ {
        name: name.into(),
        if_exists: false,
    }
}

/// Builder for [`DropValidatorOp`].
pub struct DropValidator_ {
    name: String,
    if_exists: bool,
}

impl DropValidator_ {
    /// Enable `IF EXISTS` semantics: dropping a non-existent validator is
    /// a silent no-op (`existed: false`) instead of an error.
    pub fn if_exists(mut self) -> Self {
        self.if_exists = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::DropValidator(DropValidatorOp {
            drop_validator: self.name,
            if_exists: self.if_exists,
        })
    }
}

impl From<DropValidator_> for BatchOp {
    fn from(b: DropValidator_) -> Self {
        b.build()
    }
}

impl IntoBatchOp for DropValidator_ {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Rename a validator.
pub fn rename_validator(from: impl Into<String>, to: impl Into<String>) -> BatchOp {
    BatchOp::RenameValidator(RenameValidatorOp {
        rename_validator: from.into(),
        to: to.into(),
    })
}

/// Bind a validator to a table on specified write operations. Returns a builder.
pub fn bind_validator(name: impl Into<String>, table: impl Into<String>) -> BindValidator {
    BindValidator {
        name: name.into(),
        db: String::new(),
        repo: "main".to_owned(),
        table: table.into(),
        ops: Vec::new(),
        priority: 1500,
    }
}

/// Builder for [`BindValidatorOp`].
pub struct BindValidator {
    name: String,
    db: String,
    repo: String,
    table: String,
    ops: Vec<WriteOp>,
    priority: u16,
}

impl BindValidator {
    /// Set the database name.
    pub fn db(mut self, db: impl Into<String>) -> Self {
        self.db = db.into();
        self
    }

    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Set the write operations the validator fires on.
    pub fn ops(mut self, ops: impl IntoIterator<Item = WriteOp>) -> Self {
        self.ops = ops.into_iter().collect();
        self
    }

    /// Set the priority (must be in `[1000, 9999]`).
    pub fn priority(mut self, priority: u16) -> Self {
        self.priority = priority;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::BindValidator(BindValidatorOp {
            bind_validator: self.name,
            db: self.db,
            repo: self.repo,
            table: self.table,
            ops: self.ops,
            priority: self.priority,
        })
    }
}

impl From<BindValidator> for BatchOp {
    fn from(b: BindValidator) -> Self {
        b.build()
    }
}

impl IntoBatchOp for BindValidator {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// Unbind a validator from a table. Returns a builder for optional fields.
pub fn unbind_validator(name: impl Into<String>, table: impl Into<String>) -> UnbindValidator {
    UnbindValidator {
        name: name.into(),
        db: String::new(),
        repo: "main".to_owned(),
        table: table.into(),
    }
}

/// Builder for [`UnbindValidatorOp`].
pub struct UnbindValidator {
    name: String,
    db: String,
    repo: String,
    table: String,
}

impl UnbindValidator {
    /// Set the database name.
    pub fn db(mut self, db: impl Into<String>) -> Self {
        self.db = db.into();
        self
    }

    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::UnbindValidator(UnbindValidatorOp {
            unbind_validator: self.name,
            db: self.db,
            repo: self.repo,
            table: self.table,
        })
    }
}

impl From<UnbindValidator> for BatchOp {
    fn from(b: UnbindValidator) -> Self {
        b.build()
    }
}

impl IntoBatchOp for UnbindValidator {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}

/// List validator bindings for a table. Returns a builder for optional fields.
pub fn list_validators(table: impl Into<String>) -> ListValidatorsBuilder {
    ListValidatorsBuilder {
        table: table.into(),
        db: String::new(),
        repo: "main".to_owned(),
    }
}

/// Builder for [`ListValidatorsOp`].
pub struct ListValidatorsBuilder {
    table: String,
    db: String,
    repo: String,
}

impl ListValidatorsBuilder {
    /// Set the database name.
    pub fn db(mut self, db: impl Into<String>) -> Self {
        self.db = db.into();
        self
    }

    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::ListValidators(ListValidatorsOp {
            list_validators: self.table,
            db: self.db,
            repo: self.repo,
        })
    }
}

impl From<ListValidatorsBuilder> for BatchOp {
    fn from(b: ListValidatorsBuilder) -> Self {
        b.build()
    }
}

impl IntoBatchOp for ListValidatorsBuilder {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
