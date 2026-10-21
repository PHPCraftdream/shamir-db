use std::sync::Arc;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_types::access::Actor;
use shamir_types::types::value::QueryValue;

use super::support::{
    bind_fk_validator, count_rows, insert_row, read_field_all, read_first_field, FkTestResolver,
};
use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::execute_batch_unchecked as execute_batch;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::RepoConfig;
use crate::table::TableConfig;
use crate::validator::ValidatorRegistry;

#[tokio::test]
async fn on_update_restrict_rejects_when_child_references_old() {
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
        9301,
        "child_fk_restrict_update",
        "parent_id",
        "parent",
        "id",
        FkAction::Restrict,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    // parent(id=5), child(parent_id=5).
    insert_row(
        &resolver,
        "ip",
        "parent",
        doc().set("id", 5).set("name", "P5"),
    )
    .await;
    insert_row(
        &resolver,
        "ic",
        "child",
        doc().set("cid", 50).set("parent_id", 5).set("label", "c50"),
    )
    .await;

    // Update parent.id 5 → 7 while child still references 5 → must reject.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
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
        Ok(_) => panic!("Expected fk_restrict rejection, got success"),
    }

    // Parent unchanged (rollback).
    assert_eq!(
        read_first_field(&resolver, "parent", "id").await,
        Some(QueryValue::Int(5))
    );
}

#[tokio::test]
async fn on_update_restrict_passes_when_no_child_references_old() {
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
        9302,
        "child_fk_restrict_update_pass",
        "parent_id",
        "parent",
        "id",
        FkAction::Restrict,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    // parent(id=5) with NO child referencing it.
    insert_row(
        &resolver,
        "ip",
        "parent",
        doc().set("id", 5).set("name", "P5"),
    )
    .await;

    // Update parent.id 5 → 7 → should succeed (no references).
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("upd_parent"));

    assert_eq!(
        read_first_field(&resolver, "parent", "id").await,
        Some(QueryValue::Int(7))
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// CASCADE: update parent id 5→7 → child FK re-keyed 5→7
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn on_update_cascade_rekeys_child_fk() {
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
        9303,
        "child_fk_cascade_update",
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

    insert_row(
        &resolver,
        "ip",
        "parent",
        doc().set("id", 5).set("name", "P5"),
    )
    .await;
    insert_row(
        &resolver,
        "ic",
        "child",
        doc().set("cid", 50).set("parent_id", 5).set("label", "c50"),
    )
    .await;

    assert_eq!(
        read_first_field(&resolver, "child", "parent_id").await,
        Some(QueryValue::Int(5))
    );

    // Update parent.id 5 → 7 → child.parent_id should become 7.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("upd_parent"));

    assert_eq!(
        read_first_field(&resolver, "parent", "id").await,
        Some(QueryValue::Int(7))
    );
    assert_eq!(
        read_first_field(&resolver, "child", "parent_id").await,
        Some(QueryValue::Int(7))
    );
    assert_eq!(count_rows(&resolver, "child").await, 1);
}

#[tokio::test]
async fn on_update_cascade_rekeys_multiple_children_and_rows() {
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
        9304,
        "child_fk_cascade_multi",
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

    // Two parents (id=5, id=6), three children (two referencing 5, one 6).
    insert_row(
        &resolver,
        "ip1",
        "parent",
        doc().set("id", 5).set("name", "P5"),
    )
    .await;
    insert_row(
        &resolver,
        "ip2",
        "parent",
        doc().set("id", 6).set("name", "P6"),
    )
    .await;
    insert_row(
        &resolver,
        "ic1",
        "child",
        doc().set("cid", 1).set("parent_id", 5).set("label", "a"),
    )
    .await;
    insert_row(
        &resolver,
        "ic2",
        "child",
        doc().set("cid", 2).set("parent_id", 5).set("label", "b"),
    )
    .await;
    insert_row(
        &resolver,
        "ic3",
        "child",
        doc().set("cid", 3).set("parent_id", 6).set("label", "c"),
    )
    .await;

    // Re-key BOTH parents (5→8 and 6→9) via a where that matches both.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_parents",
        write::update("parent")
            .where_(filter::in_("id", [5, 6]))
            .set(doc().set("id", 99)),
    )
    .unwrap();
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Every child now references 99.
    let parent_ids = read_field_all(&resolver, "child", "parent_id").await;
    assert_eq!(parent_ids.len(), 3);
    assert!(
        parent_ids.iter().all(|v| *v == QueryValue::Int(99)),
        "expected all children re-keyed to 99, got: {parent_ids:?}"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// SET NULL: update parent ref_field → child FK nulled
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn on_update_set_null_nulls_child_fk() {
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
        9305,
        "child_fk_setnull_update",
        "parent_id",
        "parent",
        "id",
        FkAction::SetNull,
        true, // nullable
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_row(
        &resolver,
        "ip",
        "parent",
        doc().set("id", 5).set("name", "P5"),
    )
    .await;
    insert_row(
        &resolver,
        "ic",
        "child",
        doc().set("cid", 50).set("parent_id", 5).set("label", "c50"),
    )
    .await;

    // Update parent.id 5 → 7 → child.parent_id should become Null.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("upd_parent"));

    // Child survives with parent_id == Null.
    assert_eq!(count_rows(&resolver, "child").await, 1);
    assert_eq!(
        read_first_field(&resolver, "child", "parent_id").await,
        Some(QueryValue::Null)
    );
}

#[tokio::test]
async fn on_update_set_null_on_non_nullable_errors() {
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
        9306,
        "child_fk_setnull_update_nn",
        "parent_id",
        "parent",
        "id",
        FkAction::SetNull,
        false, // NOT nullable
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_row(
        &resolver,
        "ip",
        "parent",
        doc().set("id", 5).set("name", "P5"),
    )
    .await;
    insert_row(
        &resolver,
        "ic",
        "child",
        doc().set("cid", 50).set("parent_id", 5).set("label", "c50"),
    )
    .await;

    // Update parent.id 5 → 7 → SetNull on non-nullable field → error.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;

    match resp {
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("set_null_requires_nullable"),
                "expected set_null_requires_nullable error, got: {msg}"
            );
        }
        Ok(_) => panic!("Expected set_null_requires_nullable rejection, got success"),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// NO-OP GATE: update does not touch ref_field → children untouched, zero fan-out
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn on_update_noop_when_set_does_not_touch_ref_field() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    // Cascade FK — if the no-op gate fails, the child would be re-keyed.
    bind_fk_validator(
        &db,
        &registry,
        "child",
        9307,
        "child_fk_cascade_noop",
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

    insert_row(
        &resolver,
        "ip",
        "parent",
        doc().set("id", 5).set("name", "P5"),
    )
    .await;
    insert_row(
        &resolver,
        "ic",
        "child",
        doc().set("cid", 50).set("parent_id", 5).set("label", "c50"),
    )
    .await;

    // Update parent.name (NOT id) → no fan-out, child.parent_id stays 5.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("name", "renamed")),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("upd_parent"));

    assert_eq!(
        read_first_field(&resolver, "parent", "name").await,
        Some(QueryValue::Str("renamed".to_string()))
    );
    assert_eq!(
        read_first_field(&resolver, "child", "parent_id").await,
        Some(QueryValue::Int(5)),
        "child FK must be untouched when update does not touch ref_field"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// BACK-COMPAT: on_update = NoAction → update parent does not fan out
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn on_update_no_action_no_fanout() {
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
        9308,
        "child_fk_noaction_update",
        "parent_id",
        "parent",
        "id",
        FkAction::NoAction,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_row(
        &resolver,
        "ip",
        "parent",
        doc().set("id", 5).set("name", "P5"),
    )
    .await;
    insert_row(
        &resolver,
        "ic",
        "child",
        doc().set("cid", 50).set("parent_id", 5).set("label", "c50"),
    )
    .await;

    // Update parent.id 5 → 7 → child.parent_id must stay 5 (NoAction).
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("upd_parent"));

    assert_eq!(
        read_first_field(&resolver, "parent", "id").await,
        Some(QueryValue::Int(7))
    );
    assert_eq!(
        read_first_field(&resolver, "child", "parent_id").await,
        Some(QueryValue::Int(5)),
        "child FK must be untouched with on_update = NoAction"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// old == new: update assigns the same value → no fan-out (no spurious re-key)
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn on_update_cascade_skips_when_old_equals_new() {
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
        9309,
        "child_fk_cascade_oldeqnew",
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

    insert_row(
        &resolver,
        "ip",
        "parent",
        doc().set("id", 5).set("name", "P5"),
    )
    .await;
    insert_row(
        &resolver,
        "ic",
        "child",
        doc().set("cid", 50).set("parent_id", 5).set("label", "c50"),
    )
    .await;

    // Update parent.id 5 → 5 (no-op value) → no fan-out.
    let mut b = Batch::new();
    b.id(1);
    b.try_update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 5)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("upd_parent"));

    assert_eq!(
        read_first_field(&resolver, "child", "parent_id").await,
        Some(QueryValue::Int(5))
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// MULTI-REF (shared parent ref_field): regression for the `parent_ref_field`-
// only dedup bug.  Previously `relevant_refs` itself was deduped by
// `parent_ref_field`, which silently collapsed distinct FK references that
// happened to share a parent field to a single ref — so only ONE got its
// cascade/setnull/restrict action applied (the rest, possibly a RESTRICT, were
// dropped entirely).  The fix keeps `relevant_refs` intact and dedups only the
// derived field-name lists (mirroring the delete path in `fk_actions.rs`).
