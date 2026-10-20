//! End-to-end test for `DescribeTable` (Phase E.6).
//!
//! Creates a table with a schema, an index, and custom retention, then runs
//! `describe_table` and asserts every section is present and correct.

use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::ddl::Retention;
use shamir_types::types::value::QueryValue;

// ═══════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════

/// Execute a batch and return the response.
async fn exec(db: &ShamirDb, db_name: &str, batch: &Batch) -> shamir_db::query::BatchResponse {
    db.execute(db_name, &batch.to_request_via_msgpack())
        .await
        .expect("batch should succeed")
}

fn result_map(resp: &shamir_db::query::BatchResponse, alias: &str) -> QueryValue {
    resp.results[alias].records[0].as_value().as_ref().clone()
}

// ═══════════════════════════════════════════════════════════════════════
// Test: describe_table returns all sections
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn describe_table_returns_all_sections() {
    let db = ShamirDb::init_memory().await.unwrap();
    db.create_db("testdb").await;

    // 1. Create repo + table.
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("main").tables(["users"]));
    let resp = exec(&db, "testdb", &b).await;
    assert!(resp.results.contains_key("cr"));

    // 2. Set retention to "keep all" (default Retention = Forever = all None).
    let mut b = Batch::new();
    b.id(2);
    b.set_retention("ret", ddl::set_retention("users", Retention::default()));
    let resp = exec(&db, "testdb", &b).await;
    let r = result_map(&resp, "ret");
    assert_eq!(r.get("ok").and_then(|v| v.as_bool()), Some(true));

    // 3. Create a unique index on "email".
    let mut b = Batch::new();
    b.id(3);
    b.create_index(
        "idx",
        ddl::create_index("email_idx", "users")
            .field("email")
            .unique(),
    );
    let resp = exec(&db, "testdb", &b).await;
    assert!(resp.results.contains_key("idx"));

    // 4. Set schema (email: string, required). This auto-binds a schema
    //    validator to the table.
    let mut b = Batch::new();
    b.id(4);
    b.set_table_schema(
        "sch",
        ddl::set_table_schema("users").rules([ddl::field(["email"]).string().required().build()]),
    );
    let resp = exec(&db, "testdb", &b).await;
    let r = result_map(&resp, "sch");
    assert_eq!(r.get("ok").and_then(|v| v.as_bool()), Some(true));

    // ═══════════════════════════════════════════════════════════════
    // DESCRIBE
    // ═══════════════════════════════════════════════════════════════

    let mut b = Batch::new();
    b.id(10);
    b.describe_table("desc", ddl::describe_table("users"));
    let resp = exec(&db, "testdb", &b).await;
    let d = result_map(&resp, "desc");

    // ── describe_table / repo ────────────────────────────────────
    assert_eq!(
        d.get("describe_table").and_then(|v| v.as_str()),
        Some("users")
    );
    assert_eq!(d.get("repo").and_then(|v| v.as_str()), Some("main"));

    // ── schema ───────────────────────────────────────────────────
    let schema = d.get("schema").expect("schema section missing");
    let rules = schema.as_array().expect("schema should be a list");
    assert!(!rules.is_empty(), "schema should have rules");
    // The first rule should have path=["email"], type="string".
    let first = &rules[0];
    let path = first.get("path").and_then(|v| v.as_array()).unwrap();
    assert_eq!(path.len(), 1);
    assert_eq!(path[0].as_str(), Some("email"));
    assert_eq!(first.get("type").and_then(|v| v.as_str()), Some("string"));
    assert_eq!(first.get("required").and_then(|v| v.as_bool()), Some(true));

    // schema_version > 0
    assert!(
        d.get("schema_version")
            .and_then(|v| v.as_i64())
            .unwrap_or(0)
            > 0
    );

    // ── indexes ──────────────────────────────────────────────────
    let indexes = d
        .get("indexes")
        .and_then(|v| v.as_array())
        .expect("indexes section missing");
    assert!(!indexes.is_empty(), "should have at least one index");
    // Find the email_idx by name.
    let has_email_idx = indexes
        .iter()
        .any(|i| i.get("name").and_then(|v| v.as_str()) == Some("email_idx"));
    assert!(has_email_idx, "email_idx not found in indexes");

    // ── validators ───────────────────────────────────────────────
    let validators = d
        .get("validators")
        .and_then(|v| v.as_array())
        .expect("validators section missing");
    // The schema validator is auto-bound by set_table_schema.
    assert!(
        !validators.is_empty(),
        "expected at least 1 validator (schema), got 0"
    );

    // ── retention ────────────────────────────────────────────────
    let retention = d.get("retention").expect("retention section missing");
    assert!(
        !matches!(retention, QueryValue::Null),
        "retention should not be null after set_retention"
    );
    // keep_all (Forever) = all None, so max_count should be Null.
    assert!(
        retention.get("max_count").is_some(),
        "retention should have max_count key"
    );

    // ── access meta ──────────────────────────────────────────────
    assert!(d.get("owner").is_some(), "owner section missing");
    assert!(d.get("mode").is_some(), "mode section missing");
}
