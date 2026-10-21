use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_query_types::admin::FkAction;
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use smallvec::smallvec;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::execute_batch_unchecked as execute_batch;
use crate::query::batch::TableResolver;
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::{TableConfig, TableManager};
use crate::validator::schema::constraints::Constraints;
use crate::validator::schema::field_rule::FieldRule;
use crate::validator::schema::foreign_key::ForeignKeyRef;
use crate::validator::schema::schema_validator::SchemaValidator;
use crate::validator::schema::type_tag::TypeTag;
use crate::validator::{ValidatorBinding, ValidatorRegistry, WriteOp};

/// Current row count for `table`.
pub(super) async fn row_count(table: &TableManager) -> u64 {
    table.counter().get().await.unwrap()
}

/// The `resolve_repo()` call ordinal (1-based) for the EXPLICIT-tx DELETE arm.
///
/// The explicit `Some(tx)` sub-arm of `BatchOp::Delete` (`query_runner.rs:1519`)
/// does NOT call the implicit-only isolation-upgrade hook, so its `resolve_repo`
/// sequence is:
/// 1. `check_fk_restrict` → `discover_restrict_refs` → `resolve_repo` (the
///    RESTRICT child scan runs after this returns, recording the RI barrier
///    token).
/// 2. `plan_cascade` → `discover_action_refs` → `resolve_repo` — AFTER the
///    RESTRICT scan fully returned, BEFORE `apply_cascade_plan` / commit.
///
/// So ordinal 2 is exactly the after-scan / before-commit window: the RI
/// barrier token is already recorded (from the scan at step 1), and the delete
/// has not yet staged or committed.
const INJECT_AT_RESOLVE_REPO_CALL: usize = 2;

/// Resolver that wraps a real `RepoInstance`-backed `DbInstance`, injects a
/// shared `ValidatorRegistry`, and fires a caller-supplied concurrent writer
/// batch to FULL commitment on one specific `resolve_repo()` call ordinal.
/// Identical shape to `fk_race_closure_tests.rs::RaceInjectingResolver`.
pub(super) struct RaceInjectingResolver {
    db: DbInstance,
    repo: String,
    registry: Arc<ValidatorRegistry>,
    resolve_repo_calls: AtomicUsize,
    pub(super) inject_at: usize,
    pub(super) writer: tokio::sync::Mutex<Option<InjectedWriter>>,
}

impl RaceInjectingResolver {
    pub(super) fn reset_counter(&self) {
        self.resolve_repo_calls.store(0, Ordering::SeqCst);
    }
}

pub(super) struct InjectedWriter {
    pub(super) req: shamir_query_types::batch::BatchRequest,
    pub(super) resolver: TxTestResolver,
}

/// Minimal resolver for the injected concurrent writer — resolves against the
/// SAME live repo (so its commit is really visible to the outer tx) but keeps
/// its own independent (uncounted) call stream.
pub(super) struct TxTestResolver {
    pub(super) repo: crate::repo::RepoInstance,
}

#[async_trait::async_trait]
impl TableResolver for TxTestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> shamir_storage::error::DbResult<TableManager> {
        self.repo.get_table(&table_ref.table).await
    }

    async fn resolve_repo(
        &self,
        _repo_name: &str,
    ) -> shamir_storage::error::DbResult<crate::repo::RepoInstance> {
        Ok(self.repo.clone())
    }
}

#[async_trait::async_trait]
impl TableResolver for RaceInjectingResolver {
    async fn resolve(&self, table_ref: &TableRef) -> shamir_storage::error::DbResult<TableManager> {
        let mut table = self.db.get_table(&self.repo, &table_ref.table).await?;
        table.set_validator_registry(Arc::clone(&self.registry));
        Ok(table)
    }

    async fn resolve_repo(
        &self,
        _repo_name: &str,
    ) -> shamir_storage::error::DbResult<crate::repo::RepoInstance> {
        let n = self.resolve_repo_calls.fetch_add(1, Ordering::SeqCst) + 1;
        if n == self.inject_at {
            let taken = self.writer.lock().await.take();
            if let Some(InjectedWriter { req, resolver }) = taken {
                let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
                    .await
                    .expect("injected concurrent writer batch executes");
                assert!(
                    !resp.results.is_empty(),
                    "injected writer batch must have run and produced a result"
                );
            }
        }
        self.db.get_repo(&self.repo).ok_or_else(|| {
            shamir_storage::error::DbError::NotFound(format!("repo '{}' not found", self.repo))
        })
    }
}

/// Build a parent/child test environment with a bound FK (`on_delete` per
/// `action`), mirroring `fk_race_closure_tests.rs::setup_race_test` exactly.
pub(super) async fn setup_race_test(
    action: FkAction,
) -> (RaceInjectingResolver, crate::repo::RepoInstance) {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let repo = db.get_repo("default").unwrap();

    let registry = Arc::new(ValidatorRegistry::new());
    let child_schema = SchemaValidator::new(vec![FieldRule {
        path: vec!["parent_id".to_string()],
        ty: TypeTag::Int,
        constraints: Constraints {
            foreign_key: Some(ForeignKeyRef::with_on_delete("parent", "id", action)),
            required: true,
            ..Default::default()
        },
        keyset_safe: false,
    }]);
    let validator_id = RecordId::from_ts(9101);
    registry
        .register(validator_id, "race_child_fk_schema", Arc::new(child_schema))
        .unwrap();
    let binding = ValidatorBinding {
        validator_id,
        ops: smallvec![WriteOp::Delete],
        priority: 1000,
    };
    let mut child_table = db.get_table("default", "child").await.unwrap();
    child_table.set_validator_registry(Arc::clone(&registry));
    child_table.add_validator_binding(binding).await.unwrap();

    let resolver = RaceInjectingResolver {
        db,
        repo: "default".to_string(),
        registry,
        resolve_repo_calls: AtomicUsize::new(0),
        inject_at: INJECT_AT_RESOLVE_REPO_CALL,
        writer: tokio::sync::Mutex::new(None),
    };
    (resolver, repo)
}

// ============================================================================
// 1. End-to-end race closure — EXPLICIT Snapshot parent DELETE (RESTRICT).
//
// A genuinely concurrent writer inserts a NEW child reference between the
// RESTRICT scan (which found no reference and recorded the RI barrier token)
// and the explicit tx's commit. The invariant under test: NEVER "delete
// committed AND a dangling child reference exists" — the RI barrier must abort
// the parent's commit with PhantomConflict (surfaced as "tx_conflict").
// ============================================================================

/// Build a parent/child test environment with a bound FK whose `on_update`
/// action is `action` (on_delete stays `NoAction`), mirroring
/// `fk_race_closure_tests.rs::setup_race_test_on_update`. The child field is
/// nullable so `on_update = SetNull` is exercisable.
pub(super) async fn setup_race_test_on_update(
    action: FkAction,
) -> (RaceInjectingResolver, crate::repo::RepoInstance) {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let repo = db.get_repo("default").unwrap();

    let registry = Arc::new(ValidatorRegistry::new());
    let child_schema = SchemaValidator::new(vec![FieldRule {
        path: vec!["parent_id".to_string()],
        ty: TypeTag::Int,
        constraints: Constraints {
            foreign_key: Some(ForeignKeyRef::with_on_update("parent", "id", action)),
            nullable: true,
            ..Default::default()
        },
        keyset_safe: false,
    }]);
    let validator_id = RecordId::from_ts(9101);
    registry
        .register(validator_id, "race_child_fk_schema", Arc::new(child_schema))
        .unwrap();
    let binding = ValidatorBinding {
        validator_id,
        ops: smallvec![WriteOp::Delete],
        priority: 1000,
    };
    let mut child_table = db.get_table("default", "child").await.unwrap();
    child_table.set_validator_registry(Arc::clone(&registry));
    child_table.add_validator_binding(binding).await.unwrap();

    let resolver = RaceInjectingResolver {
        db,
        repo: "default".to_string(),
        registry,
        resolve_repo_calls: AtomicUsize::new(0),
        inject_at: INJECT_AT_RESOLVE_REPO_CALL,
        writer: tokio::sync::Mutex::new(None),
    };
    (resolver, repo)
}
