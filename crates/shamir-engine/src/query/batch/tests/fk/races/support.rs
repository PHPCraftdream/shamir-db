use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
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

/// Current row count for `table`, via its persisted `RecordCounter` — the
/// same counter every insert/delete already maintains, so this needs no
/// extra query machinery.
pub(super) async fn row_count(table: &TableManager) -> u64 {
    table.counter().get().await.unwrap()
}

/// The `resolve_repo()` call ordinal (1-based), counted from the moment the
/// DELETE op begins (see [`RaceInjectingResolver::reset_counter`] — tests
/// reset the counter right before issuing the delete, so this ordinal is
/// stable regardless of how many `resolve_repo` calls any earlier seed
/// inserts made), at which `discover_action_refs`
/// (`fk_actions.rs::plan_cascade`) fires.
///
/// Per the delete arm's call sequence (`query_runner.rs`'s `BatchOp::Delete`
/// `None =>` arm):
///
/// 1. the arm's own `resolve_repo` (resolves `repo` up front),
/// 2. `implicit_tx_isolation_for_fk_parent`'s `resolve_repo`,
/// 3. `check_fk_restrict` → `discover_restrict_refs`'s `resolve_repo` —
///    this call precedes `check_fk_restrict`'s OWN row-level scan
///    (`child_has_reference`'s `list_stream_tx`), which runs with NO
///    further `resolve_repo` call in between,
/// 4. `plan_cascade` → `discover_action_refs`'s `resolve_repo` — this
///    only runs AFTER `check_fk_restrict` (including its scan) has
///    FULLY returned.
///
/// So ordinal 4 is exactly the after-scan, before-commit window: the SSI
/// `TableScan` predicate is already recorded (from call #3's
/// `check_fk_restrict`), and the delete has not yet staged or committed.
///
/// F-35 (on_update path): the implicit UPDATE arm has the SAME ordinal-4
/// shape — its `resolve_repo` sequence is (1) the arm's own, (2)
/// `implicit_tx_isolation_for_fk_parent`'s, (3)
/// `require_footprint_if_fk_child`'s (the UPDATE arm calls it inside the
/// retry closure, before `plan_fk_on_update`), (4) `plan_fk_on_update` →
/// `discover_on_update_refs`'s. So the same `4` lands the writer at the
/// UPDATE arm's FK-discovery `resolve_repo`, which is AFTER the implicit
/// tx has begun (so the writer's commit lands inside the parent's open
/// Serializable window) and BEFORE `plan_fk_on_update`'s own
/// `list_stream_tx` child scans record their predicate — the SSI conflict
/// is then detected at the parent's commit (the parent's snapshot predates
/// the writer's commit), exactly the race F-35 closes for `on_update`.
const INJECT_AT_RESOLVE_REPO_CALL: usize = 4;

/// Resolver that wraps a real `RepoInstance`-backed `DbInstance`, injects a
/// shared `ValidatorRegistry` (so FK metadata is visible to
/// `collect_fk_refs()`), and fires a caller-supplied concurrent writer batch
/// to FULL commitment on one specific `resolve_repo()` call ordinal.
pub(super) struct RaceInjectingResolver {
    db: DbInstance,
    repo: String,
    registry: Arc<ValidatorRegistry>,
    resolve_repo_calls: AtomicUsize,
    pub(super) inject_at: usize,
    /// The concurrent writer batch to run when `inject_at` is reached.
    /// `Mutex` only so the closure captured inside `resolve_repo` (an
    /// `&self` method) can take it out exactly once (idempotent — a second
    /// hit past `inject_at`, e.g. from a retry, is a no-op).
    pub(super) writer: tokio::sync::Mutex<Option<InjectedWriter>>,
}

impl RaceInjectingResolver {
    /// Zero the call counter. Tests call this immediately BEFORE issuing the
    /// delete op under test, so `inject_at`'s ordinal counts only
    /// `resolve_repo` calls made by the DELETE itself — independent of how
    /// many calls any earlier seed-data inserts made.
    pub(super) fn reset_counter(&self) {
        self.resolve_repo_calls.store(0, Ordering::SeqCst);
    }
}

pub(super) struct InjectedWriter {
    pub(super) req: shamir_query_types::batch::BatchRequest,
    /// Independent resolver for the writer batch — see module doc: the
    /// writer's own `resolve_repo`/`resolve` calls must not perturb the
    /// outer delete's call-counting.
    pub(super) resolver: TxTestResolver,
}

/// Minimal resolver for the injected concurrent writer — resolves against
/// the SAME live repo (so its commit is really visible to the outer tx) but
/// keeps its own independent (uncounted) call stream.
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
            // Take the writer exactly once — idempotent under retry.
            let taken = self.writer.lock().await.take();
            if let Some(InjectedWriter { req, resolver }) = taken {
                let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
                    .await
                    .expect("injected concurrent writer batch executes");
                // The writer is a plain autocommit insert — no `transaction`
                // block (that field is only populated for `.transactional()`
                // batches), so success is simply `Ok`. Assert the insert
                // actually happened (non-empty affected/records).
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
/// `action`) — mirrors `fk_restrict_tests.rs::setup_fk_test` exactly, plus
/// the race-injection scaffolding.
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
// 1. End-to-end race closure — RESTRICT.
//
// A genuinely concurrent writer inserts a NEW child reference between the
// RESTRICT gate's scan (which found no reference and recorded the SSI
// predicate) and the parent delete's commit. The invariant under test:
// NEVER "delete succeeds AND a dangling/orphaned reference exists after
// both commit" — either the delete aborts with a coded conflict (the child
// row now correctly outlives an existing parent... but since the delete
// itself never observed it, it must self-abort) or it correctly sees the
// new reference and rejects with fk_restrict. Both are acceptable; only the
// silent-corruption outcome is not.
// ============================================================================

/// Build a parent/child test environment with a bound FK shaped
/// `on_delete = NoAction, on_update = action` — the F-35 shape that used to
/// slip past the UPDATE-path isolation upgrade. Mirrors `setup_race_test`
/// exactly except:
/// - the FK is declared via `ForeignKeyRef::with_on_update` (so `on_delete`
///   stays `NoAction`, isolating the UPDATE-path role flag from the
///   DELETE-path one), and
/// - the child field is `nullable` so `on_update = SetNull` is exercisable
///   (SET NULL rejects a non-nullable field at plan time, before any scan).
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

/// Read the first row's `field` as `i64` from `table` via a read query
/// through the race resolver. By the time tests call this the injected
/// writer has already been consumed, so the resolver's `resolve_repo` hook
/// is a pure no-op observer. Needed because an UPDATE CASCADE re-keys
/// (doesn't delete) the child, so `row_count` alone cannot distinguish a
/// correctly cascaded child from an orphaned one.
pub(super) async fn read_first_i64(
    resolver: &RaceInjectingResolver,
    table: &str,
    field: &str,
) -> Option<i64> {
    let mut b = Batch::new();
    b.id(9998);
    b.query("q", shamir_query_builder::Query::from(table));
    let req = b.build();
    let resp = execute_batch(&req, resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    resp.results["q"]
        .records
        .first()
        .and_then(|r| r.get_value_i64(field))
}

/// Seed the single parent row `{id, name:"Alice"}` (plain autocommit insert).
pub(super) async fn seed_parent(resolver: &RaceInjectingResolver, id: i64) {
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", id).set("name", "Alice")),
    );
    execute_batch(&b.build(), resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
}

/// Arm the injected concurrent writer: an autocommit insert of a NEW child
/// row referencing `parent_id`, fired at the after-begin/before-commit seam
/// (see `INJECT_AT_RESOLVE_REPO_CALL`).
pub(super) async fn arm_child_writer(
    resolver: &RaceInjectingResolver,
    repo: &crate::repo::RepoInstance,
    parent_id: i64,
) {
    let mut wb = Batch::new();
    wb.id("writer");
    wb.insert(
        "ins_child_race",
        write::insert("child").row(doc().set("parent_id", parent_id).set("label", "race")),
    );
    *resolver.writer.lock().await = Some(InjectedWriter {
        req: wb.build(),
        resolver: TxTestResolver { repo: repo.clone() },
    });
    resolver.reset_counter();
}

/// Issue the implicit UPDATE that re-keys the parent's referenced field
/// (`id`) from `from` to `to`, returning the batch response.
pub(super) async fn rekey_parent(
    resolver: &RaceInjectingResolver,
    from: i64,
    to: i64,
) -> Result<crate::query::batch::BatchResponse, crate::query::batch::BatchError> {
    let mut b = Batch::new();
    b.id(2);
    b.try_update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", from))
            .set(doc().set("id", to)),
    )
    .unwrap();
    execute_batch(&b.build(), resolver, None, None, Actor::System, "test").await
}

/// Assert the cache role-flags for an `on_delete = NoAction, on_update != NoAction`
/// FK: the UPDATE-path flag MUST be set (the F-35 Serializable-upgrade decision
/// for the implicit UPDATE arm), and the DELETE-path flag must NOT be (since
/// `on_delete` is `NoAction`).
///
/// This is the AUTHORITATIVE proof the UPDATE arm now upgrades to Serializable.
/// The on_update race tests inject the writer at the UPDATE arm's only
/// `resolve_repo` ordinal (4 = `discover_on_update_refs`), which lands BEFORE
/// `plan_fk_on_update`'s `list_stream_tx` child scan — and that scan reads
/// committed-at-now, so it observes the raced-in child directly (at either
/// isolation). The Serializable upgrade's commit-time conflict therefore isn't
/// the mechanism that catches this particular race shape; the role-flag is what
/// proves the upgrade was wired for the UPDATE path.
pub(super) fn assert_on_update_parent_flagged(repo: &crate::repo::RepoInstance) {
    let cache = repo.fk_reverse_cache();
    assert!(
        cache.is_fk_parent_with_update_action("parent"),
        "on_update != NoAction must flag the parent for the UPDATE-path Serializable upgrade"
    );
    assert!(
        !cache.is_fk_parent_with_delete_action("parent"),
        "on_delete=NoAction must NOT flag the parent for the DELETE-path upgrade"
    );
}
