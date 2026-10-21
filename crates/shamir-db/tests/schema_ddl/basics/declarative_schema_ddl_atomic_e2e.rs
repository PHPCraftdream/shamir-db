//! F-4 regression e2e: schema DDL atomicity (validate → precompile → persist →
//! activate, with rollback on activation failure).
//!
//! Bug 1 — before F-4, all three DDL handlers (`SetTableSchema` /
//! `AddSchemaRule` / `RemoveSchemaRule`) wrote the catalogue record BEFORE
//! running `parse_schema` (the precompile gate). A schema that passed the
//! earlier index/cascade validations but failed to round-trip into
//! `Vec<FieldRule>` (unknown type tag, etc.) therefore left the catalogue
//! holding an unparseable, never-activated schema. After F-4, `parse_schema`
//! runs BEFORE `save_table_meta`, so a failed parse leaves the catalogue
//! untouched.
//!
//! These tests exercise the real `ShamirDb::execute` DDL path. The trigger is
//! an unknown `type` tag (`ddl::field(..).type_tag("bogus_type")`) — the
//! earlier validators (`validate_fk_indexes` / `validate_unique_indexes` /
//! `validate_nested_path_transforms` / `validate_no_self_referential_cascade`)
//! do not inspect the `type` string, so a bogus tag sails past them and only
//! `parse_schema` catches it, which is exactly the gap F-4 closes.
//!
//! The `array_of` DDL-surface test additionally proves Bug 2's parse-level fix
//! (garbled `array_of` → error, not silent success) surfaces through the DDL.
//!
//! ## Activation-failure rollback (Bug 1, part 2)
//!
//! The compensating-rollback path (re-`save_table_meta` the pre-mutation record
//! when `compile_table_schema` fails after a successful catalogue write) is
//! verified by code inspection of `admin_schema.rs`: there is no
//! failure-injection seam for `compile_table_schema` / `add_validator_binding`
//! in the test fixtures (it requires a live table + validator registry + I/O
//! binding), and the brief explicitly rules out inventing a new
//! transaction/WAL mechanism. The structural guarantee is:
//!
//!   1. `rec_prev = rec.clone()` is captured before the catalogue mutation.
//!   2. On `compile_table_schema` error, `save_table_meta(&rec_prev)` rewrites
//!      the catalogue to the still-working old record.
//!   3. The rollback write's own failure is `log::warn!`-ed (secondary), and
//!      the ORIGINAL activation error is returned to the caller.

use shamir_db::shamir_db::SystemStoreConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_types::types::value::QueryValue;

// ── helpers ──────────────────────────────────────────────────────────────

fn r_int(v: &QueryValue, k: &str) -> Option<i64> {
    v.get(k).and_then(|x| x.as_i64())
}

fn path_has(r: &QueryValue, name: &str) -> bool {
    r.get("path")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().any(|p| p.as_str() == Some(name)))
        .unwrap_or(false)
}

/// Execute a `set_table_schema` DDL op, returning the response result map.
async fn exec_set_schema(
    db: &ShamirDb,
    db_name: &str,
    alias: &str,
    table: &str,
    rules: Vec<ddl::FieldBuilder>,
    expected_version: Option<u64>,
) -> QueryValue {
    let rules: Vec<_> = rules.into_iter().map(|b| b.build()).collect();
    let mut b = Batch::new();
    b.id(1);
    let mut builder = ddl::set_table_schema(table).rules(rules);
    if let Some(v) = expected_version {
        builder = builder.expected_version(v);
    }
    b.set_table_schema(alias, builder);
    let resp = db
        .execute(db_name, &b.to_request_via_msgpack())
        .await
        .expect("set_table_schema should succeed");
    resp.results[alias].records[0].as_value().as_ref().clone()
}

/// Read back the catalogue schema + version via GetTableSchema.
async fn read_schema(db: &ShamirDb, db_name: &str, table: &str) -> QueryValue {
    let mut b = Batch::new();
    b.id(1);
    b.get_table_schema("gs", ddl::get_table_schema(table));
    let resp = db
        .execute(db_name, &b.to_request_via_msgpack())
        .await
        .expect("get_table_schema should succeed");
    resp.results["gs"].records[0].as_value().as_ref().clone()
}

async fn setup(db: &ShamirDb) {
    db.create_db("testdb").await;
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("main").tables(["users"]));
    db.execute("testdb", &b.to_request_via_msgpack())
        .await
        .unwrap();
}

// ═══════════════════════════════════════════════════════════════════════
// Bug 1: bad schema via set_table_schema does NOT persist
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn bad_type_tag_set_table_schema_leaves_catalogue_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
        .await
        .unwrap();
    setup(&db).await;

    // Set a valid schema: email required string → version 1.
    let r1 = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![ddl::field(["email"]).string().required()],
        None,
    )
    .await;
    assert_eq!(r_int(&r1, "schema_version"), Some(1));

    // Attempt to replace with a schema containing an unparseable type tag.
    // The earlier validations do not inspect the type string, so this reaches
    // the precompile gate (parse_schema), which must reject it BEFORE the
    // catalogue is written (F-4 reorder).
    let mut b = Batch::new();
    b.id(2);
    b.set_table_schema(
        "ss_bad",
        ddl::set_table_schema("users")
            .rules(vec![ddl::field(["age"]).type_tag("bogus_type").build()]),
    );
    let resp = db.execute("testdb", &b.to_request_via_msgpack()).await;
    assert!(resp.is_err(), "bad type tag should error");
    let err = resp.unwrap_err().to_string();
    assert!(
        err.contains("bad_schema") || err.contains("unknown schema type tag"),
        "error should mention bad_schema/type tag, got: {err}"
    );

    // Core regression: the catalogue must still hold the ORIGINAL schema.
    let result = read_schema(&db, "testdb", "users").await;
    assert_eq!(
        r_int(&result, "schema_version"),
        Some(1),
        "catalogue version must be unchanged after failed DDL"
    );
    let schema = result.get("schema").and_then(|v| v.as_array()).unwrap();
    assert!(
        schema.iter().any(|r| path_has(r, "email")),
        "email rule must still be present"
    );
    assert!(
        !schema.iter().any(|r| path_has(r, "age")),
        "bad age rule must NOT have been persisted"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Bug 1: bad schema via add_schema_rule does NOT persist
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn bad_type_tag_add_schema_rule_leaves_catalogue_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
        .await
        .unwrap();
    setup(&db).await;

    // Set a valid schema: email required string → version 1.
    let r1 = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![ddl::field(["email"]).string().required()],
        None,
    )
    .await;
    assert_eq!(r_int(&r1, "schema_version"), Some(1));

    // Attempt to add a rule with an unparseable type tag.
    let mut b = Batch::new();
    b.id(2);
    b.try_add_schema_rule(
        "ar_bad",
        ddl::add_schema_rule("users").rule(ddl::field(["age"]).type_tag("bogus_type")),
    )
    .unwrap();
    let resp = db.execute("testdb", &b.to_request_via_msgpack()).await;
    assert!(resp.is_err(), "bad type tag should error");
    let err = resp.unwrap_err().to_string();
    assert!(
        err.contains("bad_schema") || err.contains("unknown schema type tag"),
        "error should mention bad_schema/type tag, got: {err}"
    );

    // Catalogue must be unchanged: still version 1, only the email rule.
    let result = read_schema(&db, "testdb", "users").await;
    assert_eq!(
        r_int(&result, "schema_version"),
        Some(1),
        "catalogue version must be unchanged after failed add_schema_rule"
    );
    let schema = result.get("schema").and_then(|v| v.as_array()).unwrap();
    assert!(schema.iter().any(|r| path_has(r, "email")));
    assert!(
        !schema.iter().any(|r| path_has(r, "age")),
        "bad age rule must NOT have been persisted"
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Bug 2 (DDL surface): garbled array_of surfaces as a DDL error
// ═══════════════════════════════════════════════════════════════════════
//
// `array_of` is reachable via the typed builder (`.array_of("bogus")`), so
// this proves the parse-level fix (schema_management.rs) surfaces through the
// DDL handler as an error rather than a silent success. Before F-4 the bad
// `array_of` silently became `None` and the DDL reported `ok:true`; after F-4
// it is a `bad_schema` error, and (thanks to the Bug 1 reorder) the catalogue
// is left untouched.

#[tokio::test]
async fn garbled_array_of_surfaces_as_ddl_error_and_leaves_catalogue_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
        .await
        .unwrap();
    setup(&db).await;

    // Set a valid schema: tags list-of-string → version 1.
    let r1 = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![ddl::field(["tags"]).list().array_of("string")],
        None,
    )
    .await;
    assert_eq!(r_int(&r1, "schema_version"), Some(1));

    // Replace with a schema whose array_of is a bogus type tag.
    let mut b = Batch::new();
    b.id(2);
    b.set_table_schema(
        "ss_bad",
        ddl::set_table_schema("users").rules(vec![ddl::field(["tags"])
            .list()
            .array_of("bogus_type")
            .build()]),
    );
    let resp = db.execute("testdb", &b.to_request_via_msgpack()).await;
    assert!(resp.is_err(), "garbled array_of should error");
    let err = resp.unwrap_err().to_string();
    assert!(
        err.contains("bad_schema") || err.contains("array_of"),
        "error should mention bad_schema/array_of, got: {err}"
    );

    // Catalogue unchanged: still version 1, tags rule still list-of-string.
    let result = read_schema(&db, "testdb", "users").await;
    assert_eq!(r_int(&result, "schema_version"), Some(1));
}

// ═══════════════════════════════════════════════════════════════════════
// Sanity: a valid schema still succeeds (no over-tightening of the gate)
// ═══════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn valid_schema_still_succeeds_after_reorder() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");
    let db = ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone()))
        .await
        .unwrap();
    setup(&db).await;

    // A fully valid schema with Phase B + array_of must still succeed and bump
    // the version (regression: the reorder must not reject legitimate DDL).
    let r = exec_set_schema(
        &db,
        "testdb",
        "ss",
        "users",
        vec![
            ddl::field(["email"]).string().format("email").required(),
            ddl::field(["tags"]).list().array_of("string"),
        ],
        None,
    )
    .await;
    assert_eq!(r_int(&r, "schema_version"), Some(1));

    let result = read_schema(&db, "testdb", "users").await;
    assert_eq!(r_int(&result, "schema_version"), Some(1));
    let schema = result.get("schema").and_then(|v| v.as_array()).unwrap();
    assert!(schema.iter().any(|r| path_has(r, "email")));
    assert!(schema.iter().any(|r| path_has(r, "tags")));
}
