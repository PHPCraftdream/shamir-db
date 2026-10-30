//! Client-side validation error from [`super::CreateIndex::try_build`].
//!
//! Mirrors the infallible-vs-fallible split already established by
//! `crate::query::QueryBuildError` / `crate::query::Query::try_build` and
//! `crate::batch::BuildError` / `crate::batch::Batch::try_build`: the legacy
//! [`super::CreateIndex::build`] stays infallible and permissive (backward
//! compatibility for existing call sites), while
//! [`super::CreateIndex::try_build`] performs the same construction plus the
//! validation checks the server enforces at DDL-execution time
//! (`shamir-db::execute::admin_table_index`) and returns this error on a
//! semantically ill-formed op — so a caller finds out at *construction* time,
//! not after a full round trip through the server.

/// Validation error returned by [`super::CreateIndex::try_build`].
///
/// [`super::CreateIndex::build`] (the legacy infallible path) does NOT surface
/// these — it remains permissive for backward compatibility. Use `try_build()`
/// to opt into the same checks the server (`admin_table_index.rs`) enforces at
/// DDL-execution time and the TS builder (`shamir-client-ts`) enforces in
/// `build()`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CreateIndexBuildError {
    /// `.unique()` and `.sorted()` were both set.
    ///
    /// The server rejects this in `admin_table_index.rs` with "Index cannot be
    /// both sorted and unique"; the TS builder rejects it synchronously in
    /// `ddl.ts`. `try_build()` mirrors both so the caller finds out at
    /// construction time, not after a server round trip.
    #[error("index cannot be both sorted and unique; call either .unique() or .sorted(), not both — the server (admin_table_index) and the TS builder reject this combination")]
    UniqueAndSorted,
    /// `.include(...)` was set without `.sorted()`.
    ///
    /// Included (covering) fields are only meaningful for sorted indexes — the
    /// server rejects this with "include is only valid for sorted indexes".
    #[error(".include() is only valid for sorted indexes; call .sorted() before .include(), or drop the .include() call — the server rejects include without sorted")]
    IncludeWithoutSorted,
    /// `.sorted()` was set but the index does not have exactly one field.
    ///
    /// Sorted indexes are single-field scalar columns only; the server rejects
    /// composites with "Sorted index requires exactly one field (composite TBD)".
    #[error("sorted index requires exactly one field, got {field_count}; sorted indexes are single-field scalar columns only (composite TBD) — the server rejects multi-field sorted indexes")]
    SortedMultiField {
        /// The number of fields the caller supplied.
        field_count: usize,
    },
    /// No fields were supplied.
    ///
    /// The server rejects this in `admin_table_index.rs` with "CREATE INDEX
    /// requires at least one field" — applies to ALL index types.
    #[error("CREATE INDEX requires at least one field; call .field(...) — the server (admin_table_index) rejects an empty fields list for all index types")]
    EmptyFields,
    /// `.unique()` was set with a non-btree `index_type` (`"vector"` /
    /// `"fts"` / `"functional"`).
    ///
    /// Only btree/hash indexes can be unique; the server rejects this with
    /// "`unique` is not supported for '{index_type}' indexes; only btree/hash
    /// indexes can be unique".
    #[error("`unique` is not supported for '{index_type}' indexes; only btree/hash indexes can be unique — the server (admin_table_index) rejects this combination")]
    UniqueUnsupportedForType {
        /// The offending index_type string.
        index_type: String,
    },
    /// `.sorted()` was set with a non-btree `index_type` (`"vector"` /
    /// `"fts"` / `"functional"`).
    ///
    /// The server rejects this with "`sorted` is not supported for
    /// '{index_type}' indexes".
    #[error("`sorted` is not supported for '{index_type}' indexes — the server (admin_table_index) rejects this combination")]
    SortedUnsupportedForType {
        /// The offending index_type string.
        index_type: String,
    },
    /// A vector index was requested but `vector_dim` was omitted or zero.
    ///
    /// The server rejects this with "vector index requires `vector_dim` > 0".
    #[error("vector index requires `vector_dim` > 0; call .vector_dim(n) with a positive dimension — the server (admin_table_index) rejects a missing or zero dimension")]
    VectorDimRequired,
    /// An unrecognized `vector_metric` string was supplied for a vector index.
    ///
    /// The server rejects this with "unknown vector_metric '{metric}';
    /// expected 'l2', 'dot', or 'cosine'".
    #[error("unknown vector_metric '{metric}'; expected 'l2', 'dot', or 'cosine' — the server (admin_table_index) rejects unrecognized metric strings")]
    UnknownVectorMetric {
        /// The unrecognized metric string.
        metric: String,
    },
    /// Vector-specific options (`vector_dim` / `vector_metric` /
    /// `vector_quantization`) were set on a non-vector index.
    ///
    /// The server rejects this with "vector_dim/vector_metric/
    /// vector_quantization are only valid for 'vector' indexes".
    #[error("vector_dim/vector_metric/vector_quantization are only valid for 'vector' indexes — the server (admin_table_index) rejects these options on any non-vector index type")]
    VectorOptionsOnNonVectorIndex,
    /// FTS-specific options (`fts_tokenizer` / `fts_language`) were set on a
    /// non-FTS index.
    ///
    /// The server rejects this with "fts_tokenizer/fts_language are only
    /// valid for 'fts' indexes".
    #[error("fts_tokenizer/fts_language are only valid for 'fts' indexes — the server (admin_table_index) rejects these options on any non-fts index type")]
    FtsOptionsOnNonFtsIndex,
    /// Functional-specific options (`functional_op` / `functional_args`)
    /// were set on a non-functional index.
    ///
    /// The server rejects this with "functional_op/functional_args are only
    /// valid for 'functional' indexes".
    #[error("functional_op/functional_args are only valid for 'functional' indexes — the server (admin_table_index) rejects these options on any non-functional index type")]
    FunctionalOptionsOnNonFunctionalIndex,
    /// `.include(...)` was set with a non-btree `index_type` (`"vector"` /
    /// `"fts"` / `"functional"`).
    ///
    /// Covering fields are only meaningful for sorted btree indexes; the
    /// server rejects this with "`include` is not supported for
    /// '{index_type}' indexes; covering fields are only valid for sorted
    /// indexes".
    #[error("`include` is not supported for '{index_type}' indexes; covering fields are only valid for sorted indexes — the server (admin_table_index) rejects this combination")]
    IncludeUnsupportedForType {
        /// The offending index_type string.
        index_type: String,
    },
    /// A stringly setter left state on the builder that a typed terminal
    /// constructor (`.hash()`, `.unique_index()`, `.sorted_index()`, …) would
    /// silently discard.
    ///
    /// Unlike the variants above (which mirror server-side rejections), this is
    /// a purely client-side builder-misuse detector: the typed constructors read
    /// only `name` / `table` / `repo` / `if_not_exists` off the builder, so any
    /// other setter call (`.unique()`, `.fields()`, `.vector_dim()`, …) is dead
    /// state that gets overwritten by the constructor's own parameters. Rather
    /// than silently drop it — which in the worst case (`.unique().hash(...)`)
    /// sends `unique: false` over the wire despite the caller asking for a
    /// uniqueness constraint — the constructor returns this error.
    ///
    /// `method` is the typed constructor name (e.g. `"hash"`); `field` is the
    /// conflicting setter field name (e.g. `"unique"`).
    #[error("the typed constructor .{method}() does not read the builder's .{field}() state; the parameter(s) passed to .{method}() are authoritative, so the prior .{field}() call would be silently discarded — drop the .{field}() call, or switch to the typed constructor that matches your intent (this is a client-side builder-misuse check, not a server-side rejection)")]
    ConflictingBuilderState {
        /// The typed constructor that detected the conflict (e.g. `"hash"`).
        method: &'static str,
        /// The builder field carrying stale state (e.g. `"unique"`).
        field: &'static str,
    },
}
