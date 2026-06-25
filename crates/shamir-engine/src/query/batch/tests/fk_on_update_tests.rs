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
