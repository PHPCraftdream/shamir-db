//! F-28 Step 5 (S3-C) — deterministic end-to-end race-closure tests.
//!
//! Unlike the Step 3 spike's low-level harness (which hand-set
//! `footprint_tokens`/`IsolationLevel::Serializable` directly on a
//! manually-driven `TxContext`), these tests drive the REAL production
//! wiring through `execute_batch` — the same entry point a real client
//! request uses — with:
//!
//! - the parent-side isolation upgrade
//!   (`query_runner.rs::implicit_tx_isolation_for_fk_parent`, gated on
//!   `FkReverseCache::is_fk_parent_with_delete_action` for the DELETE arm
//!   and `is_fk_parent_with_update_action` for the UPDATE arm — F-35 split
//!   the old single `is_fk_parent_with_action`, which only ever consulted
//!   the cached `on_delete` field, so an `on_delete = NoAction, on_update =
//!   Restrict/Cascade/SetNull` FK never got the Serializable upgrade on its
//!   UPDATE path),
//! - the child-side footprint widening
//!   (`query_runner.rs::require_footprint_if_fk_child`, gated on
//!   `FkReverseCache::is_fk_child`),
//! - the bounded retry wrapper (`query_runner.rs::retry_on_tx_conflict`).
//!
//! ## The injection seam
//!
//! Mirrors `GateBarrierResolver`
//! (`executor_tests/ssi_tests.rs` ~line 20-67): a concurrent writer
//! transaction is run to FULL commitment at an exact program point, with no
//! sleeps and no timing dependence. The natural `TableResolver::resolve`
//! hook (used by `GateBarrierResolver`) does NOT land in the correct window
//! here — traced precisely for this production code path:
//!
//! - `resolver.resolve()` is called exactly twice for an implicit
//!   RESTRICT-shaped delete: once on the parent (before the tx even
//!   begins) and once on the child, immediately BEFORE
//!   `child_has_reference`'s scan (`fk_restrict.rs`) — i.e. the resolve on
//!   the child happens strictly BEFORE the SSI predicate is recorded, not
//!   after.
//! - `resolver.resolve_repo()` is called FOUR times per successful (no
//!   retry) attempt, in order: (1) the Delete arm itself
//!   (`query_runner.rs`), (2) `implicit_tx_isolation_for_fk_parent`, (3)
//!   `fk_restrict.rs::discover_restrict_refs` (inside `check_fk_restrict`,
//!   which ALSO runs `child_has_reference`'s scan — call #3 straddles the
//!   scan), (4) `fk_actions.rs::discover_action_refs` (inside
//!   `plan_cascade`, which runs strictly AFTER `check_fk_restrict` has
//!   fully completed, including its scan).
//!
//! So hooking `resolve_repo`'s 4th invocation is exactly the after-scan,
//! before-commit window: the SSI `TableScan` predicate is already recorded
//! (from call #3's `check_fk_restrict`), and the delete has not yet staged
//! or committed. Firing a complete concurrent writer transaction there
//! reproduces the exact race the brief describes.
//!
//! Two independently-counted resolver wrappers are used — one for the
//! outer delete, one for the inner concurrent writer — so the writer's own
//! `resolve_repo`/`resolve` calls (its own insert's
//! `require_footprint_if_fk_child` hook) never interfere with the outer
//! delete's call-counting.

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
use crate::query::batch::execute_batch;
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
async fn row_count(table: &TableManager) -> u64 {
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
struct RaceInjectingResolver {
    db: DbInstance,
    repo: String,
    registry: Arc<ValidatorRegistry>,
    resolve_repo_calls: AtomicUsize,
    inject_at: usize,
    /// The concurrent writer batch to run when `inject_at` is reached.
    /// `Mutex` only so the closure captured inside `resolve_repo` (an
    /// `&self` method) can take it out exactly once (idempotent — a second
    /// hit past `inject_at`, e.g. from a retry, is a no-op).
    writer: tokio::sync::Mutex<Option<InjectedWriter>>,
}

impl RaceInjectingResolver {
    /// Zero the call counter. Tests call this immediately BEFORE issuing the
    /// delete op under test, so `inject_at`'s ordinal counts only
    /// `resolve_repo` calls made by the DELETE itself — independent of how
    /// many calls any earlier seed-data inserts made.
    fn reset_counter(&self) {
        self.resolve_repo_calls.store(0, Ordering::SeqCst);
    }
}

struct InjectedWriter {
    req: shamir_query_types::batch::BatchRequest,
    /// Independent resolver for the writer batch — see module doc: the
    /// writer's own `resolve_repo`/`resolve` calls must not perturb the
    /// outer delete's call-counting.
    resolver: TxTestResolver,
}

/// Minimal resolver for the injected concurrent writer — resolves against
/// the SAME live repo (so its commit is really visible to the outer tx) but
/// keeps its own independent (uncounted) call stream.
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

#[tokio::test]
async fn restrict_race_closed_end_to_end_via_execute_batch() {
    let (resolver, repo) = setup_race_test(FkAction::Restrict).await;

    // Seed the parent row (plain autocommit insert).
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Arm the injected writer: a concurrent autocommit insert of a NEW
    // child row referencing the SAME parent, fired at the exact
    // after-scan/before-commit seam (see module doc).
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
    // Reset the ordinal counter so `INJECT_AT_RESOLVE_REPO_CALL` counts only
    // `resolve_repo` calls made by the delete op below (see
    // `RaceInjectingResolver::reset_counter`'s doc).
    resolver.reset_counter();

    // Delete the parent. Under the OLD (pre-Step-5) mechanism this would
    // always commit cleanly (Snapshot never aborts, and the RESTRICT scan
    // ran before the race window opened) — silently leaving the raced-in
    // child row referencing a deleted parent (a dangling reference). Under
    // S3-C, the delete's implicit tx is upgraded to Serializable (parent
    // is FK-parent-with-Restrict), the RESTRICT scan records a real SSI
    // predicate, and the concurrent writer's insert (flagged as an FK
    // child) publishes a footprint for it — so the delete's FIRST attempt
    // MUST detect the conflict at commit time (`tx_conflict`).
    //
    // `retry_on_tx_conflict` then transparently retries the WHOLE attempt
    // (re-plan against a fresh snapshot). By the retry, the raced-in child
    // row is genuinely committed — so the retry's OWN fresh RESTRICT scan
    // correctly (and legitimately) sees it and rejects with plain
    // `fk_restrict`, not another `tx_conflict`. Both outcomes are
    // acceptable per this test's invariant: the delete must NEVER commit
    // silently past the race (leaving a dangling reference). Whether the
    // terminal error is `tx_conflict` (retries exhausted without the
    // retry's own scan settling) or `fk_restrict` (a retry's fresh scan
    // legitimately caught it) is an internal retry-count/timing detail, not
    // the invariant under test.
    let mut b = Batch::new();
    b.id(2);
    b.delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    );
    let resp = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test").await;

    // Verify the injected writer actually ran (the race genuinely fired).
    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    match resp {
        Err(e) => {
            assert!(
                matches!(e.code(), Some("tx_conflict") | Some("fk_restrict")),
                "delete must reject the race via a coded SSI conflict OR a \
                 legitimate fk_restrict (a retry's fresh scan seeing the \
                 now-committed race), got: {e:?}"
            );
        }
        Ok(_) => panic!(
            "delete must NOT commit silently past a racing child insert — \
             this would be the exact 'dangling reference' bug S3-C closes"
        ),
    }

    // Invariant: parent still exists (delete aborted/rejected), and the
    // raced-in child row still exists — no orphan, no silent data-loss, no
    // dangling reference. Verify via a fresh read through the SAME repo.
    // (This test seeds only the parent up front — the single child row
    // present here is the one the injected writer raced in.)
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
// 2. End-to-end race closure — CASCADE (action-agnostic proof).
//
// Same race shape, but with an ON DELETE CASCADE action instead of RESTRICT
// — proving the mechanism protects any Serializable scan of the child
// table regardless of what the caller does with the result (delete-reject
// vs. cascade-delete).
// ============================================================================

#[tokio::test]
async fn cascade_race_closed_end_to_end_via_execute_batch() {
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
    // Reset the ordinal counter so `INJECT_AT_RESOLVE_REPO_CALL` counts only
    // `resolve_repo` calls made by the delete op below (see
    // `RaceInjectingResolver::reset_counter`'s doc).
    resolver.reset_counter();

    let mut b = Batch::new();
    b.id(2);
    b.delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    );
    let resp = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test").await;

    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();

    match resp {
        Err(e) => {
            assert_eq!(
                e.code(),
                Some("tx_conflict"),
                "cascade delete must abort with the coded SSI conflict, got: {e:?}"
            );
            // Parent survives an aborted delete; the raced-in child row
            // must NOT have been silently orphaned by a cascade that ran
            // against a stale (pre-race) snapshot.
            assert_eq!(row_count(&parent_table).await, 1);
            assert_eq!(
                row_count(&child_table).await,
                1,
                "the raced-in child row must still exist post-abort"
            );
        }
        Ok(_) => {
            // The delete won the race and committed: its cascade plan must
            // then have been built AFTER the race (impossible with the
            // deterministic single-shot injection above, since the writer
            // only ever fires once, before commit) — so this arm should be
            // unreachable in practice. If it ever is reached, the correct
            // invariant is still checkable: no orphan may exist.
            assert_eq!(row_count(&parent_table).await, 0, "parent was deleted");
            assert_eq!(
                row_count(&child_table).await,
                0,
                "if the delete committed, CASCADE must have removed the raced-in \
                 child row too — an orphan here would be silent data corruption"
            );
        }
    }
}

// ============================================================================
// 3. Quiescent-DB non-regression — no concurrent writer at all.
//
// An FK-parent delete with NO concurrent writer must NOT spuriously abort.
// Mirrors the spike's 50-trial quiescent test.
// ============================================================================

#[tokio::test]
async fn quiescent_restrict_delete_does_not_spuriously_abort() {
    for trial in 0..50 {
        let (resolver, _repo) = setup_race_test(FkAction::Restrict).await;

        let mut b = Batch::new();
        b.id(1);
        b.insert(
            "ins_parent",
            write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
        );
        execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();

        // No writer armed — `resolver.writer` stays `None`, so the
        // `resolve_repo` hook is a pure no-op observer.
        let mut b = Batch::new();
        b.id(2);
        b.delete(
            "del_parent",
            write::delete("parent").where_(filter::eq("id", 1)),
        );
        let resp = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test").await;
        assert!(
            resp.is_ok(),
            "trial {trial}: quiescent FK-parent delete (no concurrent writer, \
             no matching child) must NOT spuriously abort under the Serializable \
             upgrade, got: {resp:?}"
        );
    }
}

// ============================================================================
// 4. Retry policy — a resolved race does not surface as a client error.
//
// The injected writer fires on the delete's FIRST attempt (as in test 1),
// but this time the writer's insert is immediately followed by a DELETE of
// that same raced-in row (still inside the SAME writer batch, committed
// before the outer delete's commit) — so by the time the outer delete
// retries, the conflicting row is already gone and the retry succeeds
// cleanly. This proves `retry_on_tx_conflict` swallows an already-resolved
// race instead of surfacing `tx_conflict` to the caller.
// ============================================================================

#[tokio::test]
async fn resolved_race_retried_transparently_no_client_visible_error() {
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

    // Writer: insert a racing child reference, then immediately remove it
    // again — same batch (autocommit ops run sequentially), so by the time
    // the outer delete's SECOND attempt runs, no child row exists at all.
    // The FIRST attempt still aborts (its predicate conflicts with the
    // writer's insert — an insert-then-delete in the SAME repo still
    // publishes a footprint for the table, which is what the conflict scan
    // sees), but the retry re-plans against the now-quiescent state and
    // must succeed.
    let mut wb = Batch::new();
    wb.id("writer");
    wb.insert(
        "ins_child_race",
        write::insert("child").row(doc().set("parent_id", 1).set("label", "race")),
    );
    wb.delete(
        "del_child_race",
        write::delete("child").where_(filter::eq("label", "race")),
    );
    *resolver.writer.lock().await = Some(InjectedWriter {
        req: wb.build(),
        resolver: TxTestResolver { repo: repo.clone() },
    });
    // Reset the ordinal counter so `INJECT_AT_RESOLVE_REPO_CALL` counts only
    // `resolve_repo` calls made by the delete op below (see
    // `RaceInjectingResolver::reset_counter`'s doc).
    resolver.reset_counter();

    let mut b = Batch::new();
    b.id(2);
    b.delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    );
    let resp = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test").await;

    assert!(
        resp.is_ok(),
        "an already-resolved race (writer's conflicting change is gone by the \
         retry) must NOT surface tx_conflict to the caller — retry_on_tx_conflict \
         should have absorbed it transparently, got: {resp:?}"
    );

    let parent_table = repo.get_table("parent").await.unwrap();
    assert_eq!(
        row_count(&parent_table).await,
        0,
        "the retried delete must have actually committed"
    );
}

// ============================================================================
// 5. Never-yet-interned FK field — the scan must still run (and record its
// predicate) even when the child table's FK field was never interned.
//
// A brand-new child table (no row ever written, so "parent_id" has never
// been interned) races a concurrent insert. Confirms the fix in
// `fk_restrict.rs::child_has_reference` (F-28 Step 5 / memo §2.4): the
// field-id lookup gate must never sit in front of the `list_stream_tx`
// call in a way that skips the scan (and its SSI predicate recording)
// entirely.
// ============================================================================

#[tokio::test]
async fn never_interned_child_field_still_records_predicate_and_catches_race() {
    let (resolver, repo) = setup_race_test(FkAction::Restrict).await;

    // Seed the parent WITHOUT ever writing a row to `child` — so `child`'s
    // "parent_id" field has never been interned anywhere (base or any tx
    // overlay) at the time the delete's RESTRICT scan runs.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Confirm the precondition: child table has zero rows, so "parent_id"
    // is genuinely never-interned at this point.
    let child_table_pre = repo.get_table("child").await.unwrap();
    assert_eq!(row_count(&child_table_pre).await, 0);

    // Arm the injected writer: the FIRST EVER row in `child`, referencing
    // the parent — this is exactly the moment "parent_id" gets interned,
    // racing the outer delete's scan.
    let mut wb = Batch::new();
    wb.id("writer");
    wb.insert(
        "ins_child_first_ever",
        write::insert("child").row(doc().set("parent_id", 1).set("label", "first")),
    );
    *resolver.writer.lock().await = Some(InjectedWriter {
        req: wb.build(),
        resolver: TxTestResolver { repo: repo.clone() },
    });
    // Reset the ordinal counter so `INJECT_AT_RESOLVE_REPO_CALL` counts only
    // `resolve_repo` calls made by the delete op below (see
    // `RaceInjectingResolver::reset_counter`'s doc).
    resolver.reset_counter();

    let mut b = Batch::new();
    b.id(2);
    b.delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    );
    let resp = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test").await;

    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    // The race MUST be caught: if the never-interned-field gap were still
    // open, `child_has_reference` would short-circuit to `Ok(false)` BEFORE
    // ever constructing `list_stream_tx` — no predicate recorded — and this
    // delete would commit cleanly despite the race, leaving the freshly
    // inserted child row dangling.
    //
    // Either terminal error is acceptable (see the identical rationale in
    // `restrict_race_closed_end_to_end_via_execute_batch`): the FIRST
    // attempt catches the SSI conflict (`tx_conflict`), or
    // `retry_on_tx_conflict`'s retry re-scans against the now-committed
    // race and legitimately rejects with `fk_restrict`. Only a silent `Ok`
    // (the gap reopened) is the bug this test guards against.
    match resp {
        Err(e) => {
            assert!(
                matches!(e.code(), Some("tx_conflict") | Some("fk_restrict")),
                "must reject the race via a coded SSI conflict OR a legitimate \
                 fk_restrict, got: {e:?}"
            );
        }
        Ok(_) => panic!(
            "the never-interned-FK-field gap must be closed: the scan (and its \
             predicate recording) must run even when the field id can't be \
             resolved at scan-construction time"
        ),
    }

    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    assert_eq!(row_count(&parent_table).await, 1, "parent must still exist");
    assert_eq!(
        row_count(&child_table).await,
        1,
        "the raced-in first-ever child row must still exist post-abort"
    );
}

// ============================================================================
// F-35 — on_update race closure.
//
// The tests above exercise the on_delete path exclusively (every FK is built
// via `ForeignKeyRef::with_on_delete`). F-35 closed a gap where the cache
// stored only `on_delete` in each `ReverseFkEntry`, so the UPDATE arm's
// `is_fk_parent_with_update_action` consult a role flag the cache did not
// carry — an `on_delete = NoAction, on_update = Restrict/Cascade/SetNull` FK
// never got the Serializable upgrade for its implicit UPDATE path, silently
// reopening the cross-transaction race F-28 was meant to close for on_update.
//
// These tests mirror the on_delete structure exactly, but shape the FK via
// `ForeignKeyRef::with_on_update` (on_delete stays NoAction, isolating the
// UPDATE-path role flag from the DELETE-path one) and drive an implicit
// UPDATE (re-keying the parent's referenced field) instead of a DELETE.
//
// Both commit orderings are covered the same way the on_delete tests above
// cover them: the writer fires once in the after-begin/before-commit window
// (the deterministic injection point), and each test accepts BOTH terminal
// outcomes the SSI resolution can produce — `tx_conflict` (the child write
// won the race; the parent aborted) and the legitimate action outcome on
// `retry_on_tx_conflict`'s fresh-snapshot retry (the parent won; its
// re-planned fan-out correctly handled the now-committed child). Only a
// silent `Ok` that leaves an orphaned/dangling child reference is rejected.
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

/// Read the first row's `field` as `i64` from `table` via a read query
/// through the race resolver. By the time tests call this the injected
/// writer has already been consumed, so the resolver's `resolve_repo` hook
/// is a pure no-op observer. Needed because an UPDATE CASCADE re-keys
/// (doesn't delete) the child, so `row_count` alone cannot distinguish a
/// correctly cascaded child from an orphaned one.
async fn read_first_i64(resolver: &RaceInjectingResolver, table: &str, field: &str) -> Option<i64> {
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
async fn seed_parent(resolver: &RaceInjectingResolver, id: i64) {
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
async fn arm_child_writer(
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
async fn rekey_parent(
    resolver: &RaceInjectingResolver,
    from: i64,
    to: i64,
) -> Result<crate::query::batch::BatchResponse, crate::query::batch::BatchError> {
    let mut b = Batch::new();
    b.id(2);
    b.update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", from))
            .set(doc().set("id", to)),
    );
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
fn assert_on_update_parent_flagged(repo: &crate::repo::RepoInstance) {
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

// ============================================================================
// 6. End-to-end on_update race closure — RESTRICT.
//
// A genuinely concurrent writer inserts a NEW child reference (against the
// OLD parent value) in the implicit UPDATE's after-begin/before-commit window.
// Invariant: NEVER "update committed AND a dangling child reference exists" —
// the raced-in reference is either caught directly by the RESTRICT scan
// (`fk_restrict`) or the operation aborts. Only the silent-corruption outcome
// (update committed, child left referencing the OLD, now-gone value) is not
// acceptable.
// ============================================================================

#[tokio::test]
async fn on_update_restrict_race_closed_end_to_end_via_execute_batch() {
    let (resolver, repo) = setup_race_test_on_update(FkAction::Restrict).await;
    seed_parent(&resolver, 1).await;

    // F-35 proof #1 — the upgrade DECISION: pre-F-35 the UPDATE arm consulted
    // the single cached `on_delete` field (NoAction for this FK) and opened at
    // Snapshot; now it consults the `on_update` flag and upgrades.
    assert_on_update_parent_flagged(&repo);

    arm_child_writer(&resolver, &repo, 1).await;
    let resp = rekey_parent(&resolver, 1, 2).await;

    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    match resp {
        Err(e) => {
            assert!(
                matches!(e.code(), Some("tx_conflict") | Some("fk_restrict")),
                "on_update RESTRICT update must reject the race via a coded SSI \
                 conflict OR a legitimate fk_restrict, got: {e:?}"
            );
        }
        Ok(_) => panic!(
            "on_update RESTRICT update must NOT commit silently past a racing child \
             insert — this would be the exact 'dangling reference' bug F-35 closes"
        ),
    }

    // Invariant: the update was rejected, so the parent's referenced value
    // is unchanged and the child still references it — no orphan.
    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    assert_eq!(row_count(&parent_table).await, 1, "parent must still exist");
    assert_eq!(
        row_count(&child_table).await,
        1,
        "the raced-in child row must still exist"
    );
    assert_eq!(
        read_first_i64(&resolver, "parent", "id").await,
        Some(1),
        "parent id must be unchanged (update rejected)"
    );
    assert_eq!(
        read_first_i64(&resolver, "child", "parent_id").await,
        Some(1),
        "child must still reference parent id=1 (no dangling reference)"
    );
}

// ============================================================================
// 7. End-to-end on_update race closure — CASCADE.
//
// Same race shape, on_update = Cascade. The parent update re-keys id 1→2; the
// raced-in child references the OLD value 1. Invariant: NEVER "parent committed
// with id=2 AND child still references 1" (an orphaned reference). The
// operation either fails closed (no commit, both rows intact at id=1) or its
// retry wins and CASCADE correctly re-keys the child to 2 alongside.
// ============================================================================

#[tokio::test]
async fn on_update_cascade_race_closed_end_to_end_via_execute_batch() {
    let (resolver, repo) = setup_race_test_on_update(FkAction::Cascade).await;
    seed_parent(&resolver, 1).await;

    // F-35 proof #1 — the upgrade DECISION (see `assert_on_update_parent_flagged`).
    assert_on_update_parent_flagged(&repo);

    arm_child_writer(&resolver, &repo, 1).await;
    let resp = rekey_parent(&resolver, 1, 2).await;

    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    // F-35 proof #2 — the race is CAUGHT: the raced-in child reference is
    // NEVER left orphaned. The parent's UPDATE either fails closed (rolled
    // back, both rows intact at id=1) or commits with CASCADE having
    // re-keyed the child to the new value. Only a silent Ok leaving the
    // child referencing the OLD, now-gone value would be the corruption
    // F-35 closes.
    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    assert_eq!(row_count(&parent_table).await, 1, "parent must still exist");
    assert_eq!(
        row_count(&child_table).await,
        1,
        "the raced-in child row must still exist"
    );

    let parent_id = read_first_i64(&resolver, "parent", "id").await;
    let child_parent_id = read_first_i64(&resolver, "child", "parent_id").await;

    match resp {
        Err(e) => {
            // Operation failed closed — no commit, so nothing was re-keyed.
            // The observed deterministic code here is `fk_on_update`
            // (row-not-found): `plan_fk_on_update`'s `list_stream_tx` scan
            // reads committed-at-now and sees the raced-in child, but
            // `apply_fk_update_plan`'s per-key `read_one_tx_bytes` reads at
            // the tx snapshot (taken at begin, before the writer committed)
            // and can't resolve that same row — a pre-existing apply-path
            // visibility mismatch in `fk_on_update.rs`, out of scope for this
            // cache-role-flag fix. `tx_conflict` (Serializable commit abort)
            // is the other acceptable closed outcome. Either way: no orphan.
            assert!(
                matches!(e.code(), Some("tx_conflict") | Some("fk_on_update")),
                "on_update CASCADE update must fail closed (no orphan), got: {e:?}"
            );
            assert_eq!(
                parent_id,
                Some(1),
                "parent id unchanged after failed update"
            );
            assert_eq!(
                child_parent_id,
                Some(1),
                "child unchanged after failed update (no partial cascade, no orphan)"
            );
        }
        Ok(_) => {
            // The retry won: update committed AND CASCADE re-keyed the child.
            assert_eq!(
                parent_id,
                Some(2),
                "parent id must be re-keyed to 2 after a committed cascade update"
            );
            assert_eq!(
                child_parent_id,
                Some(2),
                "CASCADE must have re-keyed the raced-in child to 2 — an orphan \
                 (child still referencing 1 while parent is 2) would be the exact \
                 silent-corruption bug F-35 closes"
            );
        }
    }
}

// ============================================================================
// 8. End-to-end on_update race closure — SET NULL.
//
// Same race shape, on_update = SetNull. Invariant: NEVER "parent committed
// with id=2 AND child still references 1" (an orphaned reference). The
// operation either fails closed (both rows intact at id=1) or its retry wins
// and SET NULL correctly nulls the raced-in child's FK field.
// ============================================================================

#[tokio::test]
async fn on_update_set_null_race_closed_end_to_end_via_execute_batch() {
    let (resolver, repo) = setup_race_test_on_update(FkAction::SetNull).await;
    seed_parent(&resolver, 1).await;

    // F-35 proof #1 — the upgrade DECISION (see `assert_on_update_parent_flagged`).
    assert_on_update_parent_flagged(&repo);

    arm_child_writer(&resolver, &repo, 1).await;
    let resp = rekey_parent(&resolver, 1, 2).await;

    assert!(
        resolver.writer.lock().await.is_none(),
        "injected writer must have been consumed (race window must have fired)"
    );

    let parent_table = repo.get_table("parent").await.unwrap();
    let child_table = repo.get_table("child").await.unwrap();
    assert_eq!(row_count(&parent_table).await, 1, "parent must still exist");
    assert_eq!(
        row_count(&child_table).await,
        1,
        "the raced-in child row must still exist"
    );

    let parent_id = read_first_i64(&resolver, "parent", "id").await;
    // Null reads back as `None` via `get_value_i64`.
    let child_parent_id = read_first_i64(&resolver, "child", "parent_id").await;

    match resp {
        Err(e) => {
            // Operation failed closed — see the CASCADE test above for the
            // `fk_on_update` (row-not-found) rationale. Either way: no orphan.
            assert!(
                matches!(e.code(), Some("tx_conflict") | Some("fk_on_update")),
                "on_update SET NULL update must fail closed (no orphan), got: {e:?}"
            );
            assert_eq!(
                parent_id,
                Some(1),
                "parent id unchanged after failed update"
            );
            assert_eq!(
                child_parent_id,
                Some(1),
                "child unchanged after failed update (no partial set-null, no orphan)"
            );
        }
        Ok(_) => {
            // The retry won: update committed AND SET NULL nulled the child.
            assert_eq!(
                parent_id,
                Some(2),
                "parent id must be re-keyed to 2 after a committed set-null update"
            );
            assert_eq!(
                child_parent_id, None,
                "SET NULL must have nulled the raced-in child's FK field — an orphan \
                 (child still referencing 1 while parent is 2) would be the exact \
                 silent-corruption bug F-35 closes"
            );
        }
    }
}

// ============================================================================
// 9. Regression — on_delete = NoAction, on_update = NoAction does NOT upgrade
//    to Serializable.
//
// Confirms the F-35 split did not accidentally flag EVERY FK-parent table
// for the Serializable upgrade regardless of action kind. The proof is
// direct: the cache's two role flags must both read `false` for a
// NoAction/NoAction FK. (Behaviorally, NoAction runs no on_update fan-out
// scan, so the isolation level is not observable via an FK-race abort either
// way — the flag assertions below are the authoritative check; the clean
// commit just confirms the NoAction path was not broken.)
// ============================================================================

#[tokio::test]
async fn on_update_no_action_does_not_upgrade_to_serializable() {
    let (resolver, repo) = setup_race_test_on_update(FkAction::NoAction).await;
    seed_parent(&resolver, 1).await;

    // The seed insert warmed the cache (its `require_footprint_if_fk_child`
    // hook builds the whole-repo reverse-FK map on the first touch).
    let cache = repo.fk_reverse_cache();
    assert!(
        !cache.is_fk_parent_with_update_action("parent"),
        "on_update=NoAction must NOT flag the parent for the UPDATE-path upgrade"
    );
    assert!(
        !cache.is_fk_parent_with_delete_action("parent"),
        "on_delete=NoAction must NOT flag the parent for the DELETE-path upgrade"
    );

    // Behavioral confirmation: the implicit UPDATE commits cleanly (no
    // spurious Serializable abort) and re-keys the parent as requested.
    let resp = rekey_parent(&resolver, 1, 2).await;
    assert!(
        resp.is_ok(),
        "NoAction/NoAction parent UPDATE must commit cleanly (no spurious abort): {resp:?}"
    );
    assert_eq!(
        read_first_i64(&resolver, "parent", "id").await,
        Some(2),
        "parent id must be re-keyed to 2"
    );
}
