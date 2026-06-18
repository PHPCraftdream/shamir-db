use shamir_query_types::admin::CreateIndexOp;
use shamir_query_types::batch::BatchOp;
use shamir_types::types::value::QueryValue;

use crate::batch::IntoBatchOp;

/// Create an index on a table. Returns a builder for the many optional
/// knobs (unique, sorted, FTS, vector, functional).
pub fn create_index(name: impl Into<String>, table: impl Into<String>) -> CreateIndex {
    CreateIndex {
        name: name.into(),
        table: table.into(),
        fields: Vec::new(),
        unique: false,
        sorted: false,
        repo: "main".to_owned(),
        index_type: None,
        fts_tokenizer: None,
        fts_language: None,
        functional_op: None,
        functional_args: None,
        vector_dim: None,
        vector_metric: None,
        include: Vec::new(),
        if_not_exists: false,
    }
}

/// Builder for [`CreateIndexOp`].
pub struct CreateIndex {
    name: String,
    table: String,
    fields: Vec<Vec<String>>,
    unique: bool,
    sorted: bool,
    repo: String,
    index_type: Option<String>,
    fts_tokenizer: Option<String>,
    fts_language: Option<String>,
    functional_op: Option<String>,
    functional_args: Option<Vec<QueryValue>>,
    vector_dim: Option<u32>,
    vector_metric: Option<String>,
    include: Vec<Vec<String>>,
    if_not_exists: bool,
}

impl CreateIndex {
    /// Set the indexed field paths.
    ///
    /// Each element is a path (e.g. `vec!["email"]` or
    /// `vec!["address", "city"]`).
    pub fn fields(mut self, fields: impl IntoIterator<Item = Vec<String>>) -> Self {
        self.fields = fields.into_iter().collect();
        self
    }

    /// Convenience: single-field index (most common case).
    pub fn field(mut self, field: impl Into<String>) -> Self {
        self.fields = vec![vec![field.into()]];
        self
    }

    /// Mark as a unique-constraint index.
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Mark as a sorted (value-ordered) index.
    pub fn sorted(mut self) -> Self {
        self.sorted = true;
        self
    }

    /// Override the target repo (default `"main"`).
    pub fn repo(mut self, repo: impl Into<String>) -> Self {
        self.repo = repo.into();
        self
    }

    /// Set the index type (`"btree"`, `"fts"`, `"functional"`, `"vector"`).
    pub fn index_type(mut self, t: impl Into<String>) -> Self {
        self.index_type = Some(t.into());
        self
    }

    /// Set the FTS tokenizer (`"whitespace"` or `"unicode"`).
    pub fn fts_tokenizer(mut self, tok: impl Into<String>) -> Self {
        self.fts_tokenizer = Some(tok.into());
        self
    }

    /// Set the FTS language hint.
    pub fn fts_language(mut self, lang: impl Into<String>) -> Self {
        self.fts_language = Some(lang.into());
        self
    }

    /// Set the functional index operator.
    pub fn functional_op(mut self, op: impl Into<String>) -> Self {
        self.functional_op = Some(op.into());
        self
    }

    /// Set the functional index arguments.
    pub fn functional_args(mut self, args: Vec<QueryValue>) -> Self {
        self.functional_args = Some(args);
        self
    }

    /// Set the vector dimension.
    pub fn vector_dim(mut self, dim: u32) -> Self {
        self.vector_dim = Some(dim);
        self
    }

    /// Set the vector metric (`"l2"`, `"cosine"`, `"dot"`).
    pub fn vector_metric(mut self, metric: impl Into<String>) -> Self {
        self.vector_metric = Some(metric.into());
        self
    }

    /// Set the covering-index included field paths (sorted indexes only).
    ///
    /// Each element is a field path, e.g. `vec!["email"]` or
    /// `vec!["address", "city"]`. Only meaningful when `.sorted()` is set.
    pub fn include(mut self, paths: impl IntoIterator<Item = Vec<String>>) -> Self {
        self.include = paths.into_iter().collect();
        self
    }

    /// Skip error if the index already exists.
    pub fn if_not_exists(mut self) -> Self {
        self.if_not_exists = true;
        self
    }

    /// Finalize into a [`BatchOp`].
    pub fn build(self) -> BatchOp {
        BatchOp::CreateIndex(CreateIndexOp {
            create_index: self.name,
            table: self.table,
            fields: self.fields,
            unique: self.unique,
            sorted: self.sorted,
            repo: self.repo,
            index_type: self.index_type,
            fts_tokenizer: self.fts_tokenizer,
            fts_language: self.fts_language,
            functional_op: self.functional_op,
            functional_args: self.functional_args,
            vector_dim: self.vector_dim,
            vector_metric: self.vector_metric,
            include: self.include,
            if_not_exists: self.if_not_exists,
        })
    }
}

impl From<CreateIndex> for BatchOp {
    fn from(b: CreateIndex) -> Self {
        b.build()
    }
}

impl IntoBatchOp for CreateIndex {
    fn into_batch_op(self) -> BatchOp {
        self.build()
    }
}
