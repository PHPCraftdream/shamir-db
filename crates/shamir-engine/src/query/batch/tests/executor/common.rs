//! Shared test helpers for batch executor tests.

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::TableResolver;
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::{TableConfig, TableManager};
use shamir_funclib::registry::{FnEntry, ScalarResult};
use shamir_funclib::scalar_resolver::{ScalarResolver, UserScalarLayer};
use shamir_storage::error::DbResult;
use shamir_types::types::value::QueryValue;

/// Simple resolver that wraps a DbInstance + repo name.
pub(super) struct TestResolver {
    pub(super) db: DbInstance,
    pub(super) repo: String,
}

#[async_trait::async_trait]
impl TableResolver for TestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table(&self.repo, &table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<crate::repo::RepoInstance> {
        // F4b-1: non-tx writes now route through an implicit Snapshot tx, so the
        // executor calls `resolve_repo` even on the non-transactional path. Back
        // it with the real in-memory repo so insert tests exercise the implicit
        // commit pipeline end-to-end.
        self.db.get_repo(&self.repo).ok_or_else(|| {
            shamir_storage::error::DbError::NotFound(format!("repo '{}' not found", self.repo))
        })
    }
}

pub(super) async fn setup_resolver() -> TestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users"), TableConfig::new("orders")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    TestResolver {
        db,
        repo: "default".to_string(),
    }
}

pub(super) struct TxTestResolver {
    pub(super) repo: crate::repo::RepoInstance,
}

#[async_trait::async_trait]
impl TableResolver for TxTestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.repo.get_table(&table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<crate::repo::RepoInstance> {
        Ok(self.repo.clone())
    }
}

// ============================================================================
// Resolver variant with a user-registered scalar — for Fix 2 regression tests.
// ============================================================================

/// Build a `ScalarResolver` backed by a `UserScalarLayer` with one registered
/// scalar `my_double` that doubles its `Int` argument. Mirrors the
/// `resolver_with_user_scalar()` helper in
/// `crates/shamir-engine/src/query/read/tests/exec_tests.rs`.
pub(super) fn resolver_with_user_scalar() -> ScalarResolver {
    let layer = UserScalarLayer::new();
    layer.register(
        "my_double",
        FnEntry::pure(
            |args: &[QueryValue]| -> ScalarResult {
                match &args[0] {
                    QueryValue::Int(n) => Ok(QueryValue::Int(n * 2)),
                    _ => Err(shamir_funclib::registry::ScalarError::new("type_mismatch")),
                }
            },
            1,
            Some(1),
        ),
    );
    ScalarResolver::new(std::sync::Arc::new(layer))
}

/// `TestResolver` variant whose `scalar_resolver()` override returns a resolver
/// with the `my_double` user scalar registered. Used by Fix 2 regression tests
/// for `when`/`bind`/`over` — the default `TableResolver::scalar_resolver()`
/// returns builtins-only, so without this override the user scalar would be
/// invisible and the test would observe the pre-fix (broken) behavior.
pub(super) struct TestResolverWithScalars {
    pub(super) db: DbInstance,
    pub(super) repo: String,
    pub(super) scalars: ScalarResolver,
}

#[async_trait::async_trait]
impl TableResolver for TestResolverWithScalars {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table(&self.repo, &table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<crate::repo::RepoInstance> {
        self.db.get_repo(&self.repo).ok_or_else(|| {
            shamir_storage::error::DbError::NotFound(format!("repo '{}' not found", self.repo))
        })
    }

    fn scalar_resolver(&self) -> ScalarResolver {
        self.scalars.clone()
    }
}

pub(super) async fn setup_resolver_with_scalars() -> TestResolverWithScalars {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users"), TableConfig::new("orders")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    TestResolverWithScalars {
        db,
        repo: "default".to_string(),
        scalars: resolver_with_user_scalar(),
    }
}
