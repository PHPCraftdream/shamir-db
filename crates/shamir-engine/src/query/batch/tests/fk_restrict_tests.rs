//! Phase D.1 — ON DELETE RESTRICT gate tests.
//!
//! These tests exercise the reverse-FK discovery + restrict gate at the batch
//! query runner level.  The child table declares a foreign_key on `parent_id`
//! referencing `parent.id` with `on_delete = Restrict`.

use std::sync::Arc;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::filter;
use shamir_query_builder::write;
use shamir_query_builder::write::doc;
use shamir_query_types::admin::FkAction;
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
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

// ── Test resolver that injects a shared ValidatorRegistry ────────────────────

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

/// Build a test environment with parent + child tables.
///
/// The child table gets a SchemaValidator with a FK(parent, id, on_delete)
/// bound as a validator.
async fn setup_fk_test(on_delete: FkAction) -> FkTestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();

    // Build a validator registry with a SchemaValidator for the child table.
    let registry = Arc::new(ValidatorRegistry::new());

    let child_schema = SchemaValidator::new(vec![FieldRule {
        path: vec!["parent_id".to_string()],
        ty: TypeTag::Int,
        constraints: Constraints {
            foreign_key: Some(ForeignKeyRef::with_on_delete("parent", "id", on_delete)),
            required: true,
            ..Default::default()
        },
    }]);

    let validator_id = RecordId::from_ts(9001);
    registry
        .register(validator_id, "child_fk_schema", Arc::new(child_schema))
        .unwrap();

    // Bind the validator to the child table. We do NOT include Insert/Update
    // in ops because the forward-FK enforcement path (SchemaValidator.validate)
    // requires a resolver wired into the ValidatorDb, which the implicit
    // (non-tx) write path does not provide. The binding's ops are irrelevant
    // for the RESTRICT gate: `collect_fk_refs()` reads FK metadata from the
    // validator regardless of which ops it fires on.
    let binding = ValidatorBinding {
        validator_id,
        ops: smallvec![WriteOp::Delete],
        priority: 1000,
    };

    // Get child table, set registry + binding, then it will be cached.
    let mut child_table = db.get_table("default", "child").await.unwrap();
    child_table.set_validator_registry(Arc::clone(&registry));
    child_table.add_validator_binding(binding).await.unwrap();

    FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 1. Restrict: delete parent with existing child → rejected
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn restrict_rejects_parent_delete_when_child_exists() {
    let resolver = setup_fk_test(FkAction::Restrict).await;

    // Insert a parent row.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(resp.results["ins_parent"].records.len(), 1);

    // Insert a child row referencing the parent.
    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins_child",
        write::insert("child").row(doc().set("parent_id", 1).set("label", "x")),
    );
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert_eq!(resp.results["ins_child"].records.len(), 1);

    // Try to delete the parent → should be rejected by the RESTRICT gate.
    let mut b = Batch::new();
    b.id(3);
    b.delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    );
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;

    match resp {
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("fk_restrict"),
                "error should contain 'fk_restrict', got: {msg}"
            );
        }
        Ok(r) => {
            // The response might come back with an error in the results map
            // (batch returns partial errors per-alias).
            let del_result = &r.results["del_parent"];
            // If no error, this is a test failure.
            panic!("Expected fk_restrict error, got success: {:?}", del_result);
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// 2. Restrict: delete child first, then parent → succeeds
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn restrict_allows_parent_delete_after_child_removed() {
    let resolver = setup_fk_test(FkAction::Restrict).await;

    // Insert parent.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Insert child referencing parent.
    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins",
        write::insert("child").row(doc().set("parent_id", 1).set("label", "x")),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Delete the child first.
    let mut b = Batch::new();
    b.id(3);
    b.delete(
        "del_child",
        write::delete("child").where_(filter::eq("parent_id", 1)),
    );
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(
        resp.results.contains_key("del_child"),
        "child delete should succeed"
    );

    // Now delete the parent → should succeed (no more children).
    let mut b = Batch::new();
    b.id(4);
    b.delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    );
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(
        resp.results.contains_key("del_parent"),
        "parent delete should succeed after child removed"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 3. NoAction FK → parent delete succeeds even with child
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn no_action_fk_allows_parent_delete() {
    let resolver = setup_fk_test(FkAction::NoAction).await;

    // Insert parent.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Insert child referencing parent.
    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins",
        write::insert("child").row(doc().set("parent_id", 1).set("label", "x")),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Delete parent → should succeed because on_delete = NoAction.
    let mut b = Batch::new();
    b.id(3);
    b.delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    );
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(
        resp.results.contains_key("del_parent"),
        "parent delete should succeed with NoAction FK"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// 4. No referencers at all → parent delete succeeds
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn unreferenced_parent_deletes_fine() {
    let resolver = setup_fk_test(FkAction::Restrict).await;

    // Insert parent only (no child).
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Delete parent → should succeed (no children exist).
    let mut b = Batch::new();
    b.id(2);
    b.delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    );
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(
        resp.results.contains_key("del_parent"),
        "parent delete should succeed with no children"
    );
}
