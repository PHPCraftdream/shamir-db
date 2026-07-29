//! F-40b (#855 spike + #856 full implementation) — RI barrier: deterministic
//! race harness for the EXPLICIT `Snapshot`-isolation FK-parent DELETE /
//! UPDATE path.
//!
//! Mirrors `fk_race_closure_tests.rs`'s `RaceInjectingResolver` shape (the
//! exact same `resolve_repo`-call-ordinal deterministic injection seam), but
//! with an EXPLICIT `Snapshot` transaction as the outer operation instead of
//! an implicit one. The injection ordinal is 2 for every arm (the explicit-tx
//! DELETE arm has 2 `resolve_repo` calls: restrict-discover / cascade-discover;
//! the explicit-tx UPDATE arm has 2: `require_footprint_if_fk_child` /
//! `discover_on_update_refs`), because neither explicit arm calls the
//! implicit-only isolation-upgrade hook.
//!
//! ## The mechanism under test
//!
//! Every FK reverse-check scan (`fk_restrict.rs::child_has_reference`,
//! `fk_actions.rs`'s cascade probes, `fk_on_update.rs`'s on-update probes)
//! records the child `table_token` into `TxContext.ri_barrier_tokens`
//! REGARDLESS of isolation (not gated on `Serializable` like the existing
//! `predicate_set` recording). At commit, every live Phase 2-bis guard
//! (`pre_commit_locked_validate`, `pre_commit_locked`) runs
//! `predicate_conflicts_batch` / `record_conflicts` over those tokens, so a
//! concurrent committer that touched the child table in the commit window
//! aborts the parent — closing the cross-transaction FK TOCTOU race that
//! F-28 Step 5 closed for the IMPLICIT path only.
//!
//! Step 1 (#855) proved the mechanism for RESTRICT (1 recording site + 1
//! guard site + this harness). Step 2 (#856) extends it to CASCADE, SET NULL,
//! and ON UPDATE (the remaining recording + guard sites) — the tests below
//! cover all four actions.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_query_types::batch::BatchError;
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

// ============================================================================
// 3. End-to-end race closure — EXPLICIT Snapshot parent DELETE (CASCADE /
//    SET NULL).
//
// Same harness as the RESTRICT race above; only the FK `on_delete` action
// differs. The cascade / setnull child scan
// (`fk_actions.rs::plan_cascade_recursive`) records the RI barrier token at
// scan entry (Step 2's new recording sites), regardless of isolation. A
// concurrent writer's child insert lands between this tx's snapshot and its
// commit; the barrier's Phase 2-bis re-check catches it → PhantomConflict.
//
// CASCADE uses a referencing writer child (proves the FK-specific orphan
// race); SET NULL uses a non-referencing writer child (see the helper's doc
// for the apply-path visibility mismatch that a referencing child would
// trigger). Both prove the cascade scan's recording site is wired into the
// barrier.
// ============================================================================

/// Shared DELETE-path race harness for CASCADE and SET NULL.
///
/// `writer_parent_id` controls whether the raced-in child references the
/// parent being deleted:
/// - **CASCADE** uses a referencing child (`parent_id = 1`). Its `delete_tx`
///   apply step is id-only (no point-read of the child row), so the plan /
///   apply succeeds and the barrier catches the writer's footprint at commit
///   → `PhantomConflict`. This proves the FK-specific orphan race is closed.
/// - **SET NULL** uses a NON-referencing child (`parent_id = 999`). A
///   referencing child would make the cascade scan (which reads
///   committed-at-now via `list_stream_tx`) see it and plan a SET NULL
///   mutation whose `read_one_tx_bytes` apply step reads at the tx snapshot
///   and can't resolve the raced-in row — a pre-existing apply-path
///   visibility mismatch documented in `fk_race_closure_tests.rs`'s on_update
///   CASCADE test (lines ~982-990), which fails closed *before* the barrier
///   can fire. The non-referencing child still touches the child table, so
///   the barrier's `TableScan { table_token }` dep catches it at Phase 2-bis
///   → `PhantomConflict`, proving the SET NULL scan's recording site
///   (`fk_actions.rs::plan_cascade_recursive`) is wired into the barrier.
///
/// Either way the scan records the RI barrier token and the barrier fires at
/// commit — the two actions exercise the same recording site and guard.
async fn run_explicit_snapshot_delete_race(action: FkAction, writer_parent_id: i64) {
    let (resolver, repo) = setup_race_test(action).await;

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
    // row, fired at resolve_repo ordinal 2.
    let mut wb = Batch::new();
    wb.id("writer");
    wb.insert(
        "ins_child_race",
        write::insert("child").row(
            doc()
                .set("parent_id", writer_parent_id)
                .set("label", "race"),
        ),
    );
    *resolver.writer.lock().await = Some(InjectedWriter {
        req: wb.build(),
        resolver: TxTestResolver { repo: repo.clone() },
    });

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
        "{action:?} scan/plan should pass before the race window opens, got: {exec_resp:?}"
    );
    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    let outcome = commit_interactive_tx(&repo, tx).await;
    drop(guard);

    match outcome {
        Err(CommitError::PhantomConflict { .. }) => { /* expected: barrier caught the race */ }
        Ok(_) => panic!(
            "explicit Snapshot parent delete (on_delete={action:?}) must NOT commit silently \
             past a racing child write — the RI barrier must catch it"
        ),
        Err(e) => panic!("expected PhantomConflict (RI barrier abort), got: {e:?}"),
    }

    // Invariant: parent + raced-in child both still exist post-abort.
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
        "raced-in child must still exist post-abort"
    );
}

#[tokio::test]
async fn explicit_snapshot_cascade_race_closed_via_ri_barrier() {
    // Referencing child: CASCADE's id-only delete_tx apply succeeds, so the
    // barrier catches the writer's footprint at commit → PhantomConflict.
    run_explicit_snapshot_delete_race(FkAction::Cascade, 1).await;
}

#[tokio::test]
async fn explicit_snapshot_set_null_race_closed_via_ri_barrier() {
    // Non-referencing child: avoids the apply-path visibility mismatch (see
    // the helper's doc) while still touching the child table so the barrier's
    // TableScan dep fires → PhantomConflict.
    run_explicit_snapshot_delete_race(FkAction::SetNull, 999).await;
}

// ============================================================================
// 4. End-to-end race closure — EXPLICIT Snapshot parent UPDATE (ON UPDATE
//    CASCADE).
//
// The parent UPDATE re-keys the referenced `id` field; a concurrent writer
// inserts a NEW child row. The on-update child scan
// (`fk_on_update.rs::plan_fk_on_update`) records the RI barrier token at
// scan entry (Step 2's new recording site), and Phase 2-bis catches the
// writer's footprint on the child table → PhantomConflict.
//
// The writer inserts a NON-referencing child (parent_id=999): a referencing
// child would make the on-update scan (which reads committed-at-now) see it
// and plan a re-key whose `read_one_tx_bytes` apply step reads at the tx
// snapshot and can't resolve the raced-in row — a pre-existing apply-path
// visibility mismatch documented in `fk_race_closure_tests.rs`'s on_update
// CASCADE test (~lines 982-990), which fails closed *before* the barrier can
// fire. The non-referencing child still touches the child table, so the
// barrier's `TableScan { table_token }` dep catches it at Phase 2-bis,
// proving the on-update scan's recording site is wired into the barrier.
//
// The explicit-tx UPDATE arm has 2 `resolve_repo` calls:
// 1. `require_footprint_if_fk_child` (child-side hook; the parent is not an FK
//    child here, so it is a no-op but still consumes the ordinal).
// 2. `plan_fk_on_update` → `discover_on_update_refs` (after this returns, the
//    child scan runs and records the barrier).
// ============================================================================

/// Build a parent/child test environment with a bound FK whose `on_update`
/// action is `action` (on_delete stays `NoAction`), mirroring
/// `fk_race_closure_tests.rs::setup_race_test_on_update`. The child field is
/// nullable so `on_update = SetNull` is exercisable.
async fn setup_race_test_on_update(
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

#[tokio::test]
async fn explicit_snapshot_on_update_race_closed_via_ri_barrier() {
    let (resolver, repo) = setup_race_test_on_update(FkAction::Cascade).await;

    // Seed the parent row with id=1.
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
    // row with a NON-referencing parent_id (999), fired at resolve_repo
    // ordinal 2 (discover_on_update_refs). A referencing child (parent_id=1,
    // the old value) would make the on-update scan (which reads
    // committed-at-now) see it and plan a re-key whose `read_one_tx_bytes`
    // apply step reads at the tx snapshot and can't resolve the raced-in row
    // — a pre-existing apply-path visibility mismatch documented in
    // `fk_race_closure_tests.rs`'s on_update CASCADE test (~lines 982-990).
    // The non-referencing child still touches the child table, so the
    // barrier's `TableScan { table_token }` dep catches it at Phase 2-bis.
    let mut wb = Batch::new();
    wb.id("writer");
    wb.insert(
        "ins_child_race",
        write::insert("child").row(doc().set("parent_id", 999).set("label", "race")),
    );
    *resolver.writer.lock().await = Some(InjectedWriter {
        req: wb.build(),
        resolver: TxTestResolver { repo: repo.clone() },
    });

    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
    resolver.reset_counter();

    // Re-key the parent's referenced field from 1 → 2 inside the open tx.
    // The on-update child scan runs, records the RI barrier token (Step 2's
    // new recording site in `plan_fk_on_update`); the injected writer fires
    // at ordinal 2.
    let mut upd = Batch::new();
    upd.id(2);
    upd.update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 1))
            .set(doc().set("id", 2)),
    );
    let exec_resp = execute_in_open_tx(
        &upd.build(),
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
        "on-update scan/plan should pass before the race window opens, got: {exec_resp:?}"
    );
    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    let outcome = commit_interactive_tx(&repo, tx).await;
    drop(guard);

    match outcome {
        Err(CommitError::PhantomConflict { .. }) => { /* expected: barrier caught the race */ }
        Ok(_) => panic!(
            "explicit Snapshot parent update (on_update=Cascade) must NOT commit silently \
             past a racing child write — the RI barrier must catch it"
        ),
        Err(e) => panic!("expected PhantomConflict (RI barrier abort), got: {e:?}"),
    }

    // Invariant: parent still exists with id=1 (update aborted), raced-in
    // child still exists — no silent commit.
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
        "raced-in child must still exist post-abort"
    );
}

// ============================================================================
// 5. Quiescent-DB non-regression for the 3 new actions (CASCADE / SET NULL /
//    ON UPDATE).
//
// No concurrent writer at all. An explicit-Snapshot FK-parent delete (CASCADE
// / SET NULL) or update (ON UPDATE CASCADE) with an empty child table must
// NOT spuriously abort via the RI barrier. Mirrors the RESTRICT quiescent
// trial above (test 2).
// ============================================================================

#[tokio::test]
async fn quiescent_explicit_snapshot_cascade_delete_does_not_spuriously_abort() {
    let mut spurious_aborts = 0u32;
    for trial in 0..30u32 {
        let (resolver, repo) = setup_race_test(FkAction::Cascade).await;

        let mut b = Batch::new();
        b.id(1);
        b.insert(
            "ins_parent",
            write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
        );
        execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();

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
            "trial {trial}: CASCADE scan should pass (no child), got: {exec_resp:?}"
        );

        let outcome = commit_interactive_tx(&repo, tx).await;
        drop(guard);
        if outcome.is_err() {
            spurious_aborts += 1;
        }
    }
    assert_eq!(
        spurious_aborts, 0,
        "quiescent explicit-Snapshot CASCADE delete must NOT spuriously abort: \
         {spurious_aborts}/30 spurious aborts"
    );
}

#[tokio::test]
async fn quiescent_explicit_snapshot_set_null_delete_does_not_spuriously_abort() {
    let mut spurious_aborts = 0u32;
    for trial in 0..30u32 {
        let (resolver, repo) = setup_race_test(FkAction::SetNull).await;

        let mut b = Batch::new();
        b.id(1);
        b.insert(
            "ins_parent",
            write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
        );
        execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();

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
            "trial {trial}: SET NULL scan should pass (no child), got: {exec_resp:?}"
        );

        let outcome = commit_interactive_tx(&repo, tx).await;
        drop(guard);
        if outcome.is_err() {
            spurious_aborts += 1;
        }
    }
    assert_eq!(
        spurious_aborts, 0,
        "quiescent explicit-Snapshot SET NULL delete must NOT spuriously abort: \
         {spurious_aborts}/30 spurious aborts"
    );
}

#[tokio::test]
async fn quiescent_explicit_snapshot_on_update_does_not_spuriously_abort() {
    let mut spurious_aborts = 0u32;
    for trial in 0..30u32 {
        let (resolver, repo) = setup_race_test_on_update(FkAction::Cascade).await;

        let mut b = Batch::new();
        b.id(1);
        b.insert(
            "ins_parent",
            write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
        );
        execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();

        let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
            .await
            .unwrap();
        resolver.reset_counter();

        // Re-key the parent id 1 → 2. No child rows at snapshot → empty
        // fan-out plan → the update should commit cleanly.
        let mut upd = Batch::new();
        upd.id(2);
        upd.update(
            "upd_parent",
            write::update("parent")
                .where_(filter::eq("id", 1))
                .set(doc().set("id", 2)),
        );
        let exec_resp = execute_in_open_tx(
            &upd.build(),
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
            "trial {trial}: on-update scan should pass (no child), got: {exec_resp:?}"
        );

        let outcome = commit_interactive_tx(&repo, tx).await;
        drop(guard);
        if outcome.is_err() {
            spurious_aborts += 1;
        }
    }
    assert_eq!(
        spurious_aborts, 0,
        "quiescent explicit-Snapshot ON UPDATE must NOT spuriously abort: \
         {spurious_aborts}/30 spurious aborts"
    );
}

// ============================================================================
// 6. F-46 (#857, P0) — mutual commit-lock serialization for a CONCURRENT
//    PUBLISH inside the parent's own commit-lock window (not
//    before-parent-commit like the tests above).
//
// The 2026-07-28 readonly review's P0-1 finding: `commit_tx_lockfree`'s
// commit-lock condition checks `ri_barrier_tokens` but not
// `footprint_tokens`. A concurrent Snapshot writer into an FK-child table
// only ever gets `footprint_tokens` populated (never `ri_barrier_tokens`),
// so on the UNFIXED code it never takes `gate.commit_lock()` — it can
// publish (WAL write + `record_commit_writes` + materialize) WHILE the
// parent is holding `commit_lock` and has already passed its own Phase 2-bis
// `predicate_conflicts_batch` re-check. Neither side then detects the other:
// the parent's RESTRICT/CASCADE/SET NULL/ON UPDATE decision was made against
// a child-table state a concurrent writer subsequently changed.
//
// This is a DIFFERENT interleaving than tests 1-5 above: those inject the
// concurrent writer at a `resolve_repo()` call ordinal, which fires and
// fully resolves BEFORE the parent even calls `commit_interactive_tx` — a
// backward-looking recheck only. This harness instead uses the new
// `crate::tx::commit::TEST_POST_VALIDATE_PRE_PUBLISH_HOOK` seam
// (`commit.rs`), which parks the parent's OWN commit task strictly AFTER its
// Phase 2-bis check has passed and strictly BEFORE its Phase 4 WAL/publish —
// i.e. genuinely DURING the parent's commit-lock window, while the parent
// still holds `commit_lock`.
//
// Determinism (no sleeps, no wall-clock races): the parent's commit runs on
// a spawned task and parks at the hook; the harness busy-yields (via
// `tokio::task::yield_now`, not `sleep`) until `hook.reached` confirms the
// park. It then spawns the concurrent child-table writer and busy-yields a
// bounded number of times. `tokio::sync::Mutex` (what `commit_lock` is) can
// only complete a `.lock().await` waiter when the current holder's guard
// drops — it never spuriously wakes — so after enough yields with no
// progress, "the writer has not finished" is a PROOF that it is genuinely
// parked on `commit_lock`, not merely "still running": on the UNFIXED code
// the writer never takes that lock at all, so it always finishes within a
// handful of yields (no contention to wait out). This is the same
// no-sleeps-just-yield idiom `fk_reverse_cache_race_tests.rs` uses for its
// own build-pause handshake.
//
// nextest runs each test in its own process, so the global
// `TEST_POST_VALIDATE_PRE_PUBLISH_HOOK` can be `.set()` at most once per test
// function without leaking across tests in this file.
// ============================================================================

use crate::tx::commit::{PostValidatePrePublishHook, TEST_POST_VALIDATE_PRE_PUBLISH_HOOK};

/// Bounded yield budget for "prove the writer is genuinely parked on
/// `commit_lock`, not just slow". Generous enough that any writer NOT
/// contending on the lock finishes trivially within a handful of
/// cooperative yields on a single-threaded-per-task async executor: an
/// autocommit single-row insert with no other work does at most a few
/// `.await` points, each a `yield_now`-equivalent resumption. A writer that
/// IS blocked on `tokio::sync::Mutex::lock().await` cannot make ANY progress
/// in this loop no matter how many iterations, because that mutex only
/// wakes a waiter when the guard holder drops — never spuriously — so this
/// budget cannot produce a false "still blocked" reading due to scheduler
/// slowness.
const BLOCKED_PROBE_YIELDS: usize = 500;

/// Busy-yield (cooperative, no sleeps) until `hook.reached` shows the
/// commit-under-test has parked at the seam, or panic past a generous
/// iteration budget (would indicate the seam was never reached — a harness
/// bug, not a timing flake, since reaching the hook requires no external
/// event).
async fn wait_for_hook_reached(hook: &PostValidatePrePublishHook) {
    for _ in 0..100_000usize {
        if hook.reached.load(Ordering::SeqCst) >= 1 {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("commit-under-test never reached TEST_POST_VALIDATE_PRE_PUBLISH_HOOK");
}

/// Outcome of one concurrent-publish-during-commit-window trial.
struct ConcurrentPublishTrial {
    parent_outcome: Result<crate::tx::TxOutcome, CommitError>,
    writer_finished_before_release: bool,
    writer_outcome: Result<shamir_query_types::batch::BatchResponse, BatchError>,
    parent_row_count: u64,
    child_row_count: u64,
}

/// Runs the concurrent-publish-during-parent-commit-lock-window race for one
/// FK on_delete `action` (RESTRICT / CASCADE / SET NULL), against whatever
/// engine code is live when this test runs.
async fn run_concurrent_publish_during_commit_window_delete(
    action: FkAction,
) -> ConcurrentPublishTrial {
    let (resolver, repo) = setup_race_test(action).await;

    // Seed the parent row. No concurrent writer is armed on `resolver` here —
    // this harness drives the concurrent writer itself, via the new seam.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Install the post-validate/pre-publish pause hook.
    let hook = Arc::new(PostValidatePrePublishHook {
        reached: AtomicUsize::new(0),
        resume: tokio::sync::Notify::new(),
        armed: std::sync::atomic::AtomicBool::new(true),
    });
    TEST_POST_VALIDATE_PRE_PUBLISH_HOOK
        .set(Arc::clone(&hook))
        .expect("hook installed exactly once per test process (nextest: one process per test)");

    // Open the explicit Snapshot tx and run the RESTRICT/CASCADE/SET NULL
    // scan. No concurrent writer has touched the child table yet, so the
    // scan legitimately finds no reference (or plans against zero fan-out)
    // and records the RI barrier token.
    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();
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
        "{action:?} scan/plan should pass (no concurrent writer yet), got: {exec_resp:?}"
    );

    // Sanity: the repo's gate is a real handle we can probe non-blockingly.
    let gate = repo.tx_gate().await.unwrap();

    // Drive the parent's commit on a spawned task: it will run Phase 2-bis
    // (pass — nothing conflicts yet) under `commit_lock`, then park at the
    // hook, STILL HOLDING `commit_lock`.
    let repo_for_parent = repo.clone();
    let parent_task =
        tokio::spawn(async move { commit_interactive_tx(&repo_for_parent, tx).await });

    wait_for_hook_reached(&hook).await;

    // While the parent is parked mid-commit-lock-window, the gate must be
    // genuinely held (non-blocking probe fails) — confirms this harness is
    // actually testing the claimed interleaving, not a no-op.
    assert!(
        gate.try_commit_lock().is_none(),
        "parent must still hold commit_lock while parked at the post-validate/pre-publish hook"
    );

    // Fire the concurrent writer: a plain autocommit INSERT of a NEW child
    // row referencing the same parent, via the SAME live repo. On the
    // UNFIXED code this writer has only `footprint_tokens` set (never
    // `ri_barrier_tokens`), so it does NOT await `commit_lock` and races
    // straight through to a full commit while the parent is still parked. On
    // the FIXED code (footprint_tokens widened into the lock condition) it
    // blocks on `commit_lock` until the parent releases it.
    let mut wb = Batch::new();
    wb.id("writer");
    wb.insert(
        "ins_child_race",
        write::insert("child").row(doc().set("parent_id", 1).set("label", "race")),
    );
    let writer_resolver = TxTestResolver { repo: repo.clone() };
    let writer_task = tokio::spawn(async move {
        execute_batch(
            &wb.build(),
            &writer_resolver,
            None,
            None,
            Actor::System,
            "test",
        )
        .await
    });

    // Busy-yield a bounded, generous budget. If the writer is NOT contending
    // on commit_lock (the bug) it completes trivially inside this budget; if
    // it IS blocked on commit_lock (the fix) it can NEVER complete during
    // this loop, by the non-spurious-wake property of `tokio::sync::Mutex`.
    for _ in 0..BLOCKED_PROBE_YIELDS {
        tokio::task::yield_now().await;
    }
    let writer_finished_before_release = writer_task.is_finished();

    // Release the parent from the hook. `armed` is one-shot (see
    // `PostValidatePrePublishHook`'s doc): only the parent's arrival at the
    // seam consumed it, so when the writer reaches the SAME program point
    // after acquiring `commit_lock` in turn, it passes straight through —
    // a single `notify_one()` here is sufficient for the whole trial.
    hook.resume.notify_one();

    let parent_outcome = parent_task.await.unwrap();
    drop(guard);
    let writer_outcome = writer_task.await.unwrap();

    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    ConcurrentPublishTrial {
        parent_outcome,
        writer_finished_before_release,
        writer_outcome,
        parent_row_count: row_count(&parent_table).await,
        child_row_count: row_count(&child_table).await,
    }
}

/// RED proof (on the code as it stood before this task's fix): the
/// concurrent child writer completes its FULL commit (WAL + publish) WHILE
/// the parent is still parked mid-commit-lock-window — i.e.
/// `writer_finished_before_release` is `true`. This is the exact defeat of
/// the RI barrier's mutual-exclusion intent the 2026-07-28 review's P0-1
/// describes: the parent's `predicate_conflicts_batch` check already passed
/// against a child-table state that changed before the parent published, so
/// RESTRICT can allow a delete despite a concurrently-inserted child, or
/// CASCADE/SET NULL can miss the concurrently-inserted row entirely.
///
/// GREEN proof (after the fix — widening `commit_tx_lockfree`'s lock
/// condition to also check `!tx.footprint_tokens.is_empty()`): the writer's
/// Snapshot insert into the FK-child table also takes `commit_lock`, so it
/// CANNOT complete while the parent holds it — `writer_finished_before_release`
/// must be `false`, and the two commits are strictly ordered (parent first,
/// since it took the lock first): both succeed, and the raced-in child row
/// genuinely did not exist at the time the parent's RESTRICT/CASCADE/SET NULL
/// decision was made.
///
/// One `#[tokio::test]` PER action, not a `for action in [...]` loop inside a
/// single test — `TEST_POST_VALIDATE_PRE_PUBLISH_HOOK` is a process-wide
/// `OnceLock` that can be `.set()` exactly once per process. nextest gives
/// each TEST FUNCTION its own process, so looping 3 actions through
/// `run_concurrent_publish_during_commit_window_delete` (which calls
/// `.set()` every call) inside one `#[tokio::test]` would panic on the
/// second iteration's `.set()` — matches the established per-action-function
/// pattern this file already uses for the other multi-action race tests
/// (`explicit_snapshot_cascade_race_closed_via_ri_barrier` /
/// `..._set_null_...` above, each its own `#[tokio::test]`).
fn assert_concurrent_publish_trial(action: FkAction, trial: &ConcurrentPublishTrial) {
    assert!(
        !trial.writer_finished_before_release,
        "{action:?}: the concurrent FK-child writer must NOT be able to complete its \
         commit while the parent still holds commit_lock mid-validate/pre-publish window — \
         a writer finishing here means it never took commit_lock at all, defeating the RI \
         barrier's mutual-exclusion intent (2026-07-28 review P0-1)"
    );

    assert!(
        trial.parent_outcome.is_ok(),
        "{action:?}: with the writer genuinely serialized behind commit_lock, the parent's \
         own RI barrier check (which already passed before the writer could touch the child \
         table) must be allowed to commit cleanly, got: {:?}",
        trial.parent_outcome
    );
    assert!(
        trial.writer_outcome.is_ok(),
        "{action:?}: the writer must commit successfully once the parent releases \
         commit_lock, got: {:?}",
        trial.writer_outcome
    );

    // CASCADE deletes any pre-existing referencing child rows (there are
    // none here — the raced-in row lands AFTER the scan), so the parent
    // table always ends up empty for CASCADE regardless of ordering.
    // RESTRICT/SET NULL never remove the parent's own row via this path
    // in a way that depends on the race, so this assertion is about the
    // COMMIT outcome, not row survival — the row-count invariants below
    // are the correctness statement that matters.
    assert_eq!(
        trial.parent_row_count, 0,
        "{action:?}: parent delete committed"
    );
    assert_eq!(
        trial.child_row_count, 1,
        "{action:?}: the raced-in child row must exist post-commit — it was inserted \
         strictly AFTER the parent's RESTRICT/CASCADE/SET NULL decision was made, so it \
         correctly survives untouched by that decision"
    );
}

#[tokio::test]
async fn concurrent_child_publish_during_parent_commit_window_is_serialized_on_delete_restrict() {
    let trial = run_concurrent_publish_during_commit_window_delete(FkAction::Restrict).await;
    assert_concurrent_publish_trial(FkAction::Restrict, &trial);
}

#[tokio::test]
async fn concurrent_child_publish_during_parent_commit_window_is_serialized_on_delete_cascade() {
    let trial = run_concurrent_publish_during_commit_window_delete(FkAction::Cascade).await;
    assert_concurrent_publish_trial(FkAction::Cascade, &trial);
}

#[tokio::test]
async fn concurrent_child_publish_during_parent_commit_window_is_serialized_on_delete_set_null() {
    let trial = run_concurrent_publish_during_commit_window_delete(FkAction::SetNull).await;
    assert_concurrent_publish_trial(FkAction::SetNull, &trial);
}

// ============================================================================
// 7. Same concurrent-publish-during-commit-window race, for the ON UPDATE
//    CASCADE path (`fk_on_update.rs::plan_fk_on_update`'s recording site).
// ============================================================================

async fn run_concurrent_publish_during_commit_window_on_update() -> ConcurrentPublishTrial {
    let (resolver, repo) = setup_race_test_on_update(FkAction::Cascade).await;

    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let hook = Arc::new(PostValidatePrePublishHook {
        reached: AtomicUsize::new(0),
        resume: tokio::sync::Notify::new(),
        armed: std::sync::atomic::AtomicBool::new(true),
    });
    TEST_POST_VALIDATE_PRE_PUBLISH_HOOK
        .set(Arc::clone(&hook))
        .expect("hook installed exactly once per test process (nextest: one process per test)");

    let (mut tx, guard) = open_interactive_tx(&repo, shamir_tx::IsolationLevel::Snapshot)
        .await
        .unwrap();

    // Re-key the parent's referenced field 1 → 2. No child rows exist yet,
    // so the on-update scan records the RI barrier token against an empty
    // fan-out and passes.
    let mut upd = Batch::new();
    upd.id(2);
    upd.update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 1))
            .set(doc().set("id", 2)),
    );
    let exec_resp = execute_in_open_tx(
        &upd.build(),
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
        "on-update scan/plan should pass (no concurrent writer yet), got: {exec_resp:?}"
    );

    let gate = repo.tx_gate().await.unwrap();

    let repo_for_parent = repo.clone();
    let parent_task =
        tokio::spawn(async move { commit_interactive_tx(&repo_for_parent, tx).await });

    wait_for_hook_reached(&hook).await;

    assert!(
        gate.try_commit_lock().is_none(),
        "parent must still hold commit_lock while parked at the post-validate/pre-publish hook"
    );

    // Concurrent writer: a NON-referencing child row (parent_id=999) — a
    // referencing child (parent_id=1, the OLD value) would make the
    // on-update scan's apply step hit the pre-existing
    // read_one_tx_bytes-at-snapshot visibility mismatch documented earlier in
    // this file, which fails closed *before* the barrier can fire; the
    // non-referencing child still exercises the SAME recording site + guard
    // (it touches the child table, so the `TableScan { table_token }` dep is
    // what's under test here, not row-level FK matching).
    let mut wb = Batch::new();
    wb.id("writer");
    wb.insert(
        "ins_child_race",
        write::insert("child").row(doc().set("parent_id", 999).set("label", "race")),
    );
    let writer_resolver = TxTestResolver { repo: repo.clone() };
    let writer_task = tokio::spawn(async move {
        execute_batch(
            &wb.build(),
            &writer_resolver,
            None,
            None,
            Actor::System,
            "test",
        )
        .await
    });

    for _ in 0..BLOCKED_PROBE_YIELDS {
        tokio::task::yield_now().await;
    }
    let writer_finished_before_release = writer_task.is_finished();

    hook.resume.notify_one();

    let parent_outcome = parent_task.await.unwrap();
    drop(guard);
    let writer_outcome = writer_task.await.unwrap();

    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    ConcurrentPublishTrial {
        parent_outcome,
        writer_finished_before_release,
        writer_outcome,
        parent_row_count: row_count(&parent_table).await,
        child_row_count: row_count(&child_table).await,
    }
}

#[tokio::test]
async fn concurrent_child_publish_during_parent_commit_window_is_serialized_on_update() {
    let trial = run_concurrent_publish_during_commit_window_on_update().await;

    assert!(
        !trial.writer_finished_before_release,
        "ON UPDATE CASCADE: the concurrent FK-child writer must NOT be able to complete its \
         commit while the parent still holds commit_lock mid-validate/pre-publish window"
    );
    assert!(
        trial.parent_outcome.is_ok(),
        "ON UPDATE CASCADE: parent must commit cleanly once genuinely serialized, got: {:?}",
        trial.parent_outcome
    );
    assert!(
        trial.writer_outcome.is_ok(),
        "ON UPDATE CASCADE: writer must commit successfully once released, got: {:?}",
        trial.writer_outcome
    );
    assert_eq!(
        trial.parent_row_count, 1,
        "parent row (re-keyed to id=2) must still exist post-commit"
    );
    assert_eq!(
        trial.child_row_count, 1,
        "the raced-in non-referencing child row must exist post-commit"
    );
}
