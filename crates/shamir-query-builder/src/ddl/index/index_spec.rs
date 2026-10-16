//! Typed, mutually-exclusive index-kind specification ([`IndexSpec`]).
//!
//! **Internal to `shamir-query-builder`** — NOT a wire type. The wire DTO is
//! [`shamir_query_types::admin::CreateIndexOp`], whose shape (field names,
//! types, declaration order, serde attributes) is **frozen**: it is the hard
//! constraint this whole module exists to respect. [`IndexSpec`] is an
//! intermediate representation that makes the mutually-exclusive index kinds
//! (`Hash` / `Sorted` / `Fts` / `Functional` / `Vector`) *unrepresentable* in
//! the illegal combinations that [`super::CreateIndex::try_build`] used to
//! reject by hand with a pile of `bool` flags + a string `index_type` + a
//! scatter of `Option<_>` side-fields.
//!
//! # Where the validation lives
//!
//! The 12 checks that used to be inline in `try_build()` move into the
//! [`TryFrom<&super::CreateIndex>`] impl (defined alongside the builder in
//! `create_index.rs`, because it reads the builder's private fields). Once an
//! [`IndexSpec`] *exists*, the compiler — not a runtime `if` — rules out every
//! combination those checks guarded:
//!
//! - [`IndexSpec::Sorted`] has a single `field` (not `fields`), so a multi-field
//!   sorted index cannot be held (the `SortedMultiField` check fires during
//!   conversion before this variant can be built).
//! - `include` appears *only* on [`IndexSpec::Sorted`]; every other variant has
//!   no such slot, so `IncludeUnsupportedForType` / `IncludeWithoutSorted`
//!   become structurally unreachable after construction.
//! - [`IndexSpec::Vector`] carries `dim: NonZeroU32`, so a zero / absent
//!   dimension cannot be held (`VectorDimRequired` fires during conversion).
//! - No variant carries a sibling family's side-fields (a `Vector` has no
//!   `fts_tokenizer`; a `Sorted` has no `vector_dim`; …), so the cross-family
//!   option-mismatch checks (`VectorOptionsOnNonVectorIndex`, etc.) are
//!   structural.
//!
//! # Orthogonal metadata
//!
//! `create_index` (name) / `table` / `repo` / `if_not_exists` deliberately do
//! NOT appear here — they apply identically regardless of index kind and are
//! passed alongside the spec when flattening back into a [`CreateIndexOp`] via
//! [`IndexSpec::into_op`].

use std::num::NonZeroU32;

use shamir_query_types::admin::CreateIndexOp;
use shamir_types::types::value::QueryValue;

/// A validated index-kind specification.
///
/// See the [module docs](self) for the rationale and for which runtime checks
/// each variant's shape makes structurally impossible.
///
/// Fields are `pub(crate)` so the conversion (in `create_index.rs`) and the
/// typed `CreateIndex` constructors can build variants directly, and the
/// in-crate tests can construct/match them directly. The type itself is not
/// exported beyond the crate — it is an internal IR, not a wire type.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum IndexSpec {
    /// Hash/btree-family index (the default when no `index_type` — or any
    /// non-`{vector,fts,functional}` `index_type` — is set). May be unique.
    ///
    /// `index_type` is preserved verbatim so a caller that explicitly set
    /// `.index_type("btree")` round-trips byte-identically through
    /// `try_build()`; `None` is the default.
    Hash {
        fields: Vec<Vec<String>>,
        unique: bool,
        index_type: Option<String>,
    },
    /// Value-ordered sorted index. Single-field by construction (the
    /// `SortedMultiField` check fires during conversion if the caller supplies
    /// ≠ 1 field); may carry covering (`include`) field paths. Cannot be
    /// unique (`UniqueAndSorted`).
    Sorted {
        field: Vec<String>,
        include: Vec<Vec<String>>,
        index_type: Option<String>,
    },
    /// Full-text index. `index_type` is always `Some("fts")`.
    Fts {
        fields: Vec<Vec<String>>,
        tokenizer: Option<String>,
        language: Option<String>,
    },
    /// Functional (derived) index. `index_type` is always
    /// `Some("functional")`.
    Functional {
        fields: Vec<Vec<String>>,
        op: Option<String>,
        args: Option<Vec<QueryValue>>,
    },
    /// Vector (ANN) index. `index_type` is always `Some("vector")`; `dim` is
    /// [`NonZeroU32`] so a zero/absent dimension is unrepresentable.
    Vector {
        fields: Vec<Vec<String>>,
        dim: NonZeroU32,
        metric: Option<String>,
        quantization: Option<String>,
    },
}

impl IndexSpec {
    /// Flatten the validated spec back into the wire DTO, attaching the
    /// orthogonal metadata (`name` / `table` / `repo` / `if_not_exists`) that
    /// does not depend on index kind.
    ///
    /// Infallible: every field this sets is either a variant slot (already
    /// validated by the [`TryFrom`](TryFrom@super::CreateIndex) construction)
    /// or one of the four orthogonal parameters. Because this reproduces the
    /// exact same field values in the exact same [`CreateIndexOp`] struct, the
    /// msgpack wire bytes are unchanged by construction — which is what keeps
    /// the `wire_hex` assertions in `create_index_matrix.json` byte-identical.
    pub(crate) fn into_op(
        self,
        name: String,
        table: String,
        repo: String,
        if_not_exists: bool,
    ) -> CreateIndexOp {
        // Decompose once, then build the struct. `index_type` / family-specific
        // fields default to `None` / empty for every variant that does not own
        // them, matching the builder's infallible `build()` output exactly.
        let (
            fields,
            unique,
            sorted,
            index_type,
            fts_tokenizer,
            fts_language,
            functional_op,
            functional_args,
            vector_dim,
            vector_metric,
            vector_quantization,
            include,
        ) = match self {
            IndexSpec::Hash {
                fields,
                unique,
                index_type,
            } => (
                fields,
                unique,
                false,
                index_type,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                Vec::new(),
            ),
            IndexSpec::Sorted {
                field,
                include,
                index_type,
            } => (
                vec![field],
                false,
                true,
                index_type,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                include,
            ),
            IndexSpec::Fts {
                fields,
                tokenizer,
                language,
            } => (
                fields,
                false,
                false,
                Some("fts".to_string()),
                tokenizer,
                language,
                None,
                None,
                None,
                None,
                None,
                Vec::new(),
            ),
            IndexSpec::Functional { fields, op, args } => (
                fields,
                false,
                false,
                Some("functional".to_string()),
                None,
                None,
                op,
                args,
                None,
                None,
                None,
                Vec::new(),
            ),
            IndexSpec::Vector {
                fields,
                dim,
                metric,
                quantization,
            } => (
                fields,
                false,
                false,
                Some("vector".to_string()),
                None,
                None,
                None,
                None,
                Some(dim.get()),
                metric,
                quantization,
                Vec::new(),
            ),
        };

        CreateIndexOp {
            create_index: name,
            table,
            fields,
            unique,
            sorted,
            repo,
            index_type,
            fts_tokenizer,
            fts_language,
            functional_op,
            functional_args,
            vector_dim,
            vector_metric,
            vector_quantization,
            include,
            if_not_exists,
        }
    }
}
