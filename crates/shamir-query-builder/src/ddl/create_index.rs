use std::num::NonZeroU32;

use shamir_query_types::admin::CreateIndexOp;
use shamir_query_types::batch::BatchOp;
use shamir_types::types::value::QueryValue;

use crate::batch::IntoBatchOp;

use super::create_index_build_error::CreateIndexBuildError;
use super::metric::Metric;
use super::quantization::Quantization;
use super::tokenizer::Tokenizer;
use super::IndexSpec;

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
    /// Detect builder state set by the stringly setters that a typed terminal
    /// constructor would silently discard.
    ///
    /// Returns [`CreateIndexBuildError::ConflictingBuilderState`] on the first
    /// stale field found. This is the core of the F-7 fix (#1075): every typed
    /// constructor calls this before building its `IndexSpec` variant, so a
    /// caller mixing stringly setters with a typed terminal gets an error
    /// instead of a silently-wrong `BatchOp`.
    ///
    /// # The `bool` ambiguity
    ///
    /// `unique: bool` and `sorted: bool` have no sentinel for "never touched" —
    /// their default is `false`, and the setter can only flip them to `true`.
    /// So the ONLY distinguishable conflict is `true` (setter was called) vs
    /// `false` (default). When the typed constructor itself produces the same
    /// `true` value for that field (e.g. `.unique_index()` always sets
    /// `unique: true`), a prior setter call is **redundant** (same intent), not
    /// a conflict, and is exempted via the corresponding `*_ok` flag.
    ///
    /// `unique_ok`: exempt `self.unique` (used by `.unique_index()`).
    /// `sorted_ok`: exempt `self.sorted` (used by `.sorted_index()` /
    /// `.sorted_with_include()`).
    fn check_conflicting_state(
        &self,
        method: &'static str,
        unique_ok: bool,
        sorted_ok: bool,
    ) -> Result<(), CreateIndexBuildError> {
        if !unique_ok && self.unique {
            return Err(CreateIndexBuildError::ConflictingBuilderState {
                method,
                field: "unique",
            });
        }
        if !sorted_ok && self.sorted {
            return Err(CreateIndexBuildError::ConflictingBuilderState {
                method,
                field: "sorted",
            });
        }
        if !self.fields.is_empty() {
            return Err(CreateIndexBuildError::ConflictingBuilderState {
                method,
                field: "fields",
            });
        }
        if self.index_type.is_some() {
            return Err(CreateIndexBuildError::ConflictingBuilderState {
                method,
                field: "index_type",
            });
        }
        if self.fts_tokenizer.is_some() {
            return Err(CreateIndexBuildError::ConflictingBuilderState {
                method,
                field: "fts_tokenizer",
            });
        }
        if self.fts_language.is_some() {
            return Err(CreateIndexBuildError::ConflictingBuilderState {
                method,
                field: "fts_language",
            });
        }
        if self.functional_op.is_some() {
            return Err(CreateIndexBuildError::ConflictingBuilderState {
                method,
                field: "functional_op",
            });
        }
        if self.functional_args.is_some() {
            return Err(CreateIndexBuildError::ConflictingBuilderState {
                method,
                field: "functional_args",
            });
        }
        if self.vector_dim.is_some() {
            return Err(CreateIndexBuildError::ConflictingBuilderState {
                method,
                field: "vector_dim",
            });
        }
        if self.vector_metric.is_some() {
            return Err(CreateIndexBuildError::ConflictingBuilderState {
                method,
                field: "vector_metric",
            });
        }
        if self.vector_quantization.is_some() {
            return Err(CreateIndexBuildError::ConflictingBuilderState {
                method,
                field: "vector_quantization",
            });
        }
        if !self.include.is_empty() {
            return Err(CreateIndexBuildError::ConflictingBuilderState {
                method,
                field: "include",
            });
        }
        Ok(())
    }

    /// Create a hash/btree index on one or more fields (not unique).
    ///
    /// This is a **strict-by-default** typed constructor: it validates the
    /// builder state and field count, then produces a valid [`BatchOp`]. The
    /// output is byte-identical to the equivalent stringly
    /// `.fields(...).build()` call.
    ///
    /// Returns [`CreateIndexBuildError::ConflictingBuilderState`] if any
    /// stringly setter (`.unique()`, `.sorted()`, `.index_type()`, …) was called
    /// before this method — the typed constructor's `fields` parameter is
    /// authoritative and would silently discard such state. Returns
    /// [`CreateIndexBuildError::EmptyFields`] if `fields` is empty.
    ///
    /// # Example
    ///
    /// ```
    /// use shamir_query_builder::ddl::create_index;
    ///
    /// let op = create_index("idx_email", "users")
    ///     .hash(vec![vec!["email".to_string()]])
    ///     .unwrap();
    /// ```
    pub fn hash(
        self,
        fields: impl Into<Vec<Vec<String>>>,
    ) -> Result<BatchOp, CreateIndexBuildError> {
        self.check_conflicting_state("hash", false, false)?;
        let fields = fields.into();
        if fields.is_empty() {
            return Err(CreateIndexBuildError::EmptyFields);
        }
        let spec = IndexSpec::Hash {
            fields,
            unique: false,
            index_type: None,
        };

        Ok(BatchOp::CreateIndex(spec.into_op(
            self.name,
            self.table,
            self.repo,
            self.if_not_exists,
        )))
    }

    /// Create a unique hash/btree index on one or more fields.
    ///
    /// This is a **strict-by-default** typed constructor: it validates the
    /// builder state and field count, then produces a valid [`BatchOp`]. The
    /// output is byte-identical to the equivalent stringly
    /// `.fields(...).unique().build()` call.
    ///
    /// A prior `.unique()` call is **redundant** (same intent) and is NOT
    /// treated as a conflict — `unique: bool` has no sentinel for "never
    /// touched", so there is no way to distinguish it from the default.
    ///
    /// Returns [`CreateIndexBuildError::ConflictingBuilderState`] if any other
    /// stringly setter was called before this method. Returns
    /// [`CreateIndexBuildError::EmptyFields`] if `fields` is empty.
    ///
    /// # Example
    ///
    /// ```
    /// use shamir_query_builder::ddl::create_index;
    ///
    /// let op = create_index("idx_email_unique", "users")
    ///     .unique_index(vec![vec!["email".to_string()]])
    ///     .unwrap();
    /// ```
    pub fn unique_index(
        self,
        fields: impl Into<Vec<Vec<String>>>,
    ) -> Result<BatchOp, CreateIndexBuildError> {
        self.check_conflicting_state("unique_index", true, false)?;
        let fields = fields.into();
        if fields.is_empty() {
            return Err(CreateIndexBuildError::EmptyFields);
        }
        let spec = IndexSpec::Hash {
            fields,
            unique: true,
            index_type: None,
        };

        Ok(BatchOp::CreateIndex(spec.into_op(
            self.name,
            self.table,
            self.repo,
            self.if_not_exists,
        )))
    }

    /// Create a sorted (value-ordered) index on a single field.
    ///
    /// This is a **strict-by-default** typed constructor: it validates the
    /// builder state and field count, then produces a valid [`BatchOp`]. The
    /// output is byte-identical to the equivalent stringly
    /// `.field(...).sorted().build()` call.
    ///
    /// The `field` parameter accepts a single field path (`Vec<String>` or
    /// `impl Into<Vec<String>>`), making multi-field sorted indexes a compile-time
    /// error.
    ///
    /// A prior `.sorted()` call is **redundant** (same intent) and is NOT
    /// treated as a conflict — `sorted: bool` has no sentinel for "never
    /// touched", so there is no way to distinguish it from the default.
    ///
    /// Returns [`CreateIndexBuildError::ConflictingBuilderState`] if any other
    /// stringly setter was called before this method. Returns
    /// [`CreateIndexBuildError::EmptyFields`] if `field` is an empty path.
    ///
    /// # Example
    ///
    /// ```
    /// use shamir_query_builder::ddl::create_index;
    ///
    /// let op = create_index("idx_age", "users")
    ///     .sorted_index(vec!["age".to_string()])
    ///     .unwrap();
    /// ```
    pub fn sorted_index(
        self,
        field: impl Into<Vec<String>>,
    ) -> Result<BatchOp, CreateIndexBuildError> {
        self.check_conflicting_state("sorted_index", false, true)?;
        let field = field.into();
        if field.is_empty() {
            return Err(CreateIndexBuildError::EmptyFields);
        }
        let spec = IndexSpec::Sorted {
            field,
            include: Vec::new(),
            index_type: None,
        };

        Ok(BatchOp::CreateIndex(spec.into_op(
            self.name,
            self.table,
            self.repo,
            self.if_not_exists,
        )))
    }

    /// Create a sorted index with covering (include) fields.
    ///
    /// This is a **strict-by-default** typed constructor: it validates the
    /// builder state and field count, then produces a valid [`BatchOp`]. The
    /// output is byte-identical to the equivalent stringly
    /// `.field(...).sorted().include(...).build()` call.
    ///
    /// A prior `.sorted()` call is **redundant** (same intent) and is NOT
    /// treated as a conflict.
    ///
    /// Returns [`CreateIndexBuildError::ConflictingBuilderState`] if any other
    /// stringly setter was called before this method. Returns
    /// [`CreateIndexBuildError::EmptyFields`] if `field` is an empty path.
    ///
    /// # Example
    ///
    /// ```
    /// use shamir_query_builder::ddl::create_index;
    ///
    /// let op = create_index("idx_age_inc", "users")
    ///     .sorted_with_include(
    ///         vec!["age".to_string()],
    ///         vec![vec!["email".to_string()]],
    ///     )
    ///     .unwrap();
    /// ```
    pub fn sorted_with_include(
        self,
        field: impl Into<Vec<String>>,
        include: impl IntoIterator<Item = Vec<String>>,
    ) -> Result<BatchOp, CreateIndexBuildError> {
        self.check_conflicting_state("sorted_with_include", false, true)?;
        let field = field.into();
        if field.is_empty() {
            return Err(CreateIndexBuildError::EmptyFields);
        }
        let spec = IndexSpec::Sorted {
            field,
            include: include.into_iter().collect(),
            index_type: None,
        };

        Ok(BatchOp::CreateIndex(spec.into_op(
            self.name,
            self.table,
            self.repo,
            self.if_not_exists,
        )))
    }

    /// Create a full-text search index on a single field with a tokenizer.
    ///
    /// This is a **strict-by-default** typed constructor: it validates the
    /// builder state and field count, then produces a valid [`BatchOp`]. The
    /// output is byte-identical to the equivalent stringly
    /// `.field(...).index_type("fts").fts_tokenizer(...).build()` call.
    ///
    /// Returns [`CreateIndexBuildError::ConflictingBuilderState`] if any
    /// stringly setter was called before this method. Returns
    /// [`CreateIndexBuildError::EmptyFields`] if `field` is an empty path.
    ///
    /// # Example
    ///
    /// ```
    /// use shamir_query_builder::ddl::{create_index, Tokenizer};
    ///
    /// let op = create_index("idx_body", "posts")
    ///     .fts(vec!["body".to_string()], Tokenizer::Whitespace)
    ///     .unwrap();
    /// ```
    pub fn fts(
        self,
        field: impl Into<Vec<String>>,
        tokenizer: Tokenizer,
    ) -> Result<BatchOp, CreateIndexBuildError> {
        self.fts_with_language(field, tokenizer, None)
    }

    /// Create a full-text search index with a language hint.
    ///
    /// This is a **strict-by-default** typed constructor: it validates the
    /// builder state and field count, then produces a valid [`BatchOp`].
    ///
    /// Returns [`CreateIndexBuildError::ConflictingBuilderState`] if any
    /// stringly setter was called before this method. Returns
    /// [`CreateIndexBuildError::EmptyFields`] if `field` is an empty path.
    ///
    /// # Example
    ///
    /// ```
    /// use shamir_query_builder::ddl::{create_index, Tokenizer};
    ///
    /// let op = create_index("idx_body_lang", "posts")
    ///     .fts_with_language(
    ///         vec!["body".to_string()],
    ///         Tokenizer::Unicode,
    ///         Some("en".to_string()),
    ///     )
    ///     .unwrap();
    /// ```
    pub fn fts_with_language(
        self,
        field: impl Into<Vec<String>>,
        tokenizer: Tokenizer,
        language: Option<String>,
    ) -> Result<BatchOp, CreateIndexBuildError> {
        self.check_conflicting_state("fts_with_language", false, false)?;
        let field = field.into();
        if field.is_empty() {
            return Err(CreateIndexBuildError::EmptyFields);
        }
        let spec = IndexSpec::Fts {
            fields: vec![field],
            tokenizer: Some(tokenizer.into()),
            language,
        };

        Ok(BatchOp::CreateIndex(spec.into_op(
            self.name,
            self.table,
            self.repo,
            self.if_not_exists,
        )))
    }

    /// Create a functional (derived) index on a single field.
    ///
    /// This is a **strict-by-default** typed constructor: it validates the
    /// builder state and field count, then produces a valid [`BatchOp`]. The
    /// output is byte-identical to the equivalent stringly
    /// `.field(...).index_type("functional").functional_op(...).build()` call.
    ///
    /// The `func` parameter is a plain function name string (e.g., `"lower"`),
    /// matching what `functional_op` already stores in the wire format.
    ///
    /// Returns [`CreateIndexBuildError::ConflictingBuilderState`] if any
    /// stringly setter was called before this method. Returns
    /// [`CreateIndexBuildError::EmptyFields`] if `field` is an empty path.
    ///
    /// # Example
    ///
    /// ```
    /// use shamir_query_builder::ddl::create_index;
    ///
    /// let op = create_index("idx_email_lower", "users")
    ///     .functional(vec!["email".to_string()], "lower")
    ///     .unwrap();
    /// ```
    pub fn functional(
        self,
        field: impl Into<Vec<String>>,
        func: impl Into<String>,
    ) -> Result<BatchOp, CreateIndexBuildError> {
        self.functional_with_args(field, func, Vec::new())
    }

    /// Create a functional index with arguments.
    ///
    /// This is a **strict-by-default** typed constructor that validates the
    /// builder state and field count, accepts both the function name and
    /// optional arguments, and produces a valid [`BatchOp`].
    ///
    /// Returns [`CreateIndexBuildError::ConflictingBuilderState`] if any
    /// stringly setter was called before this method. Returns
    /// [`CreateIndexBuildError::EmptyFields`] if `field` is an empty path.
    ///
    /// # Example
    ///
    /// ```
    /// use shamir_query_builder::ddl::create_index;
    /// use shamir_types::types::value::QueryValue;
    ///
    /// let op = create_index("idx_custom", "users")
    ///     .functional_with_args(
    ///         vec!["email".to_string()],
    ///         "my_func",
    ///         vec![QueryValue::from("arg1")],
    ///     )
    ///     .unwrap();
    /// ```
    pub fn functional_with_args(
        self,
        field: impl Into<Vec<String>>,
        func: impl Into<String>,
        args: Vec<QueryValue>,
    ) -> Result<BatchOp, CreateIndexBuildError> {
        self.check_conflicting_state("functional_with_args", false, false)?;
        let field = field.into();
        if field.is_empty() {
            return Err(CreateIndexBuildError::EmptyFields);
        }
        let spec = IndexSpec::Functional {
            fields: vec![field],
            op: Some(func.into()),
            args: if args.is_empty() { None } else { Some(args) },
        };

        Ok(BatchOp::CreateIndex(spec.into_op(
            self.name,
            self.table,
            self.repo,
            self.if_not_exists,
        )))
    }

    /// Create a vector index on a single field with dimension, metric, and quantization.
    ///
    /// This is a **strict-by-default** typed constructor: it validates the
    /// builder state and field count, then produces a valid [`BatchOp`]. The
    /// output is byte-identical to the equivalent stringly
    /// `.field(...).index_type("vector").vector_dim(...).vector_metric(...).vector_quantization(...).build()`
    /// call.
    ///
    /// The `dim` parameter is `NonZeroU32`, making zero or absent dimensions
    /// a compile-time impossibility.
    ///
    /// Returns [`CreateIndexBuildError::ConflictingBuilderState`] if any
    /// stringly setter was called before this method. Returns
    /// [`CreateIndexBuildError::EmptyFields`] if `field` is an empty path.
    ///
    /// # Example
    ///
    /// ```
    /// use shamir_query_builder::ddl::{create_index, Metric, Quantization};
    /// use std::num::NonZeroU32;
    ///
    /// let dim = NonZeroU32::new(384).unwrap();
    /// let op = create_index("idx_embedding", "docs")
    ///     .vector(vec!["embedding".to_string()], dim, Metric::Cosine, Quantization::Off)
    ///     .unwrap();
    /// ```
    pub fn vector(
        self,
        field: impl Into<Vec<String>>,
        dim: NonZeroU32,
        metric: Metric,
        quantization: Quantization,
    ) -> Result<BatchOp, CreateIndexBuildError> {
        self.check_conflicting_state("vector", false, false)?;
        let field = field.into();
        if field.is_empty() {
            return Err(CreateIndexBuildError::EmptyFields);
        }
        let spec = IndexSpec::Vector {
            fields: vec![field],
            dim,
            metric: Some(metric.into()),
            quantization: quantization.into(),
        };

        Ok(BatchOp::CreateIndex(spec.into_op(
            self.name,
            self.table,
            self.repo,
            self.if_not_exists,
        )))
    }

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
    /// - [`CreateIndexBuildError::EmptyFields`] — no fields supplied.
    /// - [`CreateIndexBuildError::UniqueUnsupportedForType`] — `.unique()`
    ///   with a non-btree `index_type`.
    /// - [`CreateIndexBuildError::SortedUnsupportedForType`] — `.sorted()`
    ///   with a non-btree `index_type`.
    /// - [`CreateIndexBuildError::VectorDimRequired`] — vector index without a
    ///   positive `vector_dim`.
    /// - [`CreateIndexBuildError::UnknownVectorMetric`] — vector index with an
    ///   unrecognized `vector_metric` string.
    /// - [`CreateIndexBuildError::VectorOptionsOnNonVectorIndex`] —
    ///   vector-specific options on a non-vector index.
    /// - [`CreateIndexBuildError::FtsOptionsOnNonFtsIndex`] — FTS-specific
    ///   options on a non-FTS index.
    /// - [`CreateIndexBuildError::FunctionalOptionsOnNonFunctionalIndex`] —
    ///   functional-specific options on a non-functional index.
    /// - [`CreateIndexBuildError::IncludeUnsupportedForType`] — `.include(...)`
    ///   with a non-btree `index_type`.
    ///
    /// `build()` is unchanged and remains the lenient path for existing call
    /// sites; new code that wants the parity checks should prefer `try_build()`.
    ///
    /// **Scope limitation (P1-6, #970):** the checks above now cover the
    /// base_index btree-family combinations AND the cross-type /
    /// family-specific-field mismatches that `admin_table_index.rs` enforces
    /// before dispatching to `create_index_v2`. A non-btree `index_type`
    /// (`"vector"`/`"fts"`/`"functional"`) has ADDITIONAL server-side
    /// validation this method does NOT replicate — e.g. the
    /// one-vector-index-per-table constraint, functional-op trustedness, FTS
    /// tokenizer DSL well-formedness. A caller can still get a local `Ok`
    /// from `try_build()` for one of those families followed by a server-side
    /// rejection on submit; that residual round-trip cost is a known, accepted
    /// scope gap, not an oversight.
    pub fn try_build(self) -> Result<BatchOp, CreateIndexBuildError> {
        // Validate the builder into a typed `IndexSpec`, then flatten the
        // validated spec back into the wire `CreateIndexOp`. All 12 checks now
        // live in `TryFrom<&CreateIndex> for IndexSpec` (see below); this method
        // is just the spec-round-trip + the orthogonal-metadata attach.
        //
        // The borrow of `&self` for the conversion ends before the partial
        // moves of `name` / `table` / `repo` / `if_not_exists` below (NLL), so
        // the four orthogonal fields move straight into `into_op` — no clones
        // of them, only of the validated kind-fields captured by the spec.
        let spec = IndexSpec::try_from(&self)?;
        Ok(BatchOp::CreateIndex(spec.into_op(
            self.name,
            self.table,
            self.repo,
            self.if_not_exists,
        )))
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

/// Validate a [`CreateIndex`] builder into a typed [`IndexSpec`].
///
/// This is where `try_build()`'s 12 validation checks now live. The checks run
/// in the **exact same order** as the pre-refactor inline `try_build()`, so the
/// first-error decision is identical for every input — including all 21 cases
/// of `create_index_matrix.json`. On success the matching [`IndexSpec`] variant
/// is constructed; by construction it can no longer hold any of the
/// combinations the checks above guard (e.g. a `Sorted` has a single `field`,
/// `include` exists only on `Sorted`, `Vector::dim` is `NonZeroU32`, and no
/// variant carries a sibling family's side-fields).
impl TryFrom<&CreateIndex> for IndexSpec {
    type Error = CreateIndexBuildError;

    fn try_from(b: &CreateIndex) -> Result<Self, Self::Error> {
        let itype = b.index_type.as_deref();
        let non_btree = matches!(itype, Some("vector") | Some("fts") | Some("functional"));

        // 1. At least one field for ANY index type.
        if b.fields.is_empty() {
            return Err(CreateIndexBuildError::EmptyFields);
        }
        // 2. `unique` is only meaningful for btree/hash indexes.
        if b.unique && non_btree {
            return Err(CreateIndexBuildError::UniqueUnsupportedForType {
                index_type: itype.unwrap().to_string(),
            });
        }
        // 3. `sorted` is only meaningful for btree indexes.
        if b.sorted && non_btree {
            return Err(CreateIndexBuildError::SortedUnsupportedForType {
                index_type: itype.unwrap().to_string(),
            });
        }
        // 4. Vector index requires a positive dimension.
        if itype == Some("vector") && (b.vector_dim.is_none() || b.vector_dim == Some(0)) {
            return Err(CreateIndexBuildError::VectorDimRequired);
        }
        // 5. Vector metric must be a recognized value.
        if itype == Some("vector") {
            if let Some(m) = b.vector_metric.as_deref() {
                if !matches!(m, "l2" | "dot" | "cosine") {
                    return Err(CreateIndexBuildError::UnknownVectorMetric {
                        metric: m.to_string(),
                    });
                }
            }
        }
        // 6. Vector-specific options are only valid for vector indexes.
        if itype != Some("vector")
            && (b.vector_dim.is_some()
                || b.vector_metric.is_some()
                || b.vector_quantization.is_some())
        {
            return Err(CreateIndexBuildError::VectorOptionsOnNonVectorIndex);
        }
        // 7. FTS-specific options are only valid for FTS indexes.
        if itype != Some("fts") && (b.fts_tokenizer.is_some() || b.fts_language.is_some()) {
            return Err(CreateIndexBuildError::FtsOptionsOnNonFtsIndex);
        }
        // 8. Functional-specific options are only valid for functional indexes.
        if itype != Some("functional") && (b.functional_op.is_some() || b.functional_args.is_some())
        {
            return Err(CreateIndexBuildError::FunctionalOptionsOnNonFunctionalIndex);
        }
        // 9. `include` (covering index) is only meaningful for sorted btree indexes.
        if !b.include.is_empty() && non_btree {
            return Err(CreateIndexBuildError::IncludeUnsupportedForType {
                index_type: itype.unwrap().to_string(),
            });
        }

        // Existing btree-family checks (F-81 #908 / F-87 #915).
        if b.sorted && b.unique {
            return Err(CreateIndexBuildError::UniqueAndSorted);
        }
        if !b.include.is_empty() && !b.sorted {
            return Err(CreateIndexBuildError::IncludeWithoutSorted);
        }
        if b.sorted && b.fields.len() != 1 {
            return Err(CreateIndexBuildError::SortedMultiField {
                field_count: b.fields.len(),
            });
        }

        // All checks passed — build the variant. Each branch is now
        // structurally incapable of holding the combinations rejected above.
        Ok(match itype {
            Some("vector") => IndexSpec::Vector {
                fields: b.fields.clone(),
                // Check 4 guarantees Some and > 0 here.
                dim: NonZeroU32::new(b.vector_dim.expect("vector_dim checked Some & > 0"))
                    .expect("vector_dim checked Some & > 0"),
                metric: b.vector_metric.clone(),
                quantization: b.vector_quantization.clone(),
            },
            Some("fts") => IndexSpec::Fts {
                fields: b.fields.clone(),
                tokenizer: b.fts_tokenizer.clone(),
                language: b.fts_language.clone(),
            },
            Some("functional") => IndexSpec::Functional {
                fields: b.fields.clone(),
                op: b.functional_op.clone(),
                args: b.functional_args.clone(),
            },
            // Btree family: `itype` is None (default) or any non-
            // {vector,fts,functional} string (e.g. an explicit "btree"),
            // preserved verbatim on the variant so try_build()/build() can
            // never diverge.
            _ if b.sorted => IndexSpec::Sorted {
                // Check 12 guarantees exactly one field here.
                field: b
                    .fields
                    .first()
                    .expect("sorted index checked to have exactly one field")
                    .clone(),
                include: b.include.clone(),
                index_type: itype.map(str::to_string),
            },
            _ => IndexSpec::Hash {
                fields: b.fields.clone(),
                unique: b.unique,
                index_type: itype.map(str::to_string),
            },
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
