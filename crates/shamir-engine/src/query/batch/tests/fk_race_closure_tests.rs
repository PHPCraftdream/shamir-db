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
//!   `FkReverseCache::is_fk_parent_with_action`),
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
