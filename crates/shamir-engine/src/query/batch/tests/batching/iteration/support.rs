use shamir_funclib::registry::{FnEntry, ScalarResult};
use shamir_funclib::scalar_resolver::{ScalarResolver, UserScalarLayer};
use shamir_query_types::batch::{
    BatchLimits, BatchOp, BatchRequest, ForEachOp, QueryEntry, ResultEncoding,
};
use shamir_query_types::write::InsertOp;
use shamir_storage::error::DbResult;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::TableResolver;
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{RepoConfig, RepoInstance};
use crate::table::{TableConfig, TableManager};

// ============================================================================
// Shared infrastructure
// ============================================================================

pub(super) struct TestResolver {
    pub(super) db: DbInstance,
}

#[async_trait::async_trait]
impl TableResolver for TestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table("default", &table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        self.db.get_repo("default").ok_or_else(|| {
            shamir_storage::error::DbError::NotFound("repo 'default' not found".into())
        })
    }
}

pub(super) async fn setup() -> TestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users"), TableConfig::new("orders")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    TestResolver { db }
}

pub(super) struct TxTestResolver {
    pub(super) repo: RepoInstance,
}

#[async_trait::async_trait]
impl TableResolver for TxTestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.repo.get_table(&table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        Ok(self.repo.clone())
    }
}

// ============================================================================
// Fix 2 — resolver variant with a user-registered scalar for over tests.
// ============================================================================

/// Build a ScalarResolver with a user-registered scalar `my_double`.
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
    // A second scalar that returns a singleton List — used by the
    // `for_each` `over` test, where `over` must resolve to `QueryValue::List`.
    layer.register(
        "make_singleton_list",
        FnEntry::pure(
            |args: &[QueryValue]| -> ScalarResult {
                match &args[0] {
                    QueryValue::Int(n) => Ok(QueryValue::List(vec![QueryValue::Int(*n)])),
                    _ => Err(shamir_funclib::registry::ScalarError::new("type_mismatch")),
                }
            },
            1,
            Some(1),
        ),
    );
    ScalarResolver::new(std::sync::Arc::new(layer))
}

pub(super) struct TestResolverWithScalars {
    pub(super) db: DbInstance,
    scalars: ScalarResolver,
}

#[async_trait::async_trait]
impl TableResolver for TestResolverWithScalars {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        self.db.get_table("default", &table_ref.table).await
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        self.db.get_repo("default").ok_or_else(|| {
            shamir_storage::error::DbError::NotFound("repo 'default' not found".into())
        })
    }

    fn scalar_resolver(&self) -> ScalarResolver {
        self.scalars.clone()
    }
}

pub(super) async fn setup_with_scalars() -> TestResolverWithScalars {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users"), TableConfig::new("orders")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    TestResolverWithScalars {
        db,
        scalars: resolver_with_user_scalar(),
    }
}

/// Build a `for_each` loop body that inserts one order row per iteration,
/// with `user_id` sourced from the bound row element (`$param bind_row`).
pub(super) fn insert_order_body(bind_row: &str) -> BatchRequest {
    // `mpack!` needs its keys/values as literal tokens — `bind_row` is a
    // runtime `&str` parameter (the $param NAME, which varies per test), so
    // the nested `{"$param": bind_row}` object is built directly rather
    // than through the macro.
    let mut param_obj = new_map();
    param_obj.insert("$param".to_string(), QueryValue::Str(bind_row.to_string()));
    let mut insert_value = new_map();
    insert_value.insert("user_id".to_string(), QueryValue::Map(param_obj));
    insert_value.insert("note".to_string(), QueryValue::Str("fe".to_string()));

    let mut queries = new_map();
    queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("orders"),
                values: vec![QueryValue::Map(insert_value)],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    BatchRequest {
        id: QueryValue::Int(100),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    }
}

/// Build a `for_each` loop body like [`insert_order_body`], but with an
/// EXTRA `guard` field computed via `$fn: math/mod(1, $param <bind_row>)`
/// (`crates/shamir-funclib/src/math.rs`'s `"mod"` entry, dispatch name
/// `"math/mod"`, registered `FnEntry::pure`). `math/mod` returns
/// `Err(ScalarError::new("div_by_zero"))` when its divisor argument is
/// zero — `resolve_filter_query`'s `FnCall` arm collapses any scalar error
/// to `None` (`crates/shamir-engine/src/query/filter/resolve.rs`), and
/// `resolve_write_value`'s marker resolution turns that `None` into a hard
/// `WriteValueError::MalformedMarker` (`crates/shamir-engine/src/query/batch/
/// param_subst.rs`), which surfaces as a genuine `Err` out of the Insert
/// dispatch — NOT a silent skip.
///
/// This gives a PURE function of the current iteration's own `bind_row`
/// value (no cross-iteration state observation needed, unlike a unique
/// index — see this module's `#661` test section below for why a
/// unique-index-based mid-tx failure does NOT work across iterations of
/// the SAME uncommitted transaction: unique-key validation only checks
/// against DURABLE committed state, both at insert-time
/// (`validate_unique_for_create`) and at commit-time re-validation
/// (`tx::pre_commit::pre_commit_prelock`'s per-guard check against
/// `info_store`) — two inserts claiming the same key within ONE
/// still-uncommitted tx never cross-check each other and both silently
/// pass).
///
/// NOTE: The intra-tx unique-key gap described above was FIXED by #1039.
/// Phase 2.6 now includes an O(1)-amortized dedup check using TFxMap
/// keyed by (table_token, index_key) that cross-validates guards within
/// the same transaction. A bind_row value of `0` makes THIS iteration's
/// own insert fail, independent of what any other iteration did.
pub(super) fn insert_order_body_with_div_guard(bind_row: &str) -> BatchRequest {
    let mut param_obj = new_map();
    param_obj.insert("$param".to_string(), QueryValue::Str(bind_row.to_string()));
    let mut fn_args = new_map();
    fn_args.insert("name".to_string(), QueryValue::Str("math/mod".to_string()));
    fn_args.insert(
        "args".to_string(),
        QueryValue::List(vec![QueryValue::Int(1), QueryValue::Map(param_obj.clone())]),
    );
    let mut fn_marker = new_map();
    fn_marker.insert("$fn".to_string(), QueryValue::Map(fn_args));

    let mut insert_value = new_map();
    insert_value.insert("user_id".to_string(), QueryValue::Map(param_obj));
    insert_value.insert("note".to_string(), QueryValue::Str("fe".to_string()));
    insert_value.insert("guard".to_string(), QueryValue::Map(fn_marker));

    let mut queries = new_map();
    queries.insert(
        "ins".to_string(),
        QueryEntry {
            op: BatchOp::Insert(InsertOp {
                insert_into: TableRef::new("orders"),
                values: vec![QueryValue::Map(insert_value)],
                records_idmsgpack: Vec::new(),
                select: None,
            }),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    BatchRequest {
        id: QueryValue::Int(101),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    }
}

pub(super) fn wrap_for_each(fe: ForEachOp) -> BatchRequest {
    let mut queries = new_map();
    queries.insert(
        "loop".to_string(),
        QueryEntry {
            op: BatchOp::ForEach(fe),
            return_result: true,
            after: Vec::new(),
            when: None,
        },
    );
    BatchRequest {
        id: QueryValue::Int(1),
        name: None,
        transactional: false,
        isolation: None,
        visibility_wait: None,
        durability: None,
        index_visibility: None,
        queries,
        return_all: true,
        return_only: None,
        limits: BatchLimits::default(),
        interner_epochs: Default::default(),
        result_encoding: ResultEncoding::default(),
    }
}
