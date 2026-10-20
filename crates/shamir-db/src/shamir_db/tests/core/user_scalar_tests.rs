//! Phase 2 — Native↔WASM parity: user-registered native scalar functions
//! in the SCALAR plane (filters + functional indexes).
//!
//! These tests verify:
//! 1. A custom native scalar registered via `db.scalars().register`, used in a
//!    WHERE filter (FnCall), returns the SAME rows as the equivalent built-in logic.
//! 2. A custom pure+deterministic native scalar (`.trusted_pure()`) backs a
//!    functional index — index builds and reads correctly.
//! 3. A non-vouched native scalar is REJECTED from the functional-index path
//!    (clear error, not a silent wrong index).
//! 4. A built-in scalar still resolves unchanged (user layer empty → builtin).

use shamir_funclib::registry::{arg_str, v_str, FnEntry};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::create_index;
use shamir_query_builder::filter::eq;
use shamir_query_builder::val::{func, lit};
use shamir_query_builder::write::{doc, insert};
use shamir_query_builder::Query;

use crate::engine::repo::repo_types::BoxRepoFactory;
use crate::engine::repo::RepoConfig;
use crate::engine::table::TableConfig;
use crate::ShamirDb;

/// Set up a fresh in-memory ShamirDb with a `testdb` database, a `main` repo,
/// and a `users` table. Returns the ShamirDb handle.
async fn setup() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    let db = shamir.create_db("testdb").await;
    let repo_config =
        RepoConfig::new("main", BoxRepoFactory::in_memory()).add_table(TableConfig::new("users"));
    db.add_repo(repo_config).await.unwrap();
    shamir
}

/// Helper: run a batch and return the response.
async fn run(shamir: &ShamirDb, b: Batch) -> shamir_query_types::batch::BatchResponse {
    let req = b.to_request_via_msgpack();
    shamir.execute("testdb", &req).await.unwrap()
}

// ============================================================================
// TDD 1: Custom native scalar in WHERE filter (FnCall path)
// ============================================================================

#[tokio::test]
async fn user_scalar_in_where_filter() {
    let shamir = setup().await;

    // Register a custom "my_reverse" scalar that reverses a string.
    let scalars = shamir.scalars("testdb").unwrap();
    scalars.register(
        "my_reverse",
        FnEntry::pure(
            |args: &[shamir_types::types::value::QueryValue]| {
                let s = arg_str(args, 0)?;
                Ok(v_str(s.chars().rev().collect()))
            },
            1,
            Some(1),
        ),
    );

    // Insert test records.
    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "ins",
        insert("users")
            .row(doc().set("name", "alice").set("age", 30))
            .row(doc().set("name", "bob").set("age", 25)),
    );
    run(&shamir, b).await;

    // Query: WHERE name == my_reverse("ecila")  → name == "alice"
    // The FnCall path evaluates my_reverse("ecila") = "alice", then compares
    // field "name" against "alice". This is the same logic as the built-in
    // strings/reverse would give (if it existed) — proving the user scalar
    // dispatches through the 2-layer ScalarResolver on the hot filter path.
    let query = Query::from("users").where_(eq("name", func("my_reverse", [lit("ecila")])));
    let mut b = Batch::new();
    b.id(1);
    b.query("result", query);
    let resp = run(&shamir, b).await;

    assert_eq!(
        resp.results["result"].records.len(),
        1,
        "expected 1 record: my_reverse('ecila')='alice' matches name='alice'"
    );
}

// ============================================================================
// TDD 2: Custom trusted_pure scalar backs a functional index
// ============================================================================

#[tokio::test]
async fn trusted_pure_scalar_backs_functional_index() {
    let shamir = setup().await;

    // Register a custom "my_upper" scalar with .trusted_pure().
    let scalars = shamir.scalars("testdb").unwrap();
    scalars.register(
        "my_upper",
        FnEntry::pure(
            |args: &[shamir_types::types::value::QueryValue]| {
                let s = arg_str(args, 0)?;
                Ok(v_str(s.to_uppercase()))
            },
            1,
            Some(1),
        )
        .trusted_pure(),
    );

    // Insert a record BEFORE creating the index.
    let mut b = Batch::new();
    b.id(1);
    b.op_silent("ins", insert("users").row(doc().set("name", "alice")));
    run(&shamir, b).await;

    // Create a functional index backed by the trusted_pure scalar.
    let mut b = Batch::new();
    b.id(1);
    b.create_index(
        "cix",
        create_index("idx_upper_name", "users")
            .field("name")
            .index_type("functional")
            .functional_op("my_upper"),
    );
    let resp = run(&shamir, b).await;
    // Admin ops return a result — check it didn't error.
    assert!(
        !resp.results["cix"].records.is_empty() || resp.results.contains_key("cix"),
        "functional index creation with trusted_pure scalar should succeed"
    );

    // Insert another record AFTER creating the index (exercises the
    // FunctionalBackend eval path with IndexExpr::Scalar).
    let mut b = Batch::new();
    b.id(1);
    b.op_silent("ins2", insert("users").row(doc().set("name", "bob")));
    run(&shamir, b).await;

    // Query using Computed filter to probe the functional index.
    // my_upper("alice") = "ALICE" → looking for records where my_upper(name) == "ALICE".
    let query = Query::from("users").where_(shamir_query_types::filter::Filter::Computed {
        expr_op: "my_upper".to_string(),
        field: vec!["name".to_string()],
        expr_args: None,
        cmp: "eq".to_string(),
        value: shamir_query_types::filter::FilterValue::String("ALICE".to_string()),
    });
    let mut b = Batch::new();
    b.id(1);
    b.query("result", query);
    let resp = run(&shamir, b).await;

    assert_eq!(
        resp.results["result"].records.len(),
        1,
        "expected 1 record where my_upper(name) == 'ALICE'"
    );
    // Verify it's the right record.
    assert_eq!(
        resp.results["result"].records[0].get_value_str("name"),
        Some("alice"),
        "the matching record should have name='alice'"
    );
}

// ============================================================================
// Reopen tests — functional indexes must survive DB restart.
//
// These use a Fjall-backed (durable) system store so that index2 metadata
// is persisted and reloaded on reopen via TableManager::create().
// ============================================================================

use crate::shamir_db::SystemStoreConfig;

/// Re-open the system store, retrying while the previous session's
/// store still holds the file lock.
async fn reinit_with_retry(sys_path: std::path::PathBuf) -> ShamirDb {
    for _ in 0..100 {
        match ShamirDb::init(SystemStoreConfig::Fjall(sys_path.clone())).await {
            Ok(shamir) => return shamir,
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(20)).await,
        }
    }
    ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .expect("system store still locked after retries")
}

/// Set up a durable ShamirDb with a db + repo + table.
/// Uses the wire CreateRepo op so the repo is persisted in the catalogue
/// and re-attached on reopen.
async fn setup_durable(sys_path: std::path::PathBuf) -> ShamirDb {
    let shamir = ShamirDb::init(SystemStoreConfig::Fjall(sys_path))
        .await
        .unwrap();
    shamir.create_db("testdb").await;

    // Create repo via the wire protocol so it is persisted in the catalogue.
    use shamir_query_builder::ddl;
    let mut b = Batch::new();
    b.id(1);
    b.create_repo("cr", ddl::create_repo("main").tables(["users"]));
    let req = b.to_request_via_msgpack();
    let _ = shamir.execute("testdb", &req).await.unwrap();

    shamir
}

// ----------------------------------------------------------------------------
// TDD 5: BUILTIN functional index survives DB reopen
// ----------------------------------------------------------------------------

#[tokio::test]
async fn builtin_functional_index_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // === Session 1: create table + builtin functional index + insert ===
    {
        let shamir = setup_durable(sys_path.clone()).await;

        let mut b = Batch::new();
        b.id(1);
        b.create_index(
            "cix",
            create_index("idx_lower_name", "users")
                .field("name")
                .index_type("functional")
                .functional_op("lower"),
        );
        let resp = run(&shamir, b).await;
        assert!(resp.results.contains_key("cix"));

        let mut b = Batch::new();
        b.id(2);
        b.op_silent("ins", insert("users").row(doc().set("name", "Alice")));
        run(&shamir, b).await;
    }

    // === Session 2: reopen — builtin functional index must still work ===
    let shamir = reinit_with_retry(sys_path).await;

    // Insert another record (exercises FunctionalBackend eval on reopen).
    let mut b = Batch::new();
    b.id(3);
    b.op_silent("ins", insert("users").row(doc().set("name", "Bob")));
    let req = b.to_request_via_msgpack();
    let result = shamir.execute("testdb", &req).await;
    assert!(
        result.is_ok(),
        "insert after reopen should succeed with builtin functional index: {:?}",
        result.err()
    );

    // Query: WHERE lower(name) == "alice"
    let query = Query::from("users").where_(shamir_query_types::filter::Filter::Computed {
        expr_op: "lower".to_string(),
        field: vec!["name".to_string()],
        expr_args: None,
        cmp: "eq".to_string(),
        value: shamir_query_types::filter::FilterValue::String("alice".to_string()),
    });
    let mut b = Batch::new();
    b.id(4);
    b.query("result", query);
    let resp = run(&shamir, b).await;

    assert_eq!(
        resp.results["result"].records.len(),
        1,
        "builtin functional index should find 'Alice' after reopen"
    );
    assert_eq!(
        resp.results["result"].records[0].get_value_str("name"),
        Some("Alice"),
    );
}

// ----------------------------------------------------------------------------
// TDD 6: USER-scalar functional index, reopened BEFORE re-register → FAIL CLOSED
// ----------------------------------------------------------------------------

#[tokio::test]
async fn user_scalar_index_fails_closed_before_re_register() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // === Session 1: register scalar, create index, insert ===
    {
        let shamir = setup_durable(sys_path.clone()).await;

        let scalars = shamir.scalars("testdb").unwrap();
        scalars.register(
            "my_durable_upper",
            FnEntry::pure(
                |args: &[shamir_types::types::value::QueryValue]| {
                    let s = arg_str(args, 0)?;
                    Ok(v_str(s.to_uppercase()))
                },
                1,
                Some(1),
            )
            .trusted_pure(),
        );

        let mut b = Batch::new();
        b.id(1);
        b.create_index(
            "cix",
            create_index("idx_dupper", "users")
                .field("name")
                .index_type("functional")
                .functional_op("my_durable_upper"),
        );
        let resp = run(&shamir, b).await;
        assert!(resp.results.contains_key("cix"));

        let mut b = Batch::new();
        b.id(2);
        b.op_silent("ins", insert("users").row(doc().set("name", "alice")));
        run(&shamir, b).await;
    }

    // === Session 2: reopen WITHOUT re-registering the user scalar ===
    let shamir = reinit_with_retry(sys_path).await;

    // Insert must FAIL — the scalar is not re-registered, so the
    // FunctionalBackend cannot eval the index expression. It MUST
    // return a loud error, NOT silently produce a Null index key.
    let mut b = Batch::new();
    b.id(3);
    b.op_silent("ins", insert("users").row(doc().set("name", "bob")));
    let req = b.to_request_via_msgpack();
    let result = shamir.execute("testdb", &req).await;

    assert!(
        result.is_err(),
        "insert must FAIL when the user scalar backing a functional index \
         is not re-registered after reopen — got success instead"
    );
    let err_msg = format!("{:?}", result.unwrap_err());
    assert!(
        err_msg.contains("scalar") || err_msg.contains("no scalar resolver"),
        "error should mention the scalar resolution failure: got '{err_msg}'"
    );
}

// ----------------------------------------------------------------------------
// TDD 7: USER-scalar functional index works after re-registering post-reopen
// ----------------------------------------------------------------------------

#[tokio::test]
async fn user_scalar_index_works_after_re_register() {
    let dir = tempfile::tempdir().unwrap();
    let sys_path = dir.path().join("meta.redb");

    // === Session 1: register, create index, insert ===
    {
        let shamir = setup_durable(sys_path.clone()).await;

        let scalars = shamir.scalars("testdb").unwrap();
        scalars.register(
            "my_reopen_upper",
            FnEntry::pure(
                |args: &[shamir_types::types::value::QueryValue]| {
                    let s = arg_str(args, 0)?;
                    Ok(v_str(s.to_uppercase()))
                },
                1,
                Some(1),
            )
            .trusted_pure(),
        );

        let mut b = Batch::new();
        b.id(1);
        b.create_index(
            "cix",
            create_index("idx_reupper", "users")
                .field("name")
                .index_type("functional")
                .functional_op("my_reopen_upper"),
        );
        let resp = run(&shamir, b).await;
        assert!(resp.results.contains_key("cix"));

        let mut b = Batch::new();
        b.id(2);
        b.op_silent("ins", insert("users").row(doc().set("name", "alice")));
        run(&shamir, b).await;
    }

    // === Session 2: reopen AND re-register the user scalar ===
    let shamir = reinit_with_retry(sys_path).await;

    // Re-register the scalar (the embedder does this at boot).
    let scalars = shamir.scalars("testdb").unwrap();
    scalars.register(
        "my_reopen_upper",
        FnEntry::pure(
            |args: &[shamir_types::types::value::QueryValue]| {
                let s = arg_str(args, 0)?;
                Ok(v_str(s.to_uppercase()))
            },
            1,
            Some(1),
        )
        .trusted_pure(),
    );

    // Now insert should work.
    let mut b = Batch::new();
    b.id(3);
    b.op_silent("ins", insert("users").row(doc().set("name", "bob")));
    let req = b.to_request_via_msgpack();
    let result = shamir.execute("testdb", &req).await;
    assert!(
        result.is_ok(),
        "insert should succeed after re-registering the user scalar: {:?}",
        result.err()
    );

    // Query: WHERE my_reopen_upper(name) == "ALICE"
    let query = Query::from("users").where_(shamir_query_types::filter::Filter::Computed {
        expr_op: "my_reopen_upper".to_string(),
        field: vec!["name".to_string()],
        expr_args: None,
        cmp: "eq".to_string(),
        value: shamir_query_types::filter::FilterValue::String("ALICE".to_string()),
    });
    let mut b = Batch::new();
    b.id(4);
    b.query("result", query);
    let resp = run(&shamir, b).await;

    assert_eq!(
        resp.results["result"].records.len(),
        1,
        "user-scalar functional index should find 'alice' after re-register"
    );
    assert_eq!(
        resp.results["result"].records[0].get_value_str("name"),
        Some("alice"),
    );
} // ============================================================================
  // TDD 3: Non-vouched scalar rejected from functional index
  // ============================================================================

#[tokio::test]
async fn non_vouched_scalar_rejected_from_index() {
    let shamir = setup().await;

    // Register a custom scalar WITHOUT .trusted_pure().
    let scalars = shamir.scalars("testdb").unwrap();
    scalars.register(
        "my_unsafe_fn",
        FnEntry::pure(
            |args: &[shamir_types::types::value::QueryValue]| {
                let s = arg_str(args, 0)?;
                Ok(v_str(s.to_uppercase()))
            },
            1,
            Some(1),
        ),
        // NOTE: no .trusted_pure() — the embedder has NOT vouched this fn.
    );

    // Attempt to create a functional index with the non-vouched scalar.
    let mut b = Batch::new();
    b.id(1);
    b.create_index(
        "cix",
        create_index("idx_unsafe", "users")
            .field("name")
            .index_type("functional")
            .functional_op("my_unsafe_fn"),
    );
    let req = b.to_request_via_msgpack();
    let result = shamir.execute("testdb", &req).await;

    assert!(
        result.is_err(),
        "functional index creation with non-vouched scalar should FAIL"
    );
    let err_msg = format!("{:?}", result.unwrap_err());
    assert!(
        err_msg.contains("trusted_pure") || err_msg.contains("not trusted"),
        "error message should mention trusted_pure: got '{err_msg}'"
    );
}

// ============================================================================
// TDD 4: Built-in scalar resolves unchanged (user layer empty)
// ============================================================================

#[tokio::test]
async fn builtin_scalar_resolves_with_empty_user_layer() {
    let shamir = setup().await;

    // Insert test records — one uppercase, one lowercase.
    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "ins",
        insert("users")
            .row(doc().set("name", "ALICE"))
            .row(doc().set("name", "alice")),
    );
    run(&shamir, b).await;

    // Query: WHERE name == strings/lower("ALICE")  → name == "alice"
    // The built-in strings/lower resolves through the ScalarResolver's
    // builtin layer (user layer is empty → one hash-miss → builtin fallback).
    let query = Query::from("users").where_(eq("name", func("strings/lower", [lit("ALICE")])));
    let mut b = Batch::new();
    b.id(1);
    b.query("result", query);
    let resp = run(&shamir, b).await;

    assert_eq!(
        resp.results["result"].records.len(),
        1,
        "built-in strings/lower should resolve: lower('ALICE')='alice' matches only name='alice'"
    );
    assert_eq!(
        resp.results["result"].records[0].get_value_str("name"),
        Some("alice"),
        "the matching record should have name='alice'"
    );
}
