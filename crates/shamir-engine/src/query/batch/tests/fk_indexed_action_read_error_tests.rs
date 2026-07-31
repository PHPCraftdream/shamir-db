//! F-65 (#891, P1) — FK indexed-action fast paths must not swallow read
//! errors.
//!
//! An independent readonly review of snapshot `e145b1d3`
//! (`docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md`,
//! section P1-6) found that the F-53c FK indexed CASCADE/SET NULL/ON UPDATE
//! fast paths collapsed three distinct `read_one_tx_bytes` outcomes into one
//! "skip this candidate" (`_ => continue`): row genuinely absent (`Ok(None)`),
//! storage error, and decode error. After an AUTHORITATIVE index lookup told
//! the code a candidate exists, a real read `Err` must abort the whole RI
//! (referential integrity) operation, not silently shrink the affected-row
//! set — a silently-shrunk CASCADE/SET NULL fan-out, or a silently-skipped ON
//! UPDATE propagation, is an RI violation with no error surfaced to the
//! caller.
//!
//! This is the same defect class F-55 (#881, commit `f9eed337`) fixed in the
//! FK reverse-cache discovery scan, applied here to the FOUR fast-path
//! candidate RE-READ sites in `fk_actions.rs` / `fk_on_update.rs`:
//!
//! 1. `fk_actions.rs` direct-child CASCADE/SET NULL index fast path.
//! 2. `fk_actions.rs` grandchild-recursion index fast path (same shape,
//!    `plan_cascade_for_ids`).
//! 3. `fk_actions.rs` grandchild ref-field-value collection loop (reads each
//!    already-cascade-selected row to gather its ref_field values for further
//!    grandchild discovery).
//! 4. `fk_on_update.rs` ON UPDATE index fast path.
//!
//! ## Fault-injection strategy
//!
//! `read_one_tx_bytes` returns raw undecoded bytes (`DbResult<Option<Bytes>>`)
//! — corrupting stored bytes can't reach its `Err` branch (there is no decode
//! step at that layer to fail), so it can't be used to trigger a genuine read
//! error deterministically. Instead these tests use the `#[cfg(test)]`
//! failure-injection seam `TEST_READ_ONE_TX_BYTES_FAILURE`
//! (`table/table_manager_streaming.rs`), which mirrors this codebase's
//! existing `TEST_*_HOOK` conventions (`TEST_POST_BARRIER_PRE_WRITE_HOOK` et
//! al.): a one-shot `(table_token, RecordId)`-keyed injector that makes the
//! NEXT `read_one_tx_bytes` call for an armed key return a genuine
//! `Err(DbError::Storage(..))` instead of reading.
//!
//! **"Arm every row in the target table" is sound ONLY for a single-level
//! (non-recursive) scenario**, where the whole table's rows are candidates
//! for exactly ONE re-read site (sites 1 and 4 below: each scenario
//! enumerates every `RecordId` currently in the target child table via
//! `list_stream` and arms all of them, so whichever candidate id(s) the
//! index fast-path selects for re-read are guaranteed to hit the injected
//! failure — no need to predict which specific id the fast path will pick).
//!
//! That strategy is INVALID for a multi-level self-referential recursion
//! (a table that is its own child at every recursion depth, e.g. a
//! self-referential CASCADE hierarchy) or, more generally, whenever the
//! SAME table is touched by more than one re-read site (including the
//! top-level DELETE/UPDATE's own row read, which is unrelated to any
//! `fk_actions.rs`/`fk_on_update.rs` site). Arming "every row" there also
//! arms rows read by sites the test is NOT trying to exercise — including,
//! in the self-referential case, the very row the top-level batch op
//! targets — so `result.is_err()` passing proves nothing about which site
//! actually failed. This is exactly what made a prior version of the
//! grandchild-recursion test (sites 2/3, see the comment on
//! `setup_grandchild_cascade_chain` below) an invalid oracle: it never
//! reached sites 2/3 at all, yet still returned `Err` for an unrelated
//! reason. The fix is per-id (or per-table, when each table has exactly one
//! candidate row) arming that leaves the OTHER re-read sites' candidates
//! unarmed, combined with asserting the returned error message contains the
//! target site's verbatim, already-site-specific string — proving THIS
//! site failed, not merely that something did.

use futures::StreamExt;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_query_types::batch::{BatchError, BatchResponse};
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use std::sync::Arc;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::execute_batch;
use crate::query::batch::TableResolver;
use crate::query::TableRef;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::{
    ReadOneTxBytesFailHook, TableConfig, TableManager, TEST_READ_ONE_TX_BYTES_FAILURE,
};
use crate::validator::schema::constraints::Constraints;
use crate::validator::schema::field_rule::FieldRule;
use crate::validator::schema::foreign_key::ForeignKeyRef;
use crate::validator::schema::schema_validator::SchemaValidator;
use crate::validator::schema::type_tag::TypeTag;
use crate::validator::{ValidatorBinding, ValidatorRegistry, WriteOp};

// ── Test resolver (same shape as fk_actions_tests / fk_on_update_tests) ─────

struct FkTestResolver {
    db: DbInstance,
    repo: String,
    registry: Arc<ValidatorRegistry>,
}

#[async_trait::async_trait]
impl TableResolver for FkTestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> shamir_storage::error::DbResult<TableManager> {
        let mut table = self.db.get_table(&self.repo, &table_ref.table).await?;
        table.set_validator_registry(Arc::clone(&self.registry));
        Ok(table)
    }

    async fn resolve_repo(
        &self,
        _repo_name: &str,
    ) -> shamir_storage::error::DbResult<crate::repo::RepoInstance> {
        self.db.get_repo(&self.repo).ok_or_else(|| {
            shamir_storage::error::DbError::NotFound(format!("repo '{}' not found", self.repo))
        })
    }
}

/// Bind a SchemaValidator with a single FK field to a child table. Same
/// helper shape as `fk_actions_tests::bind_fk_validator` /
/// `fk_on_update_tests::bind_fk_validator`, duplicated here to keep this test
/// file self-contained (test modules do not share private helpers across
/// files).
#[allow(clippy::too_many_arguments)]
fn bind_fk_validator(
    db: &DbInstance,
    registry: &Arc<ValidatorRegistry>,
    table_name: &str,
    validator_name: &str,
    field: &str,
    ref_table: &str,
    ref_field: &str,
    on_delete: FkAction,
    nullable: bool,
) {
    let schema = SchemaValidator::new(vec![FieldRule {
        path: vec![field.to_string()],
        ty: TypeTag::Int,
        constraints: Constraints {
            foreign_key: Some(ForeignKeyRef::with_on_delete(
                ref_table, ref_field, on_delete,
            )),
            required: !nullable,
            nullable,
            ..Default::default()
        },
        keyset_safe: false,
    }]);

    let validator_id = RecordId::from_ts(9101);
    registry
        .register(validator_id, validator_name, Arc::new(schema))
        .unwrap();

    let binding = ValidatorBinding {
        validator_id,
        ops: smallvec::smallvec![WriteOp::Delete, WriteOp::Update],
        priority: 1000,
    };

    let mut table = futures::executor::block_on(db.get_table("default", table_name)).unwrap();
    table.set_validator_registry(Arc::clone(registry));
    futures::executor::block_on(table.add_validator_binding(binding)).unwrap();
}

/// Same shape as `bind_fk_validator`, but sets `on_update` (via
/// `ForeignKeyRef::with_on_update`) instead of `on_delete`. Needed for the ON
/// UPDATE fast-path test (site 4): `discover_on_update_refs` /
/// `plan_fk_on_update` key off `ForeignKeyRef::on_update`, which
/// `ForeignKeyRef::with_on_delete` leaves at `FkAction::NoAction` — using the
/// wrong constructor here would silently make `plan_fk_on_update` discover no
/// action refs at all (empty plan, no fast path, no scan), so the update
/// would go through unrelated to FK fan-out and the injected
/// `read_one_tx_bytes` failure would never be reached.
#[allow(clippy::too_many_arguments)]
fn bind_fk_validator_on_update(
    db: &DbInstance,
    registry: &Arc<ValidatorRegistry>,
    table_name: &str,
    validator_name: &str,
    field: &str,
    ref_table: &str,
    ref_field: &str,
    on_update: FkAction,
    nullable: bool,
) {
    let schema = SchemaValidator::new(vec![FieldRule {
        path: vec![field.to_string()],
        ty: TypeTag::Int,
        constraints: Constraints {
            foreign_key: Some(ForeignKeyRef::with_on_update(
                ref_table, ref_field, on_update,
            )),
            required: !nullable,
            nullable,
            ..Default::default()
        },
        keyset_safe: false,
    }]);

    let validator_id = RecordId::from_ts(9102);
    registry
        .register(validator_id, validator_name, Arc::new(schema))
        .unwrap();

    let binding = ValidatorBinding {
        validator_id,
        ops: smallvec::smallvec![WriteOp::Delete, WriteOp::Update],
        priority: 1000,
    };

    let mut table = futures::executor::block_on(db.get_table("default", table_name)).unwrap();
    table.set_validator_registry(Arc::clone(registry));
    futures::executor::block_on(table.add_validator_binding(binding)).unwrap();
}

async fn insert_helper(
    resolver: &FkTestResolver,
    table: &str,
    doc: impl Into<shamir_types::types::value::QueryValue>,
) {
    let mut b = Batch::new();
    b.id(0);
    b.insert("ins", write::insert(table).row(doc));
    let req = b.build();
    execute_batch(&req, resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
}

/// Enumerate every `RecordId` currently committed in `table_name` and arm a
/// one-shot injected `read_one_tx_bytes` failure for each of them. Whichever
/// candidate id(s) an index fast-path selects for re-read are then
/// guaranteed to hit the injected `Err` — no need to predict exactly which
/// id the fast path will pick.
async fn arm_failure_for_all_rows(resolver: &FkTestResolver, table_name: &str) {
    let hook =
        TEST_READ_ONE_TX_BYTES_FAILURE.get_or_init(|| Arc::new(ReadOneTxBytesFailHook::default()));
    let table = resolver
        .db
        .get_table(&resolver.repo, table_name)
        .await
        .unwrap();
    let token = table.table_token();
    let stream = table.list_stream(64);
    futures::pin_mut!(stream);
    while let Some(batch) = stream.next().await {
        for (id, _) in batch.unwrap() {
            hook.arm(token, id);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Site 1 — `fk_actions.rs` direct-child CASCADE index fast-path re-read.
//
// Before the fix: `Ok(Some(b)) => b, _ => continue` silently dropped the
// candidate from the cascade set on a genuine read error, so the parent
// delete would "succeed" having cascaded fewer rows than the index proved
// exist — an RI violation with no error surfaced. After the fix: the `Err`
// arm propagates a `BatchError`, aborting the whole delete.
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cascade_index_fast_path_propagates_read_error() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    bind_fk_validator(
        &db,
        &registry,
        "child",
        "child_fk_cascade_read_err",
        "parent_id",
        "parent",
        "id",
        FkAction::Cascade,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_helper(
        &resolver,
        "parent",
        doc().set("id", 1_i64).set("name", "p1"),
    )
    .await;
    insert_helper(
        &resolver,
        "child",
        doc().set("cid", 100_i64).set("parent_id", 1_i64),
    )
    .await;

    // Supporting index on the FK column → F-53c fast-path engages (fresh
    // autocommit tx, no staged child writes).
    let child_table = resolver.db.get_table("default", "child").await.unwrap();
    child_table
        .create_index("idx_child_parent_id_site1", &["parent_id"])
        .await
        .expect("index creation");

    arm_failure_for_all_rows(&resolver, "child").await;

    let mut b = Batch::new();
    b.id(10);
    b.delete(
        "del",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    );
    let result = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test").await;

    assert!(
        result.is_err(),
        "a genuine read_one_tx_bytes error during the CASCADE index \
         fast-path re-read must abort the whole delete (return Err), not \
         silently continue past the poisoned candidate. Got: {result:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Site 4 — `fk_on_update.rs` ON UPDATE index fast-path re-read.
//
// Same defect, sibling call site for the `on_update` action instead of
// `on_delete`.
// ═══════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn on_update_index_fast_path_propagates_read_error() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    bind_fk_validator_on_update(
        &db,
        &registry,
        "child",
        "child_fk_on_update_read_err",
        "parent_id",
        "parent",
        "id",
        FkAction::Cascade,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_helper(
        &resolver,
        "parent",
        doc().set("id", 5_i64).set("name", "p5"),
    )
    .await;
    insert_helper(
        &resolver,
        "child",
        doc().set("cid", 1_i64).set("parent_id", 5_i64),
    )
    .await;

    // Supporting index on the FK column → F-53c fast-path engages.
    let child_table = resolver.db.get_table("default", "child").await.unwrap();
    child_table
        .create_index("idx_child_parent_id_site4", &["parent_id"])
        .await
        .unwrap();

    arm_failure_for_all_rows(&resolver, "child").await;

    let mut b = Batch::new();
    b.id(1);
    b.update(
        "upd",
        write::update("parent")
            .where_(filter::eq("id", 5_i64))
            .set(doc().set("id", 99_i64)),
    );
    let result = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test").await;

    assert!(
        result.is_err(),
        "a genuine read_one_tx_bytes error during the ON UPDATE index \
         fast-path re-read must abort the whole update (return Err), not \
         silently continue past the poisoned candidate. Got: {result:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Sites 2 + 3 — `fk_actions.rs` grandchild recursion (`plan_cascade_for_ids`).
//
// ## Why the ORIGINAL single self-referential test was an invalid oracle
//
// A prior version of this coverage used a single 3-level SELF-referential
// hierarchy (`employees.manager_id -> employees.id`, CEO <- Mgr <- Worker)
// and armed every row of the ONE `employees` table. That construction is
// broken twice over:
//
// 1. `discover_action_refs` unconditionally filters out self-referential
//    CASCADE refs (`fk_actions.rs`, the `is_self_ref` check) — self-ref
//    CASCADE is rejected at DDL time and treated as a no-op defense-in-depth
//    here, so `action_refs` for the `employees` table was ALWAYS empty.
//    `plan_cascade_recursive` returned `Ok(())` immediately: no cascade ever
//    ran, and sites 1/2/3 were never even entered.
// 2. Because "arm every row in `employees`" also armed the id of the row
//    the top-level DELETE itself targets (the CEO), the observed `Err` came
//    from `TableManager::delete_tx`'s OWN internal `read_one_tx_bytes` read
//    of the row being deleted — a completely unrelated code path, not any
//    of `fk_actions.rs`'s three re-read sites. `result.is_err()` was true
//    for a reason that had nothing to do with what the test's name and
//    module doc claimed to prove.
//
// A genuine multi-level CASCADE recursion (so sites 2/3 are reachable at
// all) needs a REAL parent -> child -> grandchild chain across three
// DISTINCT tables (`a -> b -> c`, mirroring `fk_actions_tests::
// cascade_chain_a_to_b_to_c`) — self-referential CASCADE never recurses.
//
// ## Discrimination method: per-table arming + per-site message assertion
//
// With `a -> b -> c` (each FK `Cascade`), deleting `a`'s row walks:
//   - `plan_cascade_recursive` (depth 0, parent table `a`): direct-child
//     scan/fast-path over `b`. Site 1 re-reads `b`'s row ONLY if there is a
//     supporting index on `b`'s FK field — no index there means this level
//     falls back to `list_stream_tx` + `classify_row`, which never calls
//     `read_one_tx_bytes` at all.
//   - `plan_cascade_for_ids` (depth 1, cascaded parent table `b`): site 3
//     re-reads EACH id in `parent_ids` (here, `b`'s single cascaded row) to
//     collect its `id` ref-field value — BEFORE the by-child-table loop
//     that contains site 2.
//   - Still inside `plan_cascade_for_ids`: site 2 re-reads the grandchild
//     (`c`) index-fast-path candidates selected from that collected value.
//
// Both tests below therefore leave NO index on `b`'s FK field (`a_id`) —
// this forces the direct-child level to use the scan fallback, so site 1
// NEVER calls `read_one_tx_bytes` and can never consume an arm meant for
// site 2/3. Site 3 always runs before site 2 for the SAME id (`b`'s row),
// so arming `b` isolates site 3 (it fails before site 2's loop is ever
// reached) and arming ONLY `c` (leaving `b` unarmed) isolates site 2 (site
// 3 reads `b` successfully, then site 2's re-read of `c` fails). Each test
// additionally asserts the returned error message contains the site's
// verbatim, already-site-specific string (`fk_actions.rs`'s
// `"...grandchild index fast-path re-read failed..."` for site 2,
// `"...grandchild ref_field collection re-read failed..."` for site 3) —
// combining per-id arming with message assertion for the strongest proof
// that THIS test failed at THIS site, not merely that something, somewhere,
// returned `Err`.
// ═══════════════════════════════════════════════════════════════════════════

/// Shared 3-table `a -> b -> c` CASCADE chain setup for the site 2 / site 3
/// grandchild-recursion tests. Returns the resolver with `a(1)`, `b(2, a_id=1)`,
/// `c(3, b_id=2)` inserted. Deliberately creates NO index on `b`'s FK field
/// (`a_id`) — see the module-doc block above for why that is load-bearing
/// (it forces the direct-child level to the scan fallback, so site 1 never
/// fires and can never consume an arm meant for site 2/3).
async fn setup_grandchild_cascade_chain() -> FkTestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![
            TableConfig::new("a"),
            TableConfig::new("b"),
            TableConfig::new("c"),
        ],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    // b.a_id -> a.id ON DELETE CASCADE.
    bind_fk_validator(
        &db,
        &registry,
        "b",
        "b_fk_a_cascade_read_err",
        "a_id",
        "a",
        "id",
        FkAction::Cascade,
        true,
    );
    // c.b_id -> b.id ON DELETE CASCADE.
    bind_fk_validator(
        &db,
        &registry,
        "c",
        "c_fk_b_cascade_read_err",
        "b_id",
        "b",
        "id",
        FkAction::Cascade,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_helper(&resolver, "a", doc().set("id", 1_i64)).await;
    insert_helper(&resolver, "b", doc().set("id", 2_i64).set("a_id", 1_i64)).await;
    insert_helper(&resolver, "c", doc().set("id", 3_i64).set("b_id", 2_i64)).await;

    resolver
}

/// Delete `a`'s single row, the trigger for the whole cascade chain.
async fn delete_a(resolver: &FkTestResolver) -> Result<BatchResponse, BatchError> {
    let mut b = Batch::new();
    b.id(4);
    b.delete("del_a", write::delete("a").where_(filter::eq("id", 1_i64)));
    execute_batch(&b.build(), resolver, None, None, Actor::System, "test").await
}

#[tokio::test]
async fn cascade_grandchild_index_fast_path_propagates_read_error() {
    let resolver = setup_grandchild_cascade_chain().await;

    // Supporting index on c's FK field only -> site 2's fast path engages
    // for the grandchild level. No index on b's FK field (see module doc) ->
    // site 1 never runs, so it can't consume an arm meant for site 2/3.
    let c_table = resolver.db.get_table("default", "c").await.unwrap();
    c_table
        .create_index("idx_c_b_id_site2", &["b_id"])
        .await
        .unwrap();

    // Arm ONLY c's row. b's row (read by site 3 first) is left unarmed, so
    // site 3 succeeds and control reaches site 2's re-read of c, which then
    // hits the injected failure.
    arm_failure_for_all_rows(&resolver, "c").await;

    let result = delete_a(&resolver).await;

    let err = result.expect_err(
        "a genuine read_one_tx_bytes error during the grandchild index \
         fast-path re-read (site 2) must abort the whole delete (Err), not \
         silently continue past the poisoned candidate",
    );
    let message = err.to_string();
    assert!(
        message.contains("grandchild index fast-path re-read failed"),
        "expected the site-2-specific error message, got: {message}"
    );
}

#[tokio::test]
async fn cascade_grandchild_ref_field_collection_propagates_read_error() {
    let resolver = setup_grandchild_cascade_chain().await;

    // Arm ONLY b's row. Site 3 (`plan_cascade_for_ids`'s ref-field
    // collection loop) re-reads b's row BEFORE site 2's by-child-table loop
    // ever runs, so this must fail at site 3 specifically — site 2 (c's
    // index fast path, even if an index existed) is never reached. No index
    // on b's FK field (see module doc) -> site 1 never runs either, so it
    // can't consume this arm before site 3 does.
    arm_failure_for_all_rows(&resolver, "b").await;

    let result = delete_a(&resolver).await;

    let err = result.expect_err(
        "a genuine read_one_tx_bytes error during the grandchild ref_field \
         collection re-read (site 3) must abort the whole delete (Err), not \
         silently continue past the poisoned row",
    );
    let message = err.to_string();
    assert!(
        message.contains("grandchild ref_field collection re-read failed"),
        "expected the site-3-specific error message, got: {message}"
    );
}
