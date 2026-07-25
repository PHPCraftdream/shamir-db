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
#[derive(Debug, Clone, PartialEq)]
pub enum BuilderError {
    /// [`crate::write::Delete::build`] was called without [`crate::write::Delete::where_`].
    ///
    /// The `DeleteOp` wire type requires a filter — omitting it would delete
    /// every row in the table, so the builder rejects an unset WHERE clause.
    MissingWhereClause,
    /// [`crate::write::Update::build`] was called without [`crate::write::Update::set`].
    ///
    /// An `UpdateOp` with no `set` payload is a no-op at best and a mistake at
    /// worst; the builder refuses to ship one. Note a *deliberate*
    /// `.set(QueryValue::Null)` still builds successfully — this only fires
    /// when `.set()` was never called.
    MissingSetValue,
    /// [`crate::write::Upsert::build`] was called without [`crate::write::Upsert::key`].
    MissingKey,
    /// [`crate::write::Upsert::build`] was called without [`crate::write::Upsert::value`].
    MissingValue,
    /// [`crate::ddl::AddSchemaRuleBuilder::build`] was called without
    /// [`crate::ddl::AddSchemaRuleBuilder::rule`].
    MissingRule,
    /// [`crate::ddl::AlterSubscriptionBuilder::build`] was called with no
    /// terminal action (`.pause()` / `.resume()` / `.set_profile()`).
    MissingAction,
}

impl std::fmt::Display for BuilderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuilderError::MissingWhereClause => write!(
                f,
                "Delete::build() requires a where clause — call .where_(filter) before .build()"
            ),
            BuilderError::MissingSetValue => write!(
                f,
                "Update::build() requires a set payload — call .set(doc) before .build()"
            ),
            BuilderError::MissingKey => {
                write!(
                    f,
                    "Upsert::build() requires a key — call .key(doc) before .build()"
                )
            }
            BuilderError::MissingValue => write!(
                f,
                "Upsert::build() requires a value — call .value(doc) before .build()"
            ),
            BuilderError::MissingRule => write!(
                f,
                "AddSchemaRuleBuilder::build() requires a rule — call .rule(r) before .build()"
            ),
            BuilderError::MissingAction => write!(
                f,
                "AlterSubscriptionBuilder::build() requires a terminal action — call .pause() / \
                 .resume() / .set_profile(_) before .build()"
            ),
        }
    }
}

impl std::error::Error for BuilderError {}
