use std::sync::Arc;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_types::access::Actor;
use shamir_types::types::value::QueryValue;

use super::support::{
    bind_fk_validator, insert_row, read_field_all, read_first_field, FkTestResolver,
};
use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::execute_batch_unchecked as execute_batch;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;
use crate::validator::ValidatorRegistry;

// ═══════════════════════════════════════════════════════════════════════════════

/// Two child tables (`orders` + `sessions`), BOTH with a `user_id` FK →
/// `users.id` `ON UPDATE CASCADE`.  Re-keying a user must update BOTH child
/// tables, not just the one that survived the old dedup.
#[tokio::test]
async fn on_update_cascade_two_child_tables_same_ref_field() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![
            TableConfig::new("users"),
            TableConfig::new("orders"),
            TableConfig::new("sessions"),
        ],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    // orders.user_id → users.id ON UPDATE CASCADE
    bind_fk_validator(
        &db,
        &registry,
        "orders",
        9310,
        "orders_fk_user",
        "user_id",
        "users",
        "id",
        FkAction::Cascade,
        true,
    );
    // sessions.user_id → users.id ON UPDATE CASCADE
    bind_fk_validator(
        &db,
        &registry,
        "sessions",
        9311,
        "sessions_fk_user",
        "user_id",
        "users",
        "id",
        FkAction::Cascade,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_row(
        &resolver,
        "iu",
        "users",
        doc().set("id", 5).unwrap().set("name", "alice").unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "io",
        "orders",
        doc()
            .set("oid", 1)
            .unwrap()
            .set("user_id", 5)
            .unwrap()
            .set("total", 100)
            .unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "is",
        "sessions",
        doc()
            .set("sid", 1)
            .unwrap()
            .set("user_id", 5)
            .unwrap()
            .set("token", "t1")
            .unwrap(),
    )
    .await;

    // Update users.id 5 → 7 → BOTH orders.user_id and sessions.user_id re-keyed.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_user",
        write::update("users")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7).unwrap()),
    )
    .unwrap();
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_eq!(
        read_first_field(&resolver, "users", "id").await,
        Some(QueryValue::Int(7))
    );
    assert_eq!(
        read_first_field(&resolver, "orders", "user_id").await,
        Some(QueryValue::Int(7)),
        "orders.user_id must be re-keyed to 7 (the shared-ref-field dedup bug \
         previously dropped one of the two child references)"
    );
    assert_eq!(
        read_first_field(&resolver, "sessions", "user_id").await,
        Some(QueryValue::Int(7)),
        "sessions.user_id must be re-keyed to 7 (the shared-ref-field dedup bug \
         previously dropped one of the two child references)"
    );
}

/// One child table (`messages`) with TWO FK fields (`sender_id` +
/// `receiver_id`), BOTH → `users.id` `ON UPDATE CASCADE`.  Re-keying a user
/// must update BOTH fields on the affected message rows, not just one.
#[tokio::test]
async fn on_update_cascade_two_fk_fields_same_ref_field() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users"), TableConfig::new("messages")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    // messages.sender_id → users.id ON UPDATE CASCADE
    bind_fk_validator(
        &db,
        &registry,
        "messages",
        9312,
        "messages_fk_sender",
        "sender_id",
        "users",
        "id",
        FkAction::Cascade,
        true,
    );
    // messages.receiver_id → users.id ON UPDATE CASCADE
    bind_fk_validator(
        &db,
        &registry,
        "messages",
        9313,
        "messages_fk_receiver",
        "receiver_id",
        "users",
        "id",
        FkAction::Cascade,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    // alice(id=5) sends a message to herself (both sender & receiver = 5).
    insert_row(
        &resolver,
        "iu",
        "users",
        doc().set("id", 5).unwrap().set("name", "alice").unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "im",
        "messages",
        doc()
            .set("mid", 1)
            .unwrap()
            .set("sender_id", 5)
            .unwrap()
            .set("receiver_id", 5)
            .unwrap()
            .set("body", "hi")
            .unwrap(),
    )
    .await;

    // Update users.id 5 → 7 → BOTH sender_id and receiver_id re-keyed.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_user",
        write::update("users")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7).unwrap()),
    )
    .unwrap();
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_eq!(
        read_first_field(&resolver, "messages", "sender_id").await,
        Some(QueryValue::Int(7)),
        "sender_id must be re-keyed (the shared-ref-field dedup bug previously \
         kept only one of the two FK fields on the same child)"
    );
    assert_eq!(
        read_first_field(&resolver, "messages", "receiver_id").await,
        Some(QueryValue::Int(7)),
        "receiver_id must be re-keyed (the shared-ref-field dedup bug previously \
         kept only one of the two FK fields on the same child)"
    );
}

/// RESTRICT variant: two child tables, BOTH → `users.id`.  One CASCADE, one
/// RESTRICT.  Under the OLD (buggy) dedup only one ref survived; if the CASCADE
/// ref survived, the RESTRICT ref was silently dropped and the update was
/// wrongly allowed through despite a child still referencing the old value.
/// With the fix both refs survive → the RESTRICT check fires → update rejected.
#[tokio::test]
async fn on_update_restrict_fires_even_when_sharing_ref_field_with_cascade() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![
            TableConfig::new("users"),
            TableConfig::new("cascade_child"),
            TableConfig::new("restrict_child"),
        ],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    bind_fk_validator(
        &db,
        &registry,
        "cascade_child",
        9314,
        "cascade_child_fk",
        "user_id",
        "users",
        "id",
        FkAction::Cascade,
        true,
    );
    bind_fk_validator(
        &db,
        &registry,
        "restrict_child",
        9315,
        "restrict_child_fk",
        "user_id",
        "users",
        "id",
        FkAction::Restrict,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_row(
        &resolver,
        "iu",
        "users",
        doc().set("id", 5).unwrap().set("name", "alice").unwrap(),
    )
    .await;
    // The RESTRICT child still references the old id=5 → update must be rejected.
    insert_row(
        &resolver,
        "irc",
        "restrict_child",
        doc()
            .set("rid", 1)
            .unwrap()
            .set("user_id", 5)
            .unwrap()
            .set("note", "refs-5")
            .unwrap(),
    )
    .await;

    // Update users.id 5 → 7 → RESTRICT must fire (restrict_child still refs 5).
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_user",
        write::update("users")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7).unwrap()),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;

    match resp {
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("fk_restrict"),
                "expected fk_restrict error, got: {msg}"
            );
        }
        Ok(_) => panic!(
            "Expected fk_restrict rejection (restrict_child still references old id), \
             got success — the shared-ref-field dedup bug may have dropped the RESTRICT ref"
        ),
    }

    // Parent unchanged (rollback).
    assert_eq!(
        read_first_field(&resolver, "users", "id").await,
        Some(QueryValue::Int(5))
    );
}

/// Non-regression: the common single-child / single-FK case (unaffected by the
/// dedup bug, which only bites when refs share a parent field) must keep
/// cascading exactly as before, and children that don't reference the matched
/// value stay untouched.
#[tokio::test]
async fn on_update_cascade_single_fk_non_regression() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("users"), TableConfig::new("orders")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    bind_fk_validator(
        &db,
        &registry,
        "orders",
        9316,
        "orders_fk_single",
        "user_id",
        "users",
        "id",
        FkAction::Cascade,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_row(
        &resolver,
        "iu1",
        "users",
        doc().set("id", 5).unwrap().set("name", "alice").unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "iu2",
        "users",
        doc().set("id", 6).unwrap().set("name", "bob").unwrap(),
    )
    .await;
    // order refs 5 (should be re-keyed); order refs 9 (should be untouched).
    insert_row(
        &resolver,
        "io1",
        "orders",
        doc()
            .set("oid", 1)
            .unwrap()
            .set("user_id", 5)
            .unwrap()
            .set("v", "a")
            .unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "io2",
        "orders",
        doc()
            .set("oid", 2)
            .unwrap()
            .set("user_id", 9)
            .unwrap()
            .set("v", "b")
            .unwrap(),
    )
    .await;

    // Re-key only alice 5→7.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_user",
        write::update("users")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7).unwrap()),
    )
    .unwrap();
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let user_ids = read_field_all(&resolver, "orders", "user_id").await;
    assert_eq!(user_ids.len(), 2);
    assert!(
        user_ids.contains(&QueryValue::Int(7)),
        "order referencing 5 must be re-keyed to 7, got: {user_ids:?}"
    );
    assert!(
        user_ids.contains(&QueryValue::Int(9)),
        "order referencing 9 (unmatched) must be untouched, got: {user_ids:?}"
    );
    assert!(
        !user_ids.contains(&QueryValue::Int(5)),
        "stale reference to 5 must not survive, got: {user_ids:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Fix 1 (Finding 11) — Int↔F64 coercion on the ON UPDATE path.
//
// `scalar_ref_matches_qv` in `fk_on_update.rs` (an independent copy from
// `fk_actions.rs`) now delegates to `scalar_ref_cmp_qv`, so a parent key stored
// as `Int(5)` matches a child FK stored as `F64(5.0)` on the update path too.
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn on_update_cascade_int_parent_f64_child_coercion() {
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
        9320,
        "child_fk_cascade_coerce",
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

    // Parent key Int(5); child FK F64(5.0) — cross-type reference.
    insert_row(
        &resolver,
        "ip",
        "parent",
        doc().set("id", 5_i64).unwrap().set("name", "P5").unwrap(),
    )
    .await;
    insert_row(
        &resolver,
        "ic",
        "child",
        doc()
            .set("cid", 50_i64)
            .unwrap()
            .set("parent_id", 5.0_f64)
            .unwrap()
            .set("label", "c")
            .unwrap(),
    )
    .await;

    // Update parent.id 5 → 7 → child.parent_id (F64) must re-key to 7 via
    // coercion (previously the strict match left the child invisible).
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5_i64))
            .set(doc().set("id", 7_i64).unwrap()),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("upd_parent"));

    assert_eq!(
        read_first_field(&resolver, "child", "parent_id").await,
        Some(QueryValue::Int(7)),
        "F64-typed child FK must be re-keyed to 7 via Int↔F64 coercion"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Fix 2 Site B — self-referential ON UPDATE actions.
//
// `fk_on_update.rs` is entirely single-level (no recursion for ANY action —
// see the module doc comment), so removing the self-ref skip is safe for all
// three actions (Restrict / Cascade / SetNull).
