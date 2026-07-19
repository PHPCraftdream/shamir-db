//! Phase D.2 / D.3 — CASCADE + SET NULL + drop-guard tests.
//!
//! These tests exercise the cascade/setnull actions at the batch query runner
//! level, plus the drop-table / drop-function reverse-reference guards.

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

// ── Test resolver (same as fk_restrict_tests) ────────────────────────────────

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

/// Bind a SchemaValidator with a single FK field to a child table.
#[allow(clippy::too_many_arguments)]
fn bind_fk_validator(
    db: &DbInstance,
    registry: &Arc<ValidatorRegistry>,
    table_name: &str,
    validator_name: &str,
    field: &str,
    ref_table: &str,
    ref_field: &str,
    on_delete: FkAction,
    nullable: bool,
) {
    let schema = SchemaValidator::new(vec![FieldRule {
        path: vec![field.to_string()],
        ty: TypeTag::Int,
        constraints: Constraints {
            foreign_key: Some(ForeignKeyRef::with_on_delete(
                ref_table, ref_field, on_delete,
            )),
            required: !nullable,
            nullable,
            ..Default::default()
        },
    }]);

    let validator_id = RecordId::from_ts(9001);
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
) -> Option<shamir_types::types::value::QueryValue> {
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

// ═══════════════════════════════════════════════════════════════════════════════
// CASCADE: delete parent → child also deleted
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cascade_deletes_child_when_parent_deleted() {
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
        "child_fk_cascade",
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

    // Insert parent + child.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins_child",
        write::insert("child").row(doc().set("cid", 10).set("parent_id", 1).set("label", "c1")),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_eq!(count_rows(&resolver, "child").await, 1);

    // Delete parent → child should also be deleted (Cascade).
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
    assert!(resp.results.contains_key("del_parent"));

    // Parent gone.
    assert_eq!(count_rows(&resolver, "parent").await, 0);
    // Child also gone (cascade).
    assert_eq!(count_rows(&resolver, "child").await, 0);
}

// ═══════════════════════════════════════════════════════════════════════════════
// CASCADE chain: A→B→C, deleting A removes B and C
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cascade_chain_a_to_b_to_c() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![
            TableConfig::new("a"),
            TableConfig::new("b"),
            TableConfig::new("c"),
        ],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    // B has FK→A (Cascade), C has FK→B (Cascade).
    bind_fk_validator(
        &db,
        &registry,
        "b",
        "b_fk_a",
        "a_id",
        "a",
        "id",
        FkAction::Cascade,
        true,
    );
    bind_fk_validator(
        &db,
        &registry,
        "c",
        "c_fk_b",
        "b_id",
        "b",
        "id",
        FkAction::Cascade,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    // Insert A, B, C.
    let mut b = Batch::new();
    b.id(1);
    b.insert("ia", write::insert("a").row(doc().set("id", 1)));
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ib",
        write::insert("b").row(doc().set("id", 2).set("a_id", 1)),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(3);
    b.insert(
        "ic",
        write::insert("c").row(doc().set("id", 3).set("b_id", 2)),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_eq!(count_rows(&resolver, "a").await, 1);
    assert_eq!(count_rows(&resolver, "b").await, 1);
    assert_eq!(count_rows(&resolver, "c").await, 1);

    // Delete A → B and C should also be cascade-deleted.
    let mut b = Batch::new();
    b.id(4);
    b.delete("da", write::delete("a").where_(filter::eq("id", 1)));
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("da"));

    assert_eq!(count_rows(&resolver, "a").await, 0);
    assert_eq!(count_rows(&resolver, "b").await, 0);
    assert_eq!(count_rows(&resolver, "c").await, 0);
}

// ═══════════════════════════════════════════════════════════════════════════════
// CASCADE cycle: A→B→A, depth-guard error, no partial corruption
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cascade_cycle_triggers_depth_guard() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("x"), TableConfig::new("y")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    // X has FK→Y (Cascade), Y has FK→X (Cascade) — a cycle.
    bind_fk_validator(
        &db,
        &registry,
        "x",
        "x_fk_y",
        "y_id",
        "y",
        "id",
        FkAction::Cascade,
        true,
    );
    bind_fk_validator(
        &db,
        &registry,
        "y",
        "y_fk_x",
        "x_id",
        "x",
        "id",
        FkAction::Cascade,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    // Insert X(id=1, y_id=2), Y(id=2, x_id=1).
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ix",
        write::insert("x").row(doc().set("id", 1).set("y_id", 2)),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "iy",
        write::insert("y").row(doc().set("id", 2).set("x_id", 1)),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Delete X → cascade should recurse X→Y→X→Y... and hit depth guard.
    let mut b = Batch::new();
    b.id(3);
    b.delete("dx", write::delete("x").where_(filter::eq("id", 1)));
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;

    // The batch should fail with fk_cascade_depth.
    match resp {
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("fk_cascade_depth"),
                "expected fk_cascade_depth error, got: {msg}"
            );
        }
        Ok(r) => {
            // Check the per-alias error.
            let _ = r;
            panic!("Expected fk_cascade_depth error on cycle");
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// CASCADE diamond: A←B←D and A←C←D (D reachable via two distinct paths)
//
// This is a legal acyclic DAG (diamond), NOT a cycle.  Before the per-path
// cycle-guard fix, the global `visited` set kept "D" after the B-branch
// returned, so the C-branch's attempt to cascade through D tripped a false
// `fk_cascade_depth` error — aborting a perfectly legal DELETE.
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn cascade_diamond_topology_succeeds() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![
            TableConfig::new("a"),
            TableConfig::new("b"),
            TableConfig::new("c"),
            TableConfig::new("d"),
        ],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    // B.a_id CASCADE→A, C.a_id CASCADE→A (two independent branches).
    bind_fk_validator(
        &db,
        &registry,
        "b",
        "b_fk_a",
        "a_id",
        "a",
        "id",
        FkAction::Cascade,
        true,
    );
    bind_fk_validator(
        &db,
        &registry,
        "c",
        "c_fk_a",
        "a_id",
        "a",
        "id",
        FkAction::Cascade,
        true,
    );
    // D.b_id CASCADE→B, D.c_id CASCADE→C — D is reachable from A via BOTH
    // branches, forming a diamond: A ← B ← D and A ← C ← D.
    bind_fk_validator(
        &db,
        &registry,
        "d",
        "d_fk_b",
        "b_id",
        "b",
        "id",
        FkAction::Cascade,
        true,
    );
    bind_fk_validator(
        &db,
        &registry,
        "d",
        "d_fk_c",
        "c_id",
        "c",
        "id",
        FkAction::Cascade,
        true,
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    // Insert A(id=1), B(id=2, a_id=1), C(id=3, a_id=1), D(id=4, b_id=2, c_id=3).
    let mut b = Batch::new();
    b.id(1);
    b.insert("ia", write::insert("a").row(doc().set("id", 1)));
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ib",
        write::insert("b").row(doc().set("id", 2).set("a_id", 1)),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(3);
    b.insert(
        "ic",
        write::insert("c").row(doc().set("id", 3).set("a_id", 1)),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(4);
    b.insert(
        "id_row",
        write::insert("d").row(doc().set("id", 4).set("b_id", 2).set("c_id", 3)),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_eq!(count_rows(&resolver, "a").await, 1);
    assert_eq!(count_rows(&resolver, "b").await, 1);
    assert_eq!(count_rows(&resolver, "c").await, 1);
    assert_eq!(count_rows(&resolver, "d").await, 1);

    // Delete A → cascade through B and C, both reaching D.
    // This must SUCCEED (not error with fk_cascade_depth), and D must be
    // deleted exactly once (no double-delete error mid-cascade).
    let mut b = Batch::new();
    b.id(5);
    b.delete("da", write::delete("a").where_(filter::eq("id", 1)));
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("da"));

    // All four tables should be empty — the whole diamond was cascade-deleted.
    assert_eq!(count_rows(&resolver, "a").await, 0);
    assert_eq!(count_rows(&resolver, "b").await, 0);
    assert_eq!(count_rows(&resolver, "c").await, 0);
    assert_eq!(count_rows(&resolver, "d").await, 0);
}

// ═══════════════════════════════════════════════════════════════════════════════
// SET NULL: delete parent → child survives with FK field == Null
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn set_null_nulls_child_field_when_parent_deleted() {
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
        "child_fk_setnull",
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

    // Insert parent + child.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins_child",
        write::insert("child").row(doc().set("cid", 10).set("parent_id", 1).set("label", "c1")),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_eq!(count_rows(&resolver, "child").await, 1);

    // Delete parent → child should survive with parent_id == Null.
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
    assert!(resp.results.contains_key("del_parent"));

    // Parent gone.
    assert_eq!(count_rows(&resolver, "parent").await, 0);
    // Child survives.
    assert_eq!(count_rows(&resolver, "child").await, 1);
    // parent_id is now Null.
    let val = read_first_field(&resolver, "child", "parent_id").await;
    assert_eq!(val, Some(shamir_types::types::value::QueryValue::Null));
}

// ═══════════════════════════════════════════════════════════════════════════════
// SET NULL on non-nullable field → error
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn set_null_on_non_nullable_field_errors() {
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
        "child_fk_setnull_nn",
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

    // Insert parent + child.
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1).set("name", "Alice")),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins_child",
        write::insert("child").row(doc().set("cid", 10).set("parent_id", 1).set("label", "c1")),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Delete parent → should fail with set_null_requires_nullable.
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
                msg.contains("set_null_requires_nullable"),
                "expected set_null_requires_nullable error, got: {msg}"
            );
        }
        Ok(_) => panic!("Expected set_null_requires_nullable error"),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Fix 1 (Finding 11) — Int↔F64 coercion in cascade/setnull child matching.
//
// `scalar_ref_matches_qv` previously did exact same-variant matching only, so a
// parent key stored as `Int(5)` failed to match a child FK stored as `F64(5.0)`
// (and vice-versa) — the child was invisible to cascade scans and silently
// survived a parent delete with a dangling reference.  Both copies (this file +
// `fk_on_update.rs`) now delegate to `scalar_ref_cmp_qv`, which bridges the
// Int/F64 divide consistently with every other comparison layer.
// ═══════════════════════════════════════════════════════════════════════════════

/// CASCADE: parent key `Int(1)`, child FK field `F64(1.0)` — the cascade scan
/// must bridge the type divide and actually delete the child row.
#[tokio::test]
async fn cascade_int_parent_f64_child_coercion() {
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
        "child_fk_cascade_coerce_int_f64",
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

    // Parent key stored as Int(1).
    insert_helper(&resolver, "parent", doc().set("id", 1_i64).set("name", "P")).await;
    // Child FK field stored as F64(1.0) — cross-type reference.
    insert_helper(
        &resolver,
        "child",
        doc()
            .set("cid", 10_i64)
            .set("parent_id", 1.0_f64)
            .set("label", "c"),
    )
    .await;

    assert_eq!(count_rows(&resolver, "child").await, 1);

    // Delete parent (Int(1)) → child (F64(1.0)) must cascade-delete.
    let mut b = Batch::new();
    b.id(3);
    b.delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    );
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("del_parent"));

    assert_eq!(count_rows(&resolver, "parent").await, 0);
    assert_eq!(
        count_rows(&resolver, "child").await,
        0,
        "F64-typed child FK must cascade-delete with Int-typed parent key"
    );
}

/// CASCADE: reverse direction — parent key `F64(1.0)`, child FK field `Int(1)`.
#[tokio::test]
async fn cascade_f64_parent_int_child_coercion() {
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
        "child_fk_cascade_coerce_f64_int",
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

    // Parent key stored as F64(1.0).
    insert_helper(
        &resolver,
        "parent",
        doc().set("id", 1.0_f64).set("name", "P"),
    )
    .await;
    // Child FK field stored as Int(1).
    insert_helper(
        &resolver,
        "child",
        doc()
            .set("cid", 10_i64)
            .set("parent_id", 1_i64)
            .set("label", "c"),
    )
    .await;

    assert_eq!(count_rows(&resolver, "child").await, 1);

    // Delete parent (F64(1.0)) → child (Int(1)) must cascade-delete.
    let mut b = Batch::new();
    b.id(3);
    b.delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1.0_f64)),
    );
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("del_parent"));

    assert_eq!(count_rows(&resolver, "parent").await, 0);
    assert_eq!(
        count_rows(&resolver, "child").await,
        0,
        "Int-typed child FK must cascade-delete with F64-typed parent key"
    );
}

/// SET NULL: parent key `Int(1)`, child FK field `F64(1.0)` — the SetNull scan
/// must coerce and null the child field (not silently leave a dangling ref).
#[tokio::test]
async fn set_null_int_parent_f64_child_coercion() {
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
        "child_fk_setnull_coerce",
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

    insert_helper(&resolver, "parent", doc().set("id", 1_i64).set("name", "P")).await;
    insert_helper(
        &resolver,
        "child",
        doc()
            .set("cid", 10_i64)
            .set("parent_id", 1.0_f64)
            .set("label", "c"),
    )
    .await;

    // Delete parent → child survives with parent_id == Null (coercion applied).
    let mut b = Batch::new();
    b.id(3);
    b.delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    );
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("del_parent"));

    assert_eq!(count_rows(&resolver, "parent").await, 0);
    assert_eq!(count_rows(&resolver, "child").await, 1);
    let val = read_first_field(&resolver, "child", "parent_id").await;
    assert_eq!(
        val,
        Some(shamir_types::types::value::QueryValue::Null),
        "F64-typed child FK must be nulled via coercion"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Fix 2 Site C — self-referential ON DELETE SET NULL.
//
// Self-referential SET NULL is single-level (never recurses), so it is safe to
// enable.  Deleting a manager with direct subordinates nulls their manager_id;
// a subordinate's OWN subordinates are NOT touched (single-level, matching
// existing non-self-ref SetNull semantics).
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn self_referential_set_null_nulls_direct_subordinates() {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("employees")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();
    let registry = Arc::new(ValidatorRegistry::new());

    // employees.manager_id → employees.id ON DELETE SET NULL (self-ref).
    bind_fk_validator(
        &db,
        &registry,
        "employees",
        "self_ref_setnull",
        "manager_id",
        "employees",
        "id",
        FkAction::SetNull,
        true, // manager_id is nullable
    );

    let resolver = FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    };

    // Build a 3-level hierarchy: CEO(1) ← Mgr(2) ← Worker(3).
    insert_helper(
        &resolver,
        "employees",
        doc()
            .set("id", 1_i64)
            .set("name", "CEO")
            .set("manager_id", QueryValue::Null),
    )
    .await;
    insert_helper(
        &resolver,
        "employees",
        doc()
            .set("id", 2_i64)
            .set("name", "Mgr")
            .set("manager_id", 1_i64),
    )
    .await;
    insert_helper(
        &resolver,
        "employees",
        doc()
            .set("id", 3_i64)
            .set("name", "Worker")
            .set("manager_id", 2_i64),
    )
    .await;

    assert_eq!(count_rows(&resolver, "employees").await, 3);

    // Delete CEO (id=1) → Mgr's manager_id must be nulled (direct subordinate).
    // Worker's manager_id must be UNCHANGED (single-level: Worker is a
    // grandchild, not a direct child of CEO).
    let mut b = Batch::new();
    b.id(4);
    b.delete(
        "del_ceo",
        write::delete("employees").where_(filter::eq("id", 1_i64)),
    );
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(resp.results.contains_key("del_ceo"));

    // 2 rows survive (Mgr + Worker); CEO is gone.
    assert_eq!(count_rows(&resolver, "employees").await, 2);

    // Read all manager_id values to check Mgr was nulled but Worker untouched.
    let manager_ids = read_all_field(&resolver, "employees", "manager_id").await;
    // Exactly one Null (Mgr's manager_id) and one Int(2) (Worker's manager_id).
    assert!(
        manager_ids.contains(&QueryValue::Null),
        "Mgr's manager_id should be nulled, got: {manager_ids:?}"
    );
    assert!(
        manager_ids.contains(&QueryValue::Int(2)),
        "Worker's manager_id should be untouched (single-level), got: {manager_ids:?}"
    );
    assert!(
        !manager_ids.contains(&QueryValue::Int(1)),
        "no dangling reference to deleted CEO id=1 should survive, got: {manager_ids:?}"
    );
}

// ── local helpers ────────────────────────────────────────────────────────────

/// Insert a single row (mirrors fk_on_update_tests::insert_row but local to
/// this file to avoid a cross-file dependency).
async fn insert_helper(resolver: &FkTestResolver, table: &str, doc: impl Into<QueryValue>) {
    let mut b = Batch::new();
    b.id(0);
    b.insert("ins", write::insert(table).row(doc));
    let req = b.build();
    execute_batch(&req, resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
}

/// Read a field value from every row of a table (mirrors fk_on_update_tests).
async fn read_all_field(
    resolver: &FkTestResolver,
    table_name: &str,
    field: &str,
) -> Vec<QueryValue> {
    let mut b = Batch::new();
    b.id(9996);
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
