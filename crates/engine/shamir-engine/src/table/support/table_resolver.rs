//! Table-scoped resolver contract: table references to `TableManager`
//! instances, shared by the write paths, the validator FK probe and the
//! batch executors (re-exported through `executor_traits`).

use shamir_funclib::scalar_resolver::ScalarResolver;
use shamir_query_types::TableRef;
use shamir_storage::error::DbResult;

use crate::table::TableManager;

/// Trait for resolving table references to TableManager instances.
#[async_trait::async_trait]
pub trait TableResolver: Send + Sync {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager>;

    /// Resolve a table reference bypassing the rename-in-doubt name fence.
    ///
    /// Only for read-only declaration walks (FK reverse-cache scan): the
    /// data of a frozen in-doubt source is stable and its FK declarations
    /// cannot change, so the walk must not fail on the fenced source name.
    /// Default: the ordinary fenced [`TableResolver::resolve`].
    async fn resolve_unfenced(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.resolve(table_ref).await
    }

    /// Return the per-DB scalar resolver (user + builtin layers).
    ///
    /// Default: built-ins only (no user-registered scalars). Overridden
    /// by `DbTableResolver` when user scalars are registered on the
    /// database instance.
    fn scalar_resolver(&self) -> ScalarResolver {
        ScalarResolver::builtins_only()
    }
}
