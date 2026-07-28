//! F-40b Step 1 (#855) — RI barrier design spike: deterministic race harness
//! for the EXPLICIT `Snapshot`-isolation FK-parent DELETE path.
//!
//! Mirrors `fk_race_closure_tests.rs`'s `RaceInjectingResolver` shape (the
//! exact same `resolve_repo`-call-ordinal deterministic injection seam), but
//! with an EXPLICIT `Snapshot` transaction as the outer operation instead of
//! an implicit one. The injection ordinal shifts from 4 (implicit path: arm /
//! isolation-hook / restrict-discover / cascade-discover) to 2 (explicit path:
//! restrict-discover / cascade-discover), because the explicit-tx DELETE arm
//! does NOT call the implicit-only isolation-upgrade hook.
//!
//! ## The mechanism under test
//!
//! `fk_restrict.rs::child_has_reference` now records the child `table_token`
//! into `TxContext.ri_barrier_tokens` REGARDLESS of isolation (not gated on
//! `Serializable` like the existing `predicate_set` recording). At commit,
//! `pre_commit.rs` Phase 2-bis (widened) runs `predicate_conflicts_batch` over
//! those tokens, so a concurrent committer that touched the child table in the
//! commit window aborts the parent — closing the cross-transaction FK TOCTOU
//! race that F-28 Step 5 closed for the IMPLICIT path only.

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
use crate::query::batch::TableResolver;
use crate::query::batch::{
    commit_interactive_tx, execute_batch, execute_in_open_tx, open_interactive_tx,
};
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::{TableConfig, TableManager};
use crate::tx::CommitError;
use crate::validator::schema::constraints::Constraints;
use crate::validator::schema::field_rule::FieldRule;
use crate::validator::schema::foreign_key::ForeignKeyRef;
use crate::validator::schema::schema_validator::SchemaValidator;
use crate::validator::schema::type_tag::TypeTag;
use crate::validator::{ValidatorBinding, ValidatorRegistry, WriteOp};

/// Current row count for `table`.
async fn row_count(table: &TableManager) -> u64 {
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
struct RaceInjectingResolver {
    db: DbInstance,
    repo: String,
    registry: Arc<ValidatorRegistry>,
    resolve_repo_calls: AtomicUsize,
    inject_at: usize,
    writer: tokio::sync::Mutex<Option<InjectedWriter>>,
}

impl RaceInjectingResolver {
    fn reset_counter(&self) {
        self.resolve_repo_calls.store(0, Ordering::SeqCst);
    }
}

struct InjectedWriter {
    req: shamir_query_types::batch::BatchRequest,
    resolver: TxTestResolver,
}

/// Minimal resolver for the injected concurrent writer — resolves against the
/// SAME live repo (so its commit is really visible to the outer tx) but keeps
/// its own independent (uncounted) call stream.
struct TxTestResolver {
    repo: crate::repo::RepoInstance,
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
async fn setup_race_test(action: FkAction) -> (RaceInjectingResolver, crate::repo::RepoInstance) {
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

#[tokio::test]
async fn explicit_snapshot_restrict_race_closed_via_ri_barrier() {
    let (resolver, repo) = setup_race_test(FkAction::Restrict).await;

    // Seed the parent row.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Arm the injected writer: a concurrent autocommit insert of a NEW child
    // row referencing the SAME parent, fired at the exact after-scan /
    // before-commit seam (resolve_repo ordinal 2).
    let mut wb = Batch::new();
    wb.id("writer");
    wb.insert(
        "ins_child_race",
        write::insert("child").row(doc().set("parent_id", 1).set("label", "race")),
    );
    *resolver.writer.lock().await = Some(InjectedWriter {
        req: wb.build(),
        resolver: TxTestResolver { repo: repo.clone() },
    });

    // Open an EXPLICIT Snapshot transaction. This is the gap F-40 describes:
    // the client-chosen isolation is fixed here, BEFORE query_runner sees any
    // op, so the implicit-only Serializable upgrade never fires.
    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    resolver.reset_counter();

    // Execute the DELETE inside the open Snapshot tx. The RESTRICT scan runs
    // (records the RI barrier token regardless of isolation), then the
    // injected writer fires at resolve_repo #2 (after-scan / before-commit).
    let mut del = Batch::new();
    del.id(2);
    del.delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    );
    let exec_resp = execute_in_open_tx(
        &del.build(),
        &resolver,
        None,
        None,
        &Actor::System,
        "test",
        &mut tx,
    )
    .await;
    assert!(
        exec_resp.is_ok(),
        "RESTRICT scan should pass before the race window opens, got: {exec_resp:?}"
    );

    // Verify the injected writer actually ran (the race genuinely fired).
    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    // Commit the explicit tx. Under the RI barrier, Phase 2-bis now fires for
    // the Snapshot tx (widened guard) and detects the writer's footprint on
    // the child table → PhantomConflict.
    let outcome = commit_interactive_tx(&repo, tx).await;
    drop(guard);

    match outcome {
        Err(CommitError::PhantomConflict { .. }) => { /* expected: barrier caught the race */ }
        Ok(_) => panic!(
            "explicit Snapshot parent delete must NOT commit silently past a \
             racing child insert — this is the exact dangling-reference bug \
             the RI barrier closes"
        ),
        Err(e) => panic!("expected PhantomConflict (RI barrier abort), got: {e:?}"),
    }

    // Invariant: parent still exists (delete aborted), child still exists —
    // no orphan, no dangling reference.
    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    assert_eq!(
        row_count(&parent_table).await,
        1,
        "parent must still exist post-abort"
    );
    assert_eq!(
        row_count(&child_table).await,
        1,
        "the raced-in child row must still exist post-abort"
    );
}

// ============================================================================
// 2. Quiescent-DB non-regression — no concurrent writer at all.
//
// An explicit-Snapshot FK-parent delete with NO concurrent writer must NOT
// spuriously abort. Mirrors the F-28 Step 3 / 5 spike's 50-trial quiescent
// assertion.
// ============================================================================

#[tokio::test]
async fn quiescent_explicit_snapshot_restrict_delete_does_not_spuriously_abort() {
    let mut spurious_aborts = 0u32;
    for trial in 0..50u32 {
        let (resolver, repo) = setup_race_test(FkAction::Restrict).await;

        let mut b = Batch::new();
        b.id(1);
        b.insert(
            "ins_parent",
            write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
        );
        execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();

        // No writer armed — the resolver's resolve_repo hook is a no-op.
        let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
            .await
            .unwrap();
        resolver.reset_counter();

        let mut del = Batch::new();
        del.id(2);
        del.delete(
            "del_parent",
            write::delete("parent").where_(filter::eq("id", 1)),
        );
        let exec_resp = execute_in_open_tx(
            &del.build(),
            &resolver,
            None,
            None,
            &Actor::System,
            "test",
            &mut tx,
        )
        .await;
        assert!(
            exec_resp.is_ok(),
            "trial {trial}: RESTRICT scan should pass (no child), got: {exec_resp:?}"
        );

        let outcome = commit_interactive_tx(&repo, tx).await;
        drop(guard);
        if outcome.is_err() {
            spurious_aborts += 1;
        }
    }
    assert_eq!(
        spurious_aborts, 0,
        "quiescent explicit-Snapshot FK-parent delete must NOT spuriously abort \
         via the RI barrier: {spurious_aborts}/50 spurious aborts"
    );
}
