use shamir_query_types::admin::CreateIndexOp;
use shamir_query_types::batch::BatchOp;
use shamir_types::types::value::QueryValue;

use crate::batch::IntoBatchOp;

use super::create_index_build_error::CreateIndexBuildError;

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
        vector_quantization: None,
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
    vector_quantization: Option<String>,
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

    /// Set the vector quantization (V5.2 #411). Currently `"sq8"` for SQ8
    /// scalar quantization (deferred fit at 256 vectors → u8-code HNSW graph
    /// with dequant-rescore). `None` / omitted → unquantized f32 HNSW path.
    pub fn vector_quantization(mut self, q: impl Into<String>) -> Self {
        self.vector_quantization = Some(q.into());
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

    /// Consume the builder, run the client-side validation pass, and produce
    /// the wire-ready [`BatchOp`].
    ///
    /// This is the **fallible / validating** sibling of [`CreateIndex::build`].
    /// It constructs the identical [`BatchOp`] as `build()`, but first rejects
    /// the invalid-combination classes the server enforces at DDL-execution time
    /// (`shamir-db::execute::admin_table_index`) and the TS builder
    /// (`shamir-client-ts`) rejects synchronously:
    /// - [`CreateIndexBuildError::UniqueAndSorted`] — `.unique()` + `.sorted()`.
    /// - [`CreateIndexBuildError::IncludeWithoutSorted`] — `.include(...)`
    ///   without `.sorted()`.
    /// - [`CreateIndexBuildError::SortedMultiField`] — `.sorted()` with a field
    ///   count ≠ 1.
    ///
    /// `build()` is unchanged and remains the lenient path for existing call
    /// sites; new code that wants the parity checks should prefer `try_build()`.
    ///
    /// **Scope limitation (F-87, #915):** these three checks cover only the
    /// legacy btree-family combinations `admin_table_index.rs` checks BEFORE
    /// dispatching a non-`"btree"` `index_type` to `create_index_v2`. A
    /// non-btree `index_type` (`"vector"`/`"fts"`/`"functional"`) has
    /// ADDITIONAL server-side validation this method does NOT replicate —
    /// e.g. the one-vector-index-per-table constraint, functional-op
    /// trustedness, FTS tokenizer DSL well-formedness. A caller can still get
    /// a local `Ok` from `try_build()` for one of those families followed by
    /// a server-side rejection on submit; that residual round-trip cost is a
    /// known, accepted scope gap, not an oversight.
    pub fn try_build(self) -> Result<BatchOp, CreateIndexBuildError> {
        if self.sorted && self.unique {
            return Err(CreateIndexBuildError::UniqueAndSorted);
        }
        if !self.include.is_empty() && !self.sorted {
            return Err(CreateIndexBuildError::IncludeWithoutSorted);
        }
        if self.sorted && self.fields.len() != 1 {
            return Err(CreateIndexBuildError::SortedMultiField {
                field_count: self.fields.len(),
            });
        }
        Ok(self.build())
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
            vector_quantization: self.vector_quantization,
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
