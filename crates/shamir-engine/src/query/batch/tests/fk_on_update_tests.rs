//! Phase ②.2b — `ON UPDATE` referential-action enforcement tests.
//!
//! These tests exercise the on-update fan-out (Restrict / Cascade / SetNull)
//! at the batch query-runner level, plus the fast no-op gate and backward
//! compatibility with `on_update = NoAction`.
//!
//! Each test uses a UNIQUE `validator_id` (9301+) to avoid the pre-existing
//! isolation flake from validator-id-9001 reuse in the parallel delete-path
//! test suite (`fk_actions_tests` / `fk_restrict_tests`).

use std::sync::Arc;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;
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

// ── Test resolver (mirrors fk_actions_tests / fk_restrict_tests) ─────────────

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

/// Bind a `SchemaValidator` with a single FK field to a child table.
///
/// `validator_id` MUST be unique across the parallel test suite to avoid the
/// shared-registry collision flake (the registry is per-test here, but the
/// convention keeps the intent explicit and future-proofs shared registries).
#[allow(clippy::too_many_arguments)]
fn bind_fk_validator(
    db: &DbInstance,
    registry: &Arc<ValidatorRegistry>,
    table_name: &str,
    validator_id: u64,
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
    }]);

    let validator_id = RecordId::from_ts(validator_id as i64);
    registry
        .register(validator_id, validator_name, Arc::new(schema))
        .unwrap();

    let binding = ValidatorBinding {
        validator_id,
        ops: smallvec![WriteOp::Delete],
        priority: 1000,
    };

    let mut table = futures::executor::block_on(db.get_table("default", table_name)).unwrap();
    table.set_validator_registry(Arc::clone(registry));
    futures::executor::block_on(table.add_validator_binding(binding)).unwrap();
}

/// Count rows in a table via a read query.
async fn count_rows(resolver: &FkTestResolver, table_name: &str) -> usize {
    let mut b = Batch::new();
    b.id(9999);
    b.query("count", shamir_query_builder::Query::from(table_name));
    let req = b.build();
    let resp = execute_batch(&req, resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    resp.results["count"].records.len()
}

/// Read a single field value from the first row of a table.
async fn read_first_field(
    resolver: &FkTestResolver,
    table_name: &str,
    field: &str,
) -> Option<QueryValue> {
    let mut b = Batch::new();
    b.id(9998);
    b.query("q", shamir_query_builder::Query::from(table_name));
    let req = b.build();
    let resp = execute_batch(&req, resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    let records = &resp.results["q"].records;
    if records.is_empty() {
        return None;
    }
    records[0].get_value_owned(field)
}

/// Read a field value from every row of a table (preserving order).
async fn read_field_all(
    resolver: &FkTestResolver,
    table_name: &str,
    field: &str,
) -> Vec<QueryValue> {
    let mut b = Batch::new();
    b.id(9997);
    b.query("q", shamir_query_builder::Query::from(table_name));
    let req = b.build();
    let resp = execute_batch(&req, resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    resp.results["q"]
        .records
        .iter()
        .map(|r| r.get_value_owned(field).unwrap_or(QueryValue::Null))
        .collect()
}

/// Insert a single row into `table`.
async fn insert_row(
    resolver: &FkTestResolver,
    alias: &str,
    table: &str,
    doc: impl Into<QueryValue>,
) {
    let mut b = Batch::new();
    b.id(0);
    b.insert(alias, write::insert(table).row(doc));
    let req = b.build();
    execute_batch(&req, resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
}

// ═══════════════════════════════════════════════════════════════════════════════
// RESTRICT: update parent ref_field with referencing child → rejected
// ═══════════════════════════════════════════════════════════════════════════════

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
    b.update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    );
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
    b.update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    );
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
    b.update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    );
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
    b.update(
        "upd_parents",
        write::update("parent")
            .where_(filter::in_("id", [5, 6]))
            .set(doc().set("id", 99)),
    );
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
    b.update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    );
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
    b.update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    );
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
    b.update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("name", "renamed")),
    );
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
    b.update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    );
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
    b.update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 5)),
    );
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
        doc().set("id", 5).set("name", "alice"),
    )
    .await;
    insert_row(
        &resolver,
        "io",
        "orders",
        doc().set("oid", 1).set("user_id", 5).set("total", 100),
    )
    .await;
    insert_row(
        &resolver,
        "is",
        "sessions",
        doc().set("sid", 1).set("user_id", 5).set("token", "t1"),
    )
    .await;

    // Update users.id 5 → 7 → BOTH orders.user_id and sessions.user_id re-keyed.
    let mut b = Batch::new();
    b.id(1);
    b.update(
        "upd_user",
        write::update("users")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    );
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
        doc().set("id", 5).set("name", "alice"),
    )
    .await;
    insert_row(
        &resolver,
        "im",
        "messages",
        doc()
            .set("mid", 1)
            .set("sender_id", 5)
            .set("receiver_id", 5)
            .set("body", "hi"),
    )
    .await;

    // Update users.id 5 → 7 → BOTH sender_id and receiver_id re-keyed.
    let mut b = Batch::new();
    b.id(1);
    b.update(
        "upd_user",
        write::update("users")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    );
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
        doc().set("id", 5).set("name", "alice"),
    )
    .await;
    // The RESTRICT child still references the old id=5 → update must be rejected.
    insert_row(
        &resolver,
        "irc",
        "restrict_child",
        doc().set("rid", 1).set("user_id", 5).set("note", "refs-5"),
    )
    .await;

    // Update users.id 5 → 7 → RESTRICT must fire (restrict_child still refs 5).
    let mut b = Batch::new();
    b.id(1);
    b.update(
        "upd_user",
        write::update("users")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    );
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
        doc().set("id", 5).set("name", "alice"),
    )
    .await;
    insert_row(
        &resolver,
        "iu2",
        "users",
        doc().set("id", 6).set("name", "bob"),
    )
    .await;
    // order refs 5 (should be re-keyed); order refs 9 (should be untouched).
    insert_row(
        &resolver,
        "io1",
        "orders",
        doc().set("oid", 1).set("user_id", 5).set("v", "a"),
    )
    .await;
    insert_row(
        &resolver,
        "io2",
        "orders",
        doc().set("oid", 2).set("user_id", 9).set("v", "b"),
    )
    .await;

    // Re-key only alice 5→7.
    let mut b = Batch::new();
    b.id(1);
    b.update(
        "upd_user",
        write::update("users")
            .where_(filter::eq("id", 5))
            .set(doc().set("id", 7)),
    );
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
        doc().set("id", 5_i64).set("name", "P5"),
    )
    .await;
    insert_row(
        &resolver,
        "ic",
        "child",
        doc()
            .set("cid", 50_i64)
            .set("parent_id", 5.0_f64)
            .set("label", "c"),
    )
    .await;

    // Update parent.id 5 → 7 → child.parent_id (F64) must re-key to 7 via
    // coercion (previously the strict match left the child invisible).
    let mut b = Batch::new();
    b.id(1);
    b.update(
        "upd_parent",
        write::update("parent")
            .where_(filter::eq("id", 5_i64))
            .set(doc().set("id", 7_i64)),
    );
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
// ═══════════════════════════════════════════════════════════════════════════════

/// Bind a self-referential on_update FK to a single-table hierarchy.
fn bind_self_ref_on_update(
    db: &DbInstance,
    registry: &Arc<ValidatorRegistry>,
    validator_id: u64,
    validator_name: &str,
    on_update: FkAction,
) {
    let schema = SchemaValidator::new(vec![FieldRule {
        path: vec!["manager_id".to_string()],
        ty: TypeTag::Int,
        constraints: Constraints {
            foreign_key: Some(ForeignKeyRef::with_on_update("employees", "id", on_update)),
            required: false,
            nullable: true,
            ..Default::default()
        },
    }]);

    let vid = RecordId::from_ts(validator_id as i64);
    registry
        .register(vid, validator_name, Arc::new(schema))
        .unwrap();

    let binding = ValidatorBinding {
        validator_id: vid,
        ops: smallvec![WriteOp::Delete],
        priority: 1000,
    };

    let mut table = futures::executor::block_on(db.get_table("default", "employees")).unwrap();
    table.set_validator_registry(Arc::clone(registry));
    futures::executor::block_on(table.add_validator_binding(binding)).unwrap();
}

fn setup_self_ref_db() -> DbInstance {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("employees")],
    };
    futures::executor::block_on(DbInstance::with_repos(vec![repo_config])).unwrap()
}

/// Self-ref ON UPDATE CASCADE: re-keying a manager's `id` cascades the new
/// value to all subordinates' `manager_id`.
#[tokio::test]
async fn self_ref_on_update_cascade_rekeys_subordinates() {
    let db = setup_self_ref_db();
    let registry = Arc::new(ValidatorRegistry::new());
    bind_self_ref_on_update(
        &db,
        &registry,
        9321,
        "self_ref_cascade_upd",
        FkAction::Cascade,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    // CEO(id=10), Sub1(manager_id=10), Sub2(manager_id=10).
    insert_row(
        &resolver,
        "i1",
        "employees",
        doc()
            .set("id", 10_i64)
            .set("name", "CEO")
            .set("manager_id", QueryValue::Null),
    )
    .await;
    insert_row(
        &resolver,
        "i2",
        "employees",
        doc()
            .set("id", 20_i64)
            .set("name", "S1")
            .set("manager_id", 10_i64),
    )
    .await;
    insert_row(
        &resolver,
        "i3",
        "employees",
        doc()
            .set("id", 30_i64)
            .set("name", "S2")
            .set("manager_id", 10_i64),
    )
    .await;

    // Re-key CEO id 10 → 77 → both subordinates must cascade to 77.
    let mut b = Batch::new();
    b.id(1);
    b.update(
        "upd",
        write::update("employees")
            .where_(filter::eq("id", 10_i64))
            .set(doc().set("id", 77_i64)),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mgr_ids = read_field_all(&resolver, "employees", "manager_id").await;
    // CEO's manager_id is Null; both subordinates should be 77.
    assert!(
        mgr_ids
            .iter()
            .filter(|v| **v == QueryValue::Int(77))
            .count()
            == 2,
        "both subordinates must cascade to 77, got: {mgr_ids:?}"
    );
    assert!(
        !mgr_ids.contains(&QueryValue::Int(10)),
        "stale reference to old id 10 must not survive, got: {mgr_ids:?}"
    );
}

/// Self-ref ON UPDATE RESTRICT: re-keying a manager's `id` is blocked when a
/// subordinate still references the old value.
#[tokio::test]
async fn self_ref_on_update_restrict_blocks_rekey() {
    let db = setup_self_ref_db();
    let registry = Arc::new(ValidatorRegistry::new());
    bind_self_ref_on_update(
        &db,
        &registry,
        9322,
        "self_ref_restrict_upd",
        FkAction::Restrict,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_row(
        &resolver,
        "i1",
        "employees",
        doc()
            .set("id", 10_i64)
            .set("name", "CEO")
            .set("manager_id", QueryValue::Null),
    )
    .await;
    insert_row(
        &resolver,
        "i2",
        "employees",
        doc()
            .set("id", 20_i64)
            .set("name", "Sub")
            .set("manager_id", 10_i64),
    )
    .await;

    // Re-key CEO id 10 → 77 → must be REJECTED (Sub still references 10).
    let mut b = Batch::new();
    b.id(1);
    b.update(
        "upd",
        write::update("employees")
            .where_(filter::eq("id", 10_i64))
            .set(doc().set("id", 77_i64)),
    );
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;

    match resp {
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("fk_restrict"),
                "self-ref on_update restrict should block, got: {msg}"
            );
        }
        Ok(_) => panic!("Expected fk_restrict on self-ref rekey with subordinate"),
    }

    // Data unchanged after restrict rollback: both CEO (id=10) and Sub (id=20)
    // must survive with their original ids.
    let ids = read_field_all(&resolver, "employees", "id").await;
    assert!(
        ids.contains(&QueryValue::Int(10)),
        "CEO id=10 should be unchanged after restrict rollback, got: {ids:?}"
    );
    assert!(
        ids.contains(&QueryValue::Int(20)),
        "Sub id=20 should be unchanged after restrict rollback, got: {ids:?}"
    );
}

/// Self-ref ON UPDATE SET NULL: re-keying a manager's `id` nulls the
/// subordinates' `manager_id`.
#[tokio::test]
async fn self_ref_on_update_set_null_nulls_subordinates() {
    let db = setup_self_ref_db();
    let registry = Arc::new(ValidatorRegistry::new());
    bind_self_ref_on_update(
        &db,
        &registry,
        9323,
        "self_ref_setnull_upd",
        FkAction::SetNull,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_row(
        &resolver,
        "i1",
        "employees",
        doc()
            .set("id", 10_i64)
            .set("name", "CEO")
            .set("manager_id", QueryValue::Null),
    )
    .await;
    insert_row(
        &resolver,
        "i2",
        "employees",
        doc()
            .set("id", 20_i64)
            .set("name", "Sub")
            .set("manager_id", 10_i64),
    )
    .await;

    // Re-key CEO id 10 → 77 → Sub.manager_id must be nulled.
    let mut b = Batch::new();
    b.id(1);
    b.update(
        "upd",
        write::update("employees")
            .where_(filter::eq("id", 10_i64))
            .set(doc().set("id", 77_i64)),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mgr_ids = read_field_all(&resolver, "employees", "manager_id").await;
    assert!(
        mgr_ids.contains(&QueryValue::Null),
        "Sub.manager_id should be nulled, got: {mgr_ids:?}"
    );
    assert!(
        !mgr_ids.contains(&QueryValue::Int(10)),
        "stale reference to old id 10 must not survive, got: {mgr_ids:?}"
    );
}

/// Self-loop edge case: co-updated parent rows must NOT be incorrectly
/// re-matched as their own children.  Two roots are updated in one operation;
/// neither references the other's old value, so neither should be cascaded —
/// only the genuine child (which references one of the old values) is affected.
///
/// Hierarchy:
///   R1(id=10, manager_id=null)  — root, co-updated
///   R2(id=20, manager_id=null)  — root, co-updated (does NOT ref R1)
///   R3(id=30, manager_id=10)    — genuine child of R1, NOT co-updated
///
/// UPDATE employees SET id=99 WHERE id IN (10, 20):
///   • old values = {10, 20}
///   • R1.manager_id=null → NOT matched (co-updated parent, no ref) ✓
///   • R2.manager_id=null → NOT matched (co-updated parent, no ref) ✓
///   • R3.manager_id=10 → matched (genuine child of R1) → cascade to 99 ✓
#[tokio::test]
async fn self_ref_on_update_self_loop_overlapping_parent_and_child() {
    let db = setup_self_ref_db();
    let registry = Arc::new(ValidatorRegistry::new());
    bind_self_ref_on_update(&db, &registry, 9324, "self_ref_loop_upd", FkAction::Cascade);

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    insert_row(
        &resolver,
        "i1",
        "employees",
        doc()
            .set("id", 10_i64)
            .set("name", "R1")
            .set("manager_id", QueryValue::Null),
    )
    .await;
    insert_row(
        &resolver,
        "i2",
        "employees",
        doc()
            .set("id", 20_i64)
            .set("name", "R2")
            .set("manager_id", QueryValue::Null),
    )
    .await;
    insert_row(
        &resolver,
        "i3",
        "employees",
        doc()
            .set("id", 30_i64)
            .set("name", "R3")
            .set("manager_id", 10_i64),
    )
    .await;

    // Re-key BOTH R1 (10→99) and R2 (20→99) in one operation.
    let mut b = Batch::new();
    b.id(1);
    b.update(
        "upd",
        write::update("employees")
            .where_(filter::in_("id", [10_i64, 20_i64]))
            .set(doc().set("id", 99_i64)),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let ids = read_field_all(&resolver, "employees", "id").await;
    let mgr_ids = read_field_all(&resolver, "employees", "manager_id").await;

    // 3 rows survive.
    assert_eq!(ids.len(), 3);

    // Exactly TWO rows now have id=99 (R1 and R2 were re-keyed).
    assert_eq!(
        ids.iter().filter(|v| **v == QueryValue::Int(99)).count(),
        2,
        "R1 and R2 should be re-keyed to 99, got ids: {ids:?}"
    );
    // R3 keeps id=30.
    assert!(
        ids.contains(&QueryValue::Int(30)),
        "R3 (not matched by WHERE) keeps id=30, got ids: {ids:?}"
    );

    // Only R3 (the genuine child of R1) should have manager_id=99. R1 and R2
    // (co-updated parents with manager_id=null) must NOT have been incorrectly
    // re-matched as children — their manager_id stays Null.
    assert_eq!(
        mgr_ids
            .iter()
            .filter(|v| **v == QueryValue::Int(99))
            .count(),
        1,
        "only R3 (genuine child) should be cascaded to 99, got: {mgr_ids:?}"
    );
    assert_eq!(
        mgr_ids.iter().filter(|v| **v == QueryValue::Null).count(),
        2,
        "R1 and R2 (co-updated parents) must keep manager_id=Null, got: {mgr_ids:?}"
    );
    assert!(
        !mgr_ids.contains(&QueryValue::Int(10)),
        "no stale ref to old id 10, got: {mgr_ids:?}"
    );
}
