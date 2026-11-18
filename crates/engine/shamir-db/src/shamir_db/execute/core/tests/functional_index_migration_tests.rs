//! CI-only server tests for R1c functional-index MIGRATION admission.
//!
//! Admission is exactly `lower | upper | trim | length`, no
//! `functional_args`: the backend builds `IndexExpr` from the indexed field
//! alone (`write_domain_index_admitted.rs`), so anything else would strand
//! the journaled intent on a DDL failure after `started`.
//!
//! Coverage: one migration with all four admitted ops applies through the
//! REAL apply path over a persistent (Fjall) journal, ends `applied` with no
//! unresolved entry, and computed-equality queries serve through the
//! functional backends (`stats.index_used == Some("index2")`) with the exact
//! rows (`length` counts UTF-8 BYTES: `héllo` = 6). Every refused form is
//! refused by plan AND apply — validation precedes the checksum comparison
//! in each admission order, proven by a deliberately wrong checksum
//! surfacing the VALIDATION diagnostic — and the journal holds no event
//! after any refusal. Refusal DTOs are produced by mutating a builder-built
//! VALID body at the trust boundary (the builder refuses these forms at
//! `build()`; the server independently re-validates and refuses them).

use crate::access::Actor;
use crate::engine::repo::{BoxRepoFactory, RepoConfig};
use crate::query::batch::{BatchError, BatchResponse};
use crate::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::{
    application_schema_plan, apply_application_schema_migration, ddl, doc, filter::computed,
    write::insert, FilterValue, Query, SchemaMigration,
};
use shamir_query_types::admin::types::schema_migration::{
    canonical_body_checksum, SchemaMigrationApplyRequest, SchemaMigrationBody, SchemaMigrationOp,
    SchemaMigrationPlanRequest, SchemaMigrationPredecessor, SchemaMigrationSubmission,
    SchemaMigrationTarget,
};
use shamir_query_types::admin::CreateIndexOp;
use shamir_query_types::batch::BatchOp;

use super::super::admin_dispatch::ShamirAdminExecutor;

/// Hard cap on every await in these tests (nextest's own kill is 180s; a
/// handler that never settles fails the test well before that).
const AWAIT_CAP: std::time::Duration = std::time::Duration::from_secs(30);

/// Bound ANY await (no fixed sleeps — only a failure deadline), boxed so the
/// awaited state stays off the test stack.
async fn bounded<T>(fut: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(AWAIT_CAP, Box::pin(fut))
        .await
        .expect("bounded await deadline exceeded")
}

/// The exact refusal reason strings of `SchemaMigrationOp::validate`
/// (`operation.rs`). `SchemaMigrationError::UnsafeOperation` renders them as
/// `unsafe application schema migration operation: {reason}`.
const ONLY_VALID_FOR_FUNCTIONAL: &str =
    "functional_op/functional_args are only valid for 'functional' indexes";
const OUTSIDE_ALLOWLIST: &str = "index function is outside the built-in functional-index allowlist";
const EXPLICIT_OP_REQUIRED: &str = "functional index requires an explicit built-in operation";
const ARGS_NOT_ADMITTED: &str =
    "functional_args are not admitted because functional indexes do not use them";

/// Names the OLD allowlist admitted and the backend cannot build from a bare
/// name: `write_domain_index_admitted.rs` handles only lower/upper/trim/
/// length directly and sends everything else to the user-scalar arm
/// (`resolver.get(bare_name)` misses library-qualified registrations).
const UNBUILDABLE_OPS: [&str; 4] = ["substring", "mod", "coalesce", "concat"];

// ── private harness (the contract-suite helpers are not visible here) ─────

/// Run a test body on a roomy thread with its own current-thread runtime:
/// the apply runner nests the DDL handlers' async frames deeper than a debug
/// test thread's default stack allows (the `on_big_stack` pattern of the
/// schema-migration contract suites).
fn on_big_stack<F, Fut>(body: F)
where
    F: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()>,
{
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(move || {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("test runtime")
                .block_on(body())
        })
        .expect("spawn test thread")
        .join()
        .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
}

/// Persistent Fjall system store (the journal needs durability, never
/// `init_memory`) plus `testdb` / repo `main` / table `posts`. The `TempDir`
/// is returned so it outlives the database handle.
async fn persistent_db() -> (tempfile::TempDir, ShamirDb) {
    let dir = tempfile::tempdir().unwrap();
    let db = bounded(ShamirDb::init(
        crate::shamir_db::system_store::SystemStoreConfig::Fjall(dir.path().join("system")),
    ))
    .await
    .unwrap();
    bounded(db.create_db("testdb")).await.unwrap();
    // The repo store must be durable Fjall too (not `in_memory`), and the
    // directory must outlive the database handle — the returned TempDir
    // owns both.
    bounded(db.add_repo(
        "testdb",
        RepoConfig::new("main", BoxRepoFactory::fjall(dir.path().join("main"))),
    ))
    .await
    .unwrap();
    bounded(db.add_table("testdb", "main", "posts", true))
        .await
        .unwrap();
    (dir, db)
}

/// The `System` actor bypasses `authorize_access`, so the concrete handlers
/// are exercised without re-testing the §5.2 authorization ladder (the
/// contract suites own that coverage).
fn executor(db: &ShamirDb) -> ShamirAdminExecutor {
    ShamirAdminExecutor {
        shamir: db.clone(),
        db_name: "testdb".into(),
        actor: Actor::System,
        repo_admin_observer: None,
    }
}

/// The serving database's canonical migration identity.
async fn target_of(db: &ShamirDb) -> SchemaMigrationTarget {
    let identity = bounded(db.system_store().schema_migration_identity("testdb"))
        .await
        .unwrap();
    SchemaMigrationTarget {
        database_identity: identity.0,
    }
}

fn checksum(body: &SchemaMigrationBody) -> [u8; 32] {
    canonical_body_checksum(body).expect("test migration body canonicalizes")
}

/// A builder-built admission body carrying the given index operation(s);
/// the builder refuses any inadmissible form, so refusals are produced by
/// MUTATING the typed DTO of this valid body afterwards.
fn index_body(id: &str, ordinal: u64, op: BatchOp) -> SchemaMigrationBody {
    SchemaMigration::new(id, ordinal, format!("migration {ordinal}"))
        .op(op)
        .expect("typed create-index operation is admitted")
        .build()
        .expect("test migration body is valid")
}

/// The ONE concrete-handler call site: run a builder-produced `BatchOp`
/// (plan or apply envelope) through the boxed handler under the bounded
/// deadline, dropping the payload.
async fn run_admin_op(exec: &ShamirAdminExecutor, op: BatchOp) -> Result<(), BatchError> {
    bounded(async {
        Box::pin(exec.handle_application_schema(&op))
            .await
            .map(|_| ())
    })
    .await
}

fn plan_request(
    target: SchemaMigrationTarget,
    body: &SchemaMigrationBody,
    checksum: [u8; 32],
) -> SchemaMigrationPlanRequest {
    SchemaMigrationPlanRequest {
        protocol_version: 1,
        target,
        expected_predecessor: SchemaMigrationPredecessor {
            migration_id: None,
            ordinal: 0,
            checksum: [0; 32],
        },
        migrations: vec![SchemaMigrationSubmission::new(body, checksum)],
    }
}

fn apply_request(
    target: SchemaMigrationTarget,
    body: &SchemaMigrationBody,
    checksum: [u8; 32],
) -> SchemaMigrationApplyRequest {
    SchemaMigrationApplyRequest {
        protocol_version: 1,
        target,
        expected_predecessor: SchemaMigrationPredecessor {
            migration_id: None,
            ordinal: 0,
            checksum: [0; 32],
        },
        expected_fence_generation: 0,
        migration: SchemaMigrationSubmission::new(body, checksum),
    }
}

/// Trust-boundary probe: submit the SAME mutated body to plan AND to apply
/// with a deliberately wrong checksum and assert BOTH refusals carry the
/// exact validation diagnostic (never `schema_migration_checksum_mismatch`
/// — validation precedes checksum in the apply admission order) and that the
/// journal stayed completely empty.
async fn assert_refused_by_plan_and_apply(
    db: &ShamirDb,
    exec: &ShamirAdminExecutor,
    target: SchemaMigrationTarget,
    mutated: &SchemaMigrationBody,
    reason: &'static str,
) {
    let diagnostic = format!("unsafe application schema migration operation: {reason}");

    // Both arms get the deliberately wrong checksum: validation precedes the
    // checksum comparison in each admission order, so the refusal must carry
    // the VALIDATION diagnostic, never `checksum_mismatch`.
    let plan_error = run_admin_op(
        exec,
        application_schema_plan(plan_request(target, mutated, [9; 32])),
    )
    .await
    .expect_err("plan must refuse the inadmissible index form");
    assert_diagnostic(&plan_error, &diagnostic);

    let apply_error = run_admin_op(
        exec,
        apply_application_schema_migration(apply_request(target, mutated, [9; 32])),
    )
    .await
    .expect_err("apply must refuse the inadmissible index form");
    assert_diagnostic(&apply_error, &diagnostic);

    assert_journal_empty(db).await;
}

/// The refusal is a plain validation error: destructured EXACTLY — code
/// `None` (not a §5.3 coded refusal and not
/// `schema_migration_checksum_mismatch`) and the message equals the exact
/// model diagnostic.
fn assert_diagnostic(error: &BatchError, diagnostic: &str) {
    let BatchError::QueryError {
        alias: _,
        message,
        code,
    } = error
    else {
        panic!("expected a QueryError refusal, got {error:?}");
    };
    assert!(
        code.is_none(),
        "validation refusals are uncoded; got code {code:?}"
    );
    assert_eq!(message, diagnostic, "exact validation diagnostic expected");
}

/// The journal invariant after a refusal: NO applied entry and NO unresolved
/// (in-doubt / in-progress) entry — nothing was journaled.
async fn assert_journal_empty(db: &ShamirDb) {
    let history = migration_history(db).await;
    assert!(
        history.events.is_empty(),
        "a refusal must not journal ANY event; got {} events (applied {}, unresolved {})",
        history.events.len(),
        history.applied.len(),
        history.unresolved.len(),
    );
}

/// The journal invariant after a successful apply: exactly one applied
/// migration and no unresolved entry.
async fn assert_journal_applied_once(db: &ShamirDb, id: &str) {
    let history = migration_history(db).await;
    assert_eq!(history.applied.len(), 1, "exactly one applied migration");
    assert_eq!(history.applied[0].migration.id, id);
    assert!(
        history.unresolved.is_empty(),
        "no unresolved entry may survive an applied migration"
    );
}

async fn migration_history(db: &ShamirDb) -> crate::shamir_db::system_store::MigrationHistory {
    let serving = bounded(db.system_store().schema_migration_identity("testdb"))
        .await
        .unwrap();
    bounded(db.system_store().read_schema_migration_history(serving))
        .await
        .expect("journal history is readable")
}

/// Preinsert the mixed string rows through the NORMAL typed batch write path
/// (data DML is never admitted inside a migration body).
async fn preinsert_rows(db: &ShamirDb) {
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "w1",
        insert("posts").row(doc! {
            "name" => "Alice@FOO.com",
            "loud" => "école",
            "pad" => "  hi  ",
            "label" => "héllo",
        }),
    );
    b.insert(
        "w2",
        insert("posts").row(doc! {
            "name" => "BOB@bar.org",
            "loud" => "quiet",
            "pad" => "padded",
            "label" => "xyz",
        }),
    );
    let request = b.to_request_via_msgpack().expect("batch request encoding");
    bounded(db.execute_as(Actor::System, "testdb", &request))
        .await
        .expect("typed insert batch succeeds");
}

/// One computed-equality query on the normal typed read path.
async fn query_computed(
    db: &ShamirDb,
    expr_op: &str,
    field: &str,
    value: FilterValue,
) -> BatchResponse {
    let query = Query::from("posts").where_(computed(expr_op, field, "eq", value));
    let mut b = Batch::new();
    b.id(2);
    b.query("q", query);
    let request = b.to_request_via_msgpack().expect("batch request encoding");
    bounded(db.execute_as(Actor::System, "testdb", &request))
        .await
        .expect("computed-equality query succeeds")
}

fn assert_served_by_functional_index(response: &BatchResponse, field: &str, expected: &str) {
    let result = &response.results["q"];
    let records = &result.records;
    assert_eq!(records.len(), 1, "expected exactly one row for {field}");
    assert_eq!(
        records[0].get_value_str(field),
        Some(expected),
        "exact row data for {field}"
    );
    let stats = result.stats.as_ref().expect("query stats are present");
    assert_eq!(
        stats.index_used.as_deref(),
        Some("index2"),
        "the query must be served by the functional (index2) backend"
    );
}

/// Mutated variant of the valid baseline: `mutate` edits the typed
/// `CreateIndexOp` DTO at the trust boundary (FnOnce per variant).
fn mutated_with(
    base: &SchemaMigrationBody,
    mutate: impl FnOnce(&mut CreateIndexOp),
) -> SchemaMigrationBody {
    let mut body = base.clone();
    let Some(SchemaMigrationOp::CreateIndex(op)) = body.operations.first_mut() else {
        panic!("test body carries a create-index operation");
    };
    mutate(op);
    body
}

fn int_arg() -> shamir_types::types::value::QueryValue {
    shamir_types::types::value::QueryValue::Int(1)
}

// ── tests ─────────────────────────────────────────────────────────────────

/// The positive proof: ONE migration containing all four admitted functional
/// ops (`lower`, `upper`, `trim`, `length`, each on its OWN field — the
/// planner leases one functional backend per (kind, path) pair) applies
/// through the real path over the persistent journal, and every
/// computed-equality query is served by the functional backend with the
/// exact expected row.
#[test]
fn migration_applies_all_four_admitted_ops_and_serves_lookups() {
    on_big_stack(|| async move {
        let (_dir, db) = persistent_db().await;
        let exec = executor(&db);
        let target = target_of(&db).await;
        preinsert_rows(&db).await;

        // Four create_index ops, distinct field per kind so each (kind,
        // path) lease is unique on the shared `posts` table.
        let ops = [
            ("idx_posts_lower", "name", "lower"),
            ("idx_posts_upper", "loud", "upper"),
            ("idx_posts_trim", "pad", "trim"),
            ("idx_posts_length", "label", "length"),
        ]
        .into_iter()
        .map(|(name, field, kind)| {
            ddl::create_index(name, "posts")
                .field(field)
                .index_type("functional")
                .functional_op(kind)
                .build()
        })
        .collect::<Vec<BatchOp>>();
        let body = {
            let mut builder = SchemaMigration::new("func-001", 1, "functional indexes");
            for op in ops {
                builder = builder
                    .op(op)
                    .expect("all four admitted ops pass admission");
            }
            builder.build().expect("the four-op body is valid")
        };

        // The plan arm admits the whole body (read-only; writes nothing).
        run_admin_op(
            &exec,
            application_schema_plan(plan_request(target, &body, checksum(&body))),
        )
        .await
        .expect("the all-four-op body is plannable");
        assert_journal_empty(&db).await;

        // The REAL apply path: journaled, fenced, DDL executed, applied.
        run_admin_op(
            &exec,
            apply_application_schema_migration(apply_request(target, &body, checksum(&body))),
        )
        .await
        .expect("the all-four-op migration applies");
        assert_journal_applied_once(&db, "func-001").await;

        // `length` counts UTF-8 bytes: "héllo" is 6 bytes (é = 2), "xyz" 3.
        assert_served_by_functional_index(
            &query_computed(&db, "lower", "name", "alice@foo.com".into()).await,
            "name",
            "Alice@FOO.com",
        );
        assert_served_by_functional_index(
            &query_computed(&db, "upper", "loud", "ÉCOLE".into()).await,
            "loud",
            "école",
        );
        assert_served_by_functional_index(
            &query_computed(&db, "trim", "pad", "hi".into()).await,
            "pad",
            "  hi  ",
        );
        assert_served_by_functional_index(
            &query_computed(&db, "length", "label", 6_i64.into()).await,
            "label",
            "héllo",
        );
    });
}

/// Every refused form is refused by plan AND apply (with a dummy checksum —
/// validation precedes checksum) with the EXACT model diagnostic, and no
/// journal row or event survives any refusal.
#[test]
fn every_refused_form_is_refused_before_checksum_and_before_journaling() {
    on_big_stack(|| async move {
        let (_dir, db) = persistent_db().await;
        let exec = executor(&db);
        let target = target_of(&db).await;

        // Valid baseline: builder-built functional `lower` index body.
        let valid = index_body(
            "func-100",
            1,
            ddl::create_index("idx_ok", "posts")
                .field("name")
                .index_type("functional")
                .functional_op("lower")
                .build(),
        );
        // Positive control: the valid body passes the model validation the
        // handlers call (the builder already proved the same thing).
        valid.validate().expect("the baseline body is admitted");

        // ── wrong family / omitted family (functional_op present) ─────
        for index_type in [
            Some("btree".to_string()),
            Some("fts".to_string()),
            Some("vector".to_string()),
            None,
        ] {
            let body = mutated_with(&valid, |op| op.index_type = index_type);
            assert_refused_by_plan_and_apply(&db, &exec, target, &body, ONLY_VALID_FOR_FUNCTIONAL)
                .await;
        }

        // ── args-only on a non-functional index ───────────────────────
        let body = mutated_with(&valid, |op| {
            op.index_type = Some("btree".to_string());
            op.functional_op = None;
            op.functional_args = Some(vec![int_arg()]);
        });
        assert_refused_by_plan_and_apply(&db, &exec, target, &body, ONLY_VALID_FOR_FUNCTIONAL)
            .await;

        // ── missing functional_op: with args and without ──────────────
        for with_args in [false, true] {
            let body = mutated_with(&valid, |op| {
                op.functional_op = None;
                op.functional_args = with_args.then(|| vec![int_arg()]);
            });
            assert_refused_by_plan_and_apply(&db, &exec, target, &body, EXPLICIT_OP_REQUIRED).await;
        }

        // ── unbuildable ops (substring/mod/coalesce/concat), with AND
        //    without args — the allowlist check precedes the args check ──
        for name in UNBUILDABLE_OPS {
            for with_args in [false, true] {
                let body = mutated_with(&valid, |op| {
                    op.functional_op = Some(name.to_string());
                    op.functional_args = with_args.then(|| vec![int_arg()]);
                });
                assert_refused_by_plan_and_apply(&db, &exec, target, &body, OUTSIDE_ALLOWLIST)
                    .await;
            }
        }

        // ── args refused for EVERY admitted op, empty and nonempty ────
        for admitted in ["lower", "upper", "trim", "length"] {
            for args in [
                Vec::new(),
                vec![shamir_types::types::value::QueryValue::Str("x".into())],
            ] {
                let body = mutated_with(&valid, |op| {
                    op.functional_op = Some(admitted.to_string());
                    op.functional_args = Some(args);
                });
                assert_refused_by_plan_and_apply(&db, &exec, target, &body, ARGS_NOT_ADMITTED)
                    .await;
            }
        }
    });
}
