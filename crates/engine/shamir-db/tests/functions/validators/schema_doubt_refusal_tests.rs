//! Integration test of the two per-rule wire schema DDL handlers'
//! (`handle_add_schema_rule` / `handle_remove_schema_rule`) in-doubt refusal:
//! both must refuse an IN-DOUBT table BEFORE activation, leaving the catalogue
//! as it was. Hosted with the validator end-to-end tests, since the doubt state
//! is raised through the validator registry.
//!
//! Both handlers persist the new schema to the catalogue and only then call
//! `compile_table_schema_admitted`, whose FIRST statement is the
//! in-doubt refusal (`schema_management.rs:636`). The catalogue write is
//! rolled back on the resulting error, so the refusal's own error text is
//! the only externally visible evidence that the check ran.
//!
//! The doubt state is raised directly on the table's already-open manager
//! (`mark_validator_bindings_in_doubt`, available only with the `test-util`
//! feature), exactly as the sibling `binding_in_doubt_refusals.rs` does; the
//! production producer is a binding flush that could not be compensated and
//! needs a fault-injection store, which would test the fault rather than the
//! refusal.
//!
//! The registry is made to DISAGREE with the catalogue before each case: the
//! catalogue still holds the schema validator id `X`, while the canonical
//! name `__schema__/d/main/users` is registered under a different id. That is
//! the stale-name-collision shape — with the in-doubt check DELETED, the
//! admitted entry proceeds to `replace_artifact` with the stale id `X`,
//! `replace_artifact` returns `false`, and the handler reports "stale name
//! collision" instead. The catalogue rollback still restores the record, so
//! without the check the message assertions below fail while the
//! "schema is unchanged" assertion keeps passing — which is precisely why
//! those assertions are not vacuous.

use std::future::Future;
use std::time::Duration;

use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

// ═══════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════

const DB: &str = "d";
const REPO: &str = "main";
const TABLE: &str = "users";

/// `schema_validator_name(DB, REPO, TABLE)`, reconstructed literally: the
/// helper lives in the crate-private `db::schema_management` module and is
/// not reachable from an integration target.
const SCHEMA_NAME: &str = "__schema__/d/main/users";

/// Catalogue field carrying the persisted schema validator id.
const SCHEMA_VALIDATOR_ID_FIELD: &str = "schema_validator_id";

/// Every await in this file is bounded by this budget.
const BUDGET: Duration = Duration::from_secs(120);

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

/// Bound a single await by [`BUDGET`]; panics on timeout.
async fn within_budget<F: Future>(fut: F) -> F::Output {
    tokio::time::timeout(BUDGET, fut)
        .await
        .expect("operation exceeded the 120s budget")
}

fn request(batch: &Batch) -> shamir_db::query::batch::BatchRequest {
    batch
        .to_request_via_msgpack()
        .expect("batch request encoding")
}

/// Build the shared fixture:
///
/// 1. in-memory `ShamirDb`, db `d`, repo `main` with table `users` (wire DDL);
/// 2. one `SET TABLE SCHEMA` batch so the catalogue carries a
///    `schema_validator_id` and the rule list is non-empty;
/// 3. make the registry disagree with the catalogue (stale-name shape);
/// 4. open the table and raise its in-doubt flag.
async fn fixture() -> Fixture {
    let db = within_budget(ShamirDb::init_memory()).await.unwrap();
    within_budget(db.create_db(DB)).await.expect("create_db");

    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo(REPO).tables([TABLE]));
    within_budget(db.execute(DB, &request(&b)))
        .await
        .expect("create_repo must succeed");

    let mut b = Batch::new();
    b.id(2);
    b.set_table_schema(
        "ss",
        ddl::set_table_schema(TABLE).rules([ddl::field(["age"]).int().min(0).max(150).build()]),
    );
    let resp = within_budget(db.execute(DB, &request(&b)))
        .await
        .expect("set_table_schema must succeed");
    let r = resp.results["ss"].records[0].as_value().as_ref().clone();
    assert_eq!(r.get("ok").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(r.get("schema_version").and_then(|v| v.as_i64()), Some(1));

    // 3. Capture the catalogue's schema validator id and re-register the SAME
    //    artifact under a different id, so `id_for_name` no longer resolves
    //    to the catalogue's id.
    let schema_validator_id = db
        .validators()
        .id_for_name(SCHEMA_NAME)
        .expect("the set_table_schema batch must register a schema validator");
    let artifact = db
        .validators()
        .get_by_id(&schema_validator_id)
        .expect("the registered schema validator must have an artifact");
    assert!(
        db.validators().remove(&schema_validator_id),
        "the captured schema validator must exist"
    );
    db.validators()
        .register(RecordId::system("doubt_stale"), SCHEMA_NAME, artifact)
        .expect("re-registering the artifact under a new id must succeed");
    assert_ne!(
        db.validators().id_for_name(SCHEMA_NAME),
        Some(schema_validator_id),
        "the registry must disagree with the catalogue's schema_validator_id"
    );

    // 4. Raise the in-doubt flag on the table's live manager.
    let table = within_budget(db.get_table(DB, REPO, TABLE))
        .await
        .expect("the table must open");
    table.mark_validator_bindings_in_doubt();
    assert!(
        table.validator_bindings_in_doubt(),
        "the fixture must leave the table in doubt"
    );

    Fixture {
        db,
        schema_validator_id,
    }
}

struct Fixture {
    db: ShamirDb,
    schema_validator_id: RecordId,
}

/// Re-open the table and assert the in-doubt flag survived the refused DDL.
async fn assert_still_in_doubt(db: &ShamirDb) {
    let table = within_budget(db.get_table(DB, REPO, TABLE))
        .await
        .expect("the table must still open");
    assert!(
        table.validator_bindings_in_doubt(),
        "the in-doubt flag must survive the refused DDL"
    );
}

/// The catalogue observables the refused DDL must not change.
#[derive(Debug, PartialEq, Eq)]
struct CatalogueSnapshot {
    schema_version: i64,
    rule_paths: Vec<String>,
    schema_validator_id: String,
}

/// Read the persisted schema through the public wire read
/// (`GET TABLE SCHEMA`) plus the catalogue record's validator id.
async fn catalogue_snapshot(db: &ShamirDb) -> CatalogueSnapshot {
    let mut b = Batch::new();
    b.id(90);
    b.get_table_schema("gs", ddl::get_table_schema(TABLE));
    let resp = within_budget(db.execute(DB, &request(&b)))
        .await
        .expect("get_table_schema must succeed");
    let r = resp.results["gs"].records[0].as_value().as_ref().clone();

    let schema_version = r
        .get("schema_version")
        .and_then(|v| v.as_i64())
        .expect("schema_version must be present");
    let mut rule_paths: Vec<String> = r
        .get("schema")
        .and_then(|v| v.as_array())
        .expect("schema must be an array")
        .iter()
        .map(|rule| {
            rule.get("path")
                .and_then(|p| p.as_array())
                .expect("every rule must carry a path")
                .iter()
                .map(|seg| seg.as_str().unwrap_or("<non-str>").to_string())
                .collect::<Vec<_>>()
                .join(".")
        })
        .collect();
    rule_paths.sort();

    let rec: QueryValue = within_budget(db.system_store().load_table_record(DB, REPO, TABLE))
        .await
        .expect("load_table_record must succeed")
        .expect("the table catalogue record must exist");
    let schema_validator_id = rec
        .get(SCHEMA_VALIDATOR_ID_FIELD)
        .and_then(|v| v.as_str())
        .expect("the catalogue must carry a schema_validator_id")
        .to_string();

    CatalogueSnapshot {
        schema_version,
        rule_paths,
        schema_validator_id,
    }
}

/// Execute a schema-DDL batch and return the error it must produce.
async fn expect_batch_error(db: &ShamirDb, batch: &Batch) -> String {
    match within_budget(db.execute(DB, &request(batch))).await {
        Ok(_) => panic!("the batch must be refused on an in-doubt table"),
        Err(e) => e.to_string(),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// Test A: ADD SCHEMA RULE refuses an in-doubt table
// ═══════════════════════════════════════════════════════════════════════

/// Kills the mutant that deletes the in-doubt check at
/// `schema_management.rs:636`. PASSES on the parent commit, which already
/// contains that check — this is a regression oracle for the mutant, not for
/// a parent defect.
///
/// Why the assertions are not vacuous: the catalogue write happens BEFORE
/// `compile_table_schema_admitted`, and the caller rolls it back on error. So
/// the "schema is unchanged" assertion holds with or without the check. What
/// distinguishes the two is the error text: with the check, activation stops
/// on the doubt refusal; without it, the admitted entry reaches
/// `replace_artifact` with the catalogue's stale schema validator id (the
/// registry maps the canonical name to a different id), that call returns
/// `false`, and the handler reports "stale name collision" instead. Hence the
/// "in doubt" / "not stale name collision" assertions fail only when the
/// check is gone.
#[tokio::test]
async fn add_schema_rule_refuses_an_in_doubt_table_before_activating_or_replacing_the_artifact() {
    let fx = fixture().await;
    let before = catalogue_snapshot(&fx.db).await;
    assert_eq!(before.schema_version, 1);
    assert_eq!(before.rule_paths, vec!["age".to_string()]);
    assert_eq!(
        before.schema_validator_id,
        fx.schema_validator_id.to_string()
    );

    let mut b = Batch::new();
    b.id(10);
    b.try_add_schema_rule(
        "ar",
        ddl::add_schema_rule(TABLE).rule(ddl::field(["email"]).string().required()),
    )
    .expect("add-schema-rule builder is complete");
    let message = expect_batch_error(&fx.db, &b).await;
    assert!(
        message.contains("in doubt"),
        "the refusal must name the in-doubt state, got: {message}"
    );
    assert!(
        !message.contains("stale name collision"),
        "the in-doubt check must run before the stale-name collision check, got: {message}"
    );

    let after = catalogue_snapshot(&fx.db).await;
    assert_eq!(
        after, before,
        "the catalogue must be unchanged after the refused ADD SCHEMA RULE"
    );
    assert!(
        !after.rule_paths.iter().any(|p| p == "email"),
        "the new rule must not survive the refusal: {:?}",
        after.rule_paths
    );
    assert_still_in_doubt(&fx.db).await;
}

// ═══════════════════════════════════════════════════════════════════════
// Test B: REMOVE SCHEMA RULE refuses an in-doubt table
// ═══════════════════════════════════════════════════════════════════════

/// Kills the mutant that deletes the in-doubt check at
/// `schema_management.rs:636`. PASSES on the parent commit, which already
/// contains that check — this is a regression oracle for the mutant, not for
/// a parent defect.
///
/// Why the assertions are not vacuous: `handle_remove_schema_rule` writes the
/// shortened rule list to the catalogue and only then calls
/// `compile_table_schema_admitted`, and the caller rolls the record back on
/// error — so the "rule still present / schema unchanged" assertion passes
/// with or without the check. With the check, activation stops on the doubt
/// refusal; without it, the admitted entry reaches `replace_artifact` with the
/// catalogue's stale schema validator id (the registry maps the canonical
/// name to a different id), that call returns `false`, and the handler
/// reports "stale name collision" instead.
#[tokio::test]
async fn remove_schema_rule_refuses_an_in_doubt_table_before_activating_or_replacing_the_artifact()
{
    let fx = fixture().await;
    let before = catalogue_snapshot(&fx.db).await;
    assert_eq!(before.schema_version, 1);
    assert_eq!(before.rule_paths, vec!["age".to_string()]);
    assert_eq!(
        before.schema_validator_id,
        fx.schema_validator_id.to_string()
    );

    let mut b = Batch::new();
    b.id(20);
    b.remove_schema_rule("rr", ddl::remove_schema_rule(TABLE, ["age"]));
    let message = expect_batch_error(&fx.db, &b).await;
    assert!(
        message.contains("in doubt"),
        "the refusal must name the in-doubt state, got: {message}"
    );
    assert!(
        !message.contains("stale name collision"),
        "the in-doubt check must run before the stale-name collision check, got: {message}"
    );

    let after = catalogue_snapshot(&fx.db).await;
    assert_eq!(
        after, before,
        "the catalogue must be unchanged after the refused REMOVE SCHEMA RULE"
    );
    assert!(
        after.rule_paths.iter().any(|p| p == "age"),
        "the removed rule must still be present: {:?}",
        after.rule_paths
    );
    assert_still_in_doubt(&fx.db).await;
}
