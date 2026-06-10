//! Actor flow and row-level security (RLS) enforcement tests.

use shamir_query_builder::batch::Batch;
use shamir_query_builder::query::Query;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_types::access::Actor;

use crate::query::auth::{Action, Effect, Permission, Resource, Role, SessionPermissions};
use crate::query::batch::{execute_batch, execute_batch_with_permissions};

use super::common::setup_resolver;

// ============================================================================
// R2 structural test — actor flows through FilterContext
// ============================================================================

/// Verifies the actor field reaches the FilterContext that the QueryRunner
/// builds for each data op. The gate is transparent (always Ok), so this
/// confirms plumbing without needing enforcement.
#[tokio::test]
async fn r2_actor_flows_through_filter_context() {
    let resolver = setup_resolver().await;

    // Insert a row so the read has something to scan.
    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "ins",
        write::insert("users").row(doc().set("name", "Alice").set("age", 30)),
    );
    let seed_req = b.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test_db")
        .await
        .unwrap();

    // Read with an explicit User actor — the executor must carry it
    // into the FilterContext it builds.
    let user_actor = Actor::User(42);
    let mut b = Batch::new();
    b.id(2);
    b.query("q", Query::from("users"));
    let read_req = b.build();
    let resp = execute_batch(
        &read_req,
        &resolver,
        None,
        None,
        user_actor.clone(),
        "test_db",
    )
    .await
    .unwrap();
    assert_eq!(resp.results["q"].records.len(), 1);
}

// ============================================================================
// Stage B-1 — row-level security (RLS) enforcement
// ============================================================================

/// Build a `SessionPermissions` that grants Read/Update/Delete on
/// `default/users` with a row_filter restricting to `status == "active"`.
fn rls_permissions() -> SessionPermissions {
    let row_filter = crate::query::filter::Filter::Eq {
        field: vec!["status".to_string()],
        value: crate::query::filter::FilterValue::String("active".to_string()),
    };
    SessionPermissions::build(&[Role {
        name: "rls_role".to_string(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::Read, Action::Update, Action::Delete],
            resource: Resource::Table {
                database: "test".to_string(),
                repo: "main".to_string(),
                table: "users".to_string(),
            },
            row_filter: Some(row_filter),
        }],
    }])
}

/// Superadmin session — Action::All on Resource::Global → row_filter is None.
fn superadmin_permissions() -> SessionPermissions {
    SessionPermissions::build(&[Role {
        name: "admin".to_string(),
        permissions: vec![Permission {
            effect: Effect::Allow,
            actions: vec![Action::All],
            resource: Resource::Global,
            row_filter: None,
        }],
    }])
}

#[tokio::test]
async fn rls_read_returns_only_matching_rows() {
    let resolver = setup_resolver().await;
    let permissions = rls_permissions();

    // Seed mixed rows: some status="active", some not.
    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "seed",
        write::insert("users")
            .row(doc().set("name", "Alice").set("status", "active"))
            .row(doc().set("name", "Bob").set("status", "inactive"))
            .row(doc().set("name", "Carol").set("status", "active"))
            .row(doc().set("name", "Dave").set("status", "pending")),
    );
    let seed_req = b.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Read via execute_batch_with_permissions — should return ONLY active rows.
    let mut b = Batch::new();
    b.id(2);
    b.query("q", Query::from("users"));
    let read_req = b.build();

    let resp = execute_batch_with_permissions(&read_req, &resolver, None, &permissions, "test")
        .await
        .unwrap();

    let records = &resp.results["q"].records;
    assert_eq!(
        records.len(),
        2,
        "RLS must restrict Read to active rows only; got {:?}",
        records
    );
    let names: Vec<&str> = records
        .iter()
        .filter_map(|r| r.get("name").and_then(|v| v.as_str()))
        .collect();
    assert!(names.contains(&"Alice"), "Alice is active");
    assert!(names.contains(&"Carol"), "Carol is active");
    assert!(
        !names.contains(&"Bob"),
        "Bob is inactive — must be excluded"
    );
    assert!(
        !names.contains(&"Dave"),
        "Dave is pending — must be excluded"
    );
}

#[tokio::test]
async fn rls_delete_only_removes_matching_rows() {
    let resolver = setup_resolver().await;
    let permissions = rls_permissions();

    // Seed mixed rows.
    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "seed",
        write::insert("users")
            .row(doc().set("name", "Alice").set("status", "active"))
            .row(doc().set("name", "Bob").set("status", "inactive"))
            .row(doc().set("name", "Carol").set("status", "active")),
    );
    let seed_req = b.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Delete all via RLS — only active rows should be deleted.
    let mut b = Batch::new();
    b.id(2);
    b.delete(
        "del",
        write::delete("users").where_(shamir_query_builder::filter::eq("status", "active")),
    );
    let delete_req = b.build();

    let resp = execute_batch_with_permissions(&delete_req, &resolver, None, &permissions, "test")
        .await
        .unwrap();

    // The delete should have matched 2 records (Alice + Carol).
    assert_eq!(
        resp.results["del"].stats.as_ref().unwrap().records_scanned,
        2,
        "RLS restricts Delete to active rows"
    );

    // Verify the inactive row (Bob) still exists.
    let mut b = Batch::new();
    b.id(3);
    b.query("remaining", Query::from("users"));
    let verify_req = b.build();
    let verify_resp = execute_batch(&verify_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    let remaining = &verify_resp.results["remaining"].records;
    assert_eq!(
        remaining.len(),
        1,
        "only the inactive row should remain after RLS-scoped delete"
    );
    assert_eq!(
        remaining[0]["name"].as_str().unwrap(),
        "Bob",
        "the surviving row must be the inactive one"
    );
}

#[tokio::test]
async fn rls_superadmin_sees_all_rows() {
    let resolver = setup_resolver().await;
    let permissions = superadmin_permissions();

    // Seed mixed rows.
    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "seed",
        write::insert("users")
            .row(doc().set("name", "Alice").set("status", "active"))
            .row(doc().set("name", "Bob").set("status", "inactive"))
            .row(doc().set("name", "Carol").set("status", "pending")),
    );
    let seed_req = b.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Superadmin read — no row_filter restriction.
    let mut b = Batch::new();
    b.id(2);
    b.query("q", Query::from("users"));
    let read_req = b.build();

    let resp = execute_batch_with_permissions(&read_req, &resolver, None, &permissions, "test")
        .await
        .unwrap();

    assert_eq!(
        resp.results["q"].records.len(),
        3,
        "superadmin must see ALL rows (no RLS restriction)"
    );
}

#[tokio::test]
async fn rls_update_only_affects_matching_rows() {
    let resolver = setup_resolver().await;
    let permissions = rls_permissions();

    // Seed mixed rows.
    let mut b = Batch::new();
    b.id(1);
    b.op_silent(
        "seed",
        write::insert("users")
            .row(doc().set("name", "Alice").set("status", "active"))
            .row(doc().set("name", "Bob").set("status", "inactive")),
    );
    let seed_req = b.build();
    execute_batch(&seed_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Update ALL rows (no WHERE clause) — RLS should restrict to active only.
    let mut b = Batch::new();
    b.id(2);
    b.update(
        "upd",
        write::update("users").set(doc().set("tag", "updated")),
    );
    let update_req = b.build();

    let resp = execute_batch_with_permissions(&update_req, &resolver, None, &permissions, "test")
        .await
        .unwrap();

    // Only 1 record should have been updated (Alice — active).
    assert_eq!(
        resp.results["upd"].stats.as_ref().unwrap().records_scanned,
        1,
        "RLS restricts Update to active rows only"
    );

    // Verify Bob was NOT updated.
    let mut b = Batch::new();
    b.id(3);
    b.query("check", Query::from("users").where_eq("name", "Bob"));
    let verify_req = b.build();
    let verify_resp = execute_batch(&verify_req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    let bob = &verify_resp.results["check"].records;
    assert_eq!(bob.len(), 1, "Bob should still exist");
    assert!(
        bob[0].get("tag").is_none(),
        "Bob should NOT have the 'tag' field — Update was RLS-restricted to active rows"
    );
}
