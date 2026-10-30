//! Shared validation error for the write/DDL op builders that previously
//! panicked (`.expect(...)`) or silently shipped a `QueryValue::Null` sentinel
//! when a required field was never set.
//!
//! These builders' [`build`](crate::write) methods now return
//! `Result<_, BuilderError>` instead of panicking, so a malformed op surfaces
//! as a typed error rather than an abort. The name is deliberately distinct
//! from [`crate::batch::BuildError`] (batch-DAG validation) and
//! [`crate::query::QueryBuildError`] (read-query validation) — three separate
//! error families for three separate builder domains, matching this crate's
//! existing precedent of not overloading one enum across unrelated builder
//! kinds.

/// Error returned when a write/DDL op builder is missing a required field.
///
/// Produced by the `build()` methods of [`crate::write::Delete`],
/// [`crate::write::Update`], [`crate::write::Upsert`],
/// [`crate::ddl::AddSchemaRuleBuilder`] and [`crate::ddl::AlterSubscriptionBuilder`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum BuilderError {
    /// [`crate::write::Delete::build`] was called without [`crate::write::Delete::where_`].
    ///
    /// The `DeleteOp` wire type requires a filter — omitting it would delete
    /// every row in the table, so the builder rejects an unset WHERE clause.
    #[error("Delete::build() requires a where clause — call .where_(filter) before .build()")]
    MissingWhereClause,
    /// [`crate::write::Update::build`] was called without [`crate::write::Update::set`].
    ///
    /// An `UpdateOp` with no `set` payload is a no-op at best and a mistake at
    /// worst; the builder refuses to ship one. Note a *deliberate*
    /// `.set(QueryValue::Null)` still builds successfully — this only fires
    /// when `.set()` was never called.
    #[error("Update::build() requires a set payload — call .set(doc) before .build()")]
    MissingSetValue,
    /// [`crate::write::Upsert::build`] was called without [`crate::write::Upsert::key`].
    #[error("Upsert::build() requires a key — call .key(doc) before .build()")]
    MissingKey,
    /// [`crate::write::Upsert::build`] was called without [`crate::write::Upsert::value`].
    #[error("Upsert::build() requires a value — call .value(doc) before .build()")]
    MissingValue,
    /// [`crate::ddl::AddSchemaRuleBuilder::build`] was called without
    /// [`crate::ddl::AddSchemaRuleBuilder::rule`].
    #[error("AddSchemaRuleBuilder::build() requires a rule — call .rule(r) before .build()")]
    MissingRule,
    /// [`crate::ddl::AlterSubscriptionBuilder::build`] was called with no
    /// terminal action (`.pause()` / `.resume()` / `.set_profile()`).
    #[error("AlterSubscriptionBuilder::build() requires a terminal action — call .pause() / .resume() / .set_profile(_) before .build()")]
    MissingAction,
    /// [`crate::write::Doc::set`] could not convert the supplied
    /// [`FilterValue`](shamir_query_types::filter::FilterValue) into a
    /// `QueryValue` — nesting beyond `MAX_FILTER_DEPTH`, or a codec failure
    /// in the expression-marker msgpack round-trip fallback
    /// (RV-QUERY-BUILDER-20D9732A).
    #[error("Doc::set() could not convert the field value to a QueryValue: {0}")]
    FieldValueCodec(String),
    /// An HMAC-gated builder's `build()` was called without `.hmac(tag)`
    /// (or the conditional gate — definer security, replace, secret grants —
    /// fired without one). The server rejects such ops with `hmac_required`;
    /// the builder now fails closed client-side (RV-QUERY-BUILDER-E6F53E38).
    #[error("this op is HMAC-gated — call .hmac(tag) before .build()")]
    MissingHmac,
}
