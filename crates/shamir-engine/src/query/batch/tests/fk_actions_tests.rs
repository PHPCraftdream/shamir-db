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
        keyset_safe: false,
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
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    )
    .unwrap();
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
    b.try_delete("da", write::delete("a").where_(filter::eq("id", 1)))
        .unwrap();
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
    b.try_delete("dx", write::delete("x").where_(filter::eq("id", 1)))
        .unwrap();
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
    b.try_delete("da", write::delete("a").where_(filter::eq("id", 1)))
        .unwrap();
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
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    )
    .unwrap();
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
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
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
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
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
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1.0_f64)),
    )
    .unwrap();
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
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
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
    b.try_delete(
        "del_ceo",
        write::delete("employees").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
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

// ═══════════════════════════════════════════════════════════════════════════════
// F-28 Step 2 (D1) — transactional [insert child; delete parent] under CASCADE
// must NOT leave an orphan: the cascade plan must see the child row staged
// EARLIER in the same tx (read-your-own-writes) and include it in the
// cascade, deleting it alongside the parent.
//
// Before F-28 Step 2, `plan_cascade`'s row-level discovery
// (`collect_parent_values` / the child scan in `plan_cascade_recursive`) read
// committed-only state, so a child row inserted earlier in the SAME
// transaction was invisible to the cascade plan — the parent delete would
// commit, but the newly-inserted child would silently survive as an orphan
// (still referencing a now-deleted parent id).
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn transactional_insert_child_then_delete_parent_cascades_no_orphan() {
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
        "child_fk_cascade_tx",
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

    // Parent already exists (committed, autocommit).
    insert_helper(&resolver, "parent", doc().set("id", 1_i64).set("name", "P")).await;
    assert_eq!(count_rows(&resolver, "parent").await, 1);
    assert_eq!(count_rows(&resolver, "child").await, 0);

    // ONE transactional batch: insert a child referencing the parent, THEN
    // delete the parent. The cascade plan (triggered by the delete) must see
    // the child inserted EARLIER in this same tx and cascade-delete it too —
    // otherwise it survives, orphaned, referencing a deleted parent id.
    let mut b = Batch::new();
    b.id(2);
    b.transactional();
    b.insert(
        "ins_child",
        write::insert("child").row(
            doc()
                .set("cid", 10_i64)
                .set("parent_id", 1_i64)
                .set("label", "c1"),
        ),
    );
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let info = resp.transaction.expect("transaction info present");
    assert!(
        info.is_committed(),
        "transactional [insert child; delete parent] under CASCADE must commit, got: {info:?}"
    );

    // Parent gone.
    assert_eq!(count_rows(&resolver, "parent").await, 0);
    // Child must ALSO be gone — cascaded, not left as an orphan.
    assert_eq!(
        count_rows(&resolver, "child").await,
        0,
        "child inserted earlier in the SAME tx must be visible to the cascade \
         plan and cascade-deleted alongside the parent — no orphan should survive"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// F-28 Step 2 (D1) — a child row staged as an UPDATE (re-keying its FK value
// to point elsewhere) EARLIER in the same tx must be reflected by the cascade
// plan using the UPDATED reference, not the stale committed one.
//
// Setup: two parents (1, 2), one child initially referencing parent 1. In one
// transactional batch: (a) re-key the child's FK to point at parent 2
// instead, then (b) delete parent 1. Since the plan must see the STAGED
// update (not the stale committed `parent_id = 1`), the child must NOT be
// cascade-deleted by parent 1's delete (it no longer references parent 1 by
// the time the plan runs) — it must survive, still pointing at parent 2.
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn transactional_update_then_delete_parent_cascade_uses_updated_reference() {
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
        "child_fk_cascade_restage",
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

    // Two parents; the child initially references parent 1.
    insert_helper(
        &resolver,
        "parent",
        doc().set("id", 1_i64).set("name", "P1"),
    )
    .await;
    insert_helper(
        &resolver,
        "parent",
        doc().set("id", 2_i64).set("name", "P2"),
    )
    .await;
    insert_helper(
        &resolver,
        "child",
        doc()
            .set("cid", 10_i64)
            .set("parent_id", 1_i64)
            .set("label", "c1"),
    )
    .await;

    // ONE transactional batch: re-key the child to reference parent 2, THEN
    // delete parent 1. The cascade plan for the parent-1 delete must use the
    // UPDATED (staged) reference — child now points at parent 2 — and so must
    // NOT cascade-delete the child.
    let mut b = Batch::new();
    b.id(3);
    b.transactional();
    b.try_update(
        "update_child_fk",
        write::update("child")
            .where_(filter::eq("parent_id", 1_i64))
            .set(doc().set("parent_id", 2_i64)),
    )
    .unwrap();
    b.try_delete(
        "del_parent_1",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let info = resp.transaction.expect("transaction info present");
    assert!(
        info.is_committed(),
        "transactional [re-key child; delete parent 1] must commit, got: {info:?}"
    );

    // Parent 1 gone, parent 2 survives.
    assert_eq!(count_rows(&resolver, "parent").await, 1);
    // Child survives — it no longer referenced parent 1 by the time the
    // cascade plan ran (it was re-keyed to parent 2 earlier in the same tx).
    assert_eq!(
        count_rows(&resolver, "child").await,
        1,
        "child re-keyed to parent 2 earlier in the SAME tx must survive \
         parent 1's cascade-delete — the plan must reflect the UPDATED \
         reference, not the stale committed one"
    );
    let val = read_first_field(&resolver, "child", "parent_id").await;
    assert_eq!(
        val,
        Some(QueryValue::Int(2)),
        "child's parent_id must reflect the staged update (now 2), not the \
         stale committed value (1)"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// F-28 Step 2, point 4 — staged-overlay fix for the RESTRICT index fast-path.
//
// A child row INSERTED (staged, not yet committed) referencing the parent,
// with an index covering the child FK field: `lookup_by_index` returns empty
// (indexing happens only at commit), so the RESTRICT gate's index fast-path
// must fall back to probing `tx.write_set` directly (mirroring
// `ValidatorDb::exists_in_table`'s staged-overlay pattern) rather than
// concluding "no reference" from the empty index alone.
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn transactional_insert_child_with_index_then_delete_parent_restrict_sees_staged_insert() {
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
        "child_fk_restrict_idx",
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

    // Parent exists (committed).
    insert_helper(&resolver, "parent", doc().set("id", 1_i64).set("name", "P")).await;

    // Build an index covering the child's FK field (`parent_id`) so the
    // RESTRICT gate's `child_has_reference` takes the index fast-path.
    let child_table = resolver.db.get_table("default", "child").await.unwrap();
    child_table
        .create_index("idx_parent_id", &["parent_id"])
        .await
        .expect("index creation should succeed");

    // ONE transactional batch: insert a child referencing the parent (staged,
    // not yet committed — never in the index), THEN try to delete the
    // parent. The RESTRICT gate must see the STAGED child via the
    // write_set overlay probe (the index alone returns empty) and reject.
    let mut b = Batch::new();
    b.id(2);
    b.transactional();
    b.insert(
        "ins_child",
        write::insert("child").row(doc().set("parent_id", 1_i64).set("label", "x")),
    );
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;

    // A transactional batch's mid-batch rejection surfaces as `Ok(response)`
    // with an ABORTED `TransactionInfo` (not an `Err`) — the batch executor
    // wraps every op error inside the tx into the abort reason. Mirrors how
    // `execute_batch_transactional_si_happy_path` reads `response.transaction`.
    match resp {
        Ok(r) => {
            let info = r
                .transaction
                .as_ref()
                .expect("transaction info present for a transactional batch");
            assert!(
                !info.is_committed(),
                "expected the tx to abort with fk_restrict (staged child insert \
                 should still block the parent delete via the index fast-path's \
                 staged-overlay fallback), but it committed: {info:?}"
            );
            let reason = info.reason.as_deref().unwrap_or("");
            assert!(
                reason.contains("fk_restrict"),
                "expected abort reason to contain 'fk_restrict', got: {reason}"
            );
        }
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("fk_restrict"),
                "expected fk_restrict — staged (uncommitted, unindexed) child \
                 insert must still be visible to the RESTRICT gate via the \
                 write_set overlay probe, got: {msg}"
            );
        }
    }
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

// ═══════════════════════════════════════════════════════════════════════════════
// F-53c — index fast-path vs scan-fallback equivalence for CASCADE / SET NULL.
//
// `fk_actions.rs::index_candidate_ids` (the F-53c index fast-path) and the
// `list_stream_tx` scan fallback MUST produce byte-for-byte identical results:
// the same rows deleted (CASCADE) or nulled (SET NULL), the same survivors,
// whether or not a supporting single-field index exists on the child FK
// column. These tests pin that equivalence by running the SAME scenario twice
// — once with NO index (scan path) and once WITH a supporting index (fast
// path) — and asserting identical final state. A third test pins the
// "no staged writes" correctness gate: with an index present AND a staged
// child insert in the SAME tx, the fast-path must NOT engage (the index is
// committed-only and would miss the staged row); the tx-aware scan fallback
// runs instead and correctly cascades the staged child too.
// ═══════════════════════════════════════════════════════════════════════════════

/// Build the shared CASCADE scenario: parent rows {1,2,3}; child rows where
/// four reference parent id=1 (the cascade target) and one each reference
/// id=2 / id=3 (must survive). When `with_index` is true a single-field
/// index is created on `child.parent_id` so the F-53c fast-path engages (the
/// fresh autocommit tx has no staged child writes).
async fn build_cascade_index_scenario(with_index: bool) -> FkTestResolver {
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
        "child_fk_cascade_idx",
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

    for pid in [1_i64, 2, 3] {
        insert_helper(
            &resolver,
            "parent",
            doc().set("id", pid).set("name", format!("p{pid}")),
        )
        .await;
    }
    let mut cid = 100_i64;
    for _ in 0..4 {
        insert_helper(
            &resolver,
            "child",
            doc().set("cid", cid).set("parent_id", 1_i64),
        )
        .await;
        cid += 1;
    }
    insert_helper(
        &resolver,
        "child",
        doc().set("cid", cid).set("parent_id", 2_i64),
    )
    .await;
    cid += 1;
    insert_helper(
        &resolver,
        "child",
        doc().set("cid", cid).set("parent_id", 3_i64),
    )
    .await;

    if with_index {
        let child_table = resolver.db.get_table("default", "child").await.unwrap();
        child_table
            .create_index("idx_child_parent_id", &["parent_id"])
            .await
            .expect("index creation");
    }

    resolver
}

/// Assert the post-cascade invariant for `build_cascade_index_scenario`:
/// parent 1 + its 4 children gone; parents 2,3 survive with one child each.
async fn assert_cascade_scenario_result(resolver: &FkTestResolver) {
    assert_eq!(count_rows(resolver, "parent").await, 2);
    assert_eq!(count_rows(resolver, "child").await, 2);
    let mut surviving: Vec<i64> = read_all_field(resolver, "child", "parent_id")
        .await
        .into_iter()
        .filter_map(|v| match v {
            QueryValue::Int(i) => Some(i),
            _ => None,
        })
        .collect();
    surviving.sort_unstable();
    assert_eq!(
        surviving,
        [2, 3],
        "only the children referencing parents 2 and 3 should survive"
    );
}

#[tokio::test]
async fn cascade_no_index_scan_path_deletes_referencing_children() {
    // Scan-fallback path (no supporting index).
    let resolver = build_cascade_index_scenario(false).await;
    assert_eq!(count_rows(&resolver, "child").await, 6);

    let mut b = Batch::new();
    b.id(10);
    b.try_delete(
        "del",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_cascade_scenario_result(&resolver).await;
}

#[tokio::test]
async fn cascade_with_index_fast_path_deletes_same_children() {
    // F-53c index fast-path (supporting index, fresh autocommit tx → no
    // staged child writes → fast-path engages). Must produce the IDENTICAL
    // result to the no-index scan path above.
    let resolver = build_cascade_index_scenario(true).await;
    assert_eq!(count_rows(&resolver, "child").await, 6);

    let mut b = Batch::new();
    b.id(10);
    b.try_delete(
        "del",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    assert_cascade_scenario_result(&resolver).await;
}

#[tokio::test]
async fn cascade_with_index_fast_path_falls_back_to_scan_for_staged_child_writes() {
    // With a supporting index BUT a staged child insert in the SAME tx, the
    // F-53c "no staged writes" gate MUST route to the tx-aware scan fallback
    // — otherwise the staged (unindexed) child would be orphaned by the
    // cascade (the index reflects committed state only). This is the CASCADE
    // analog of
    // `transactional_insert_child_with_index_then_delete_parent_restrict_sees_staged_insert`.
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
        "child_fk_cascade_staged",
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

    insert_helper(&resolver, "parent", doc().set("id", 1_i64).set("name", "P")).await;
    // One COMMITTED child referencing parent 1.
    insert_helper(
        &resolver,
        "child",
        doc().set("cid", 10_i64).set("parent_id", 1_i64),
    )
    .await;

    // Supporting index → the fast-path WOULD engage, except the same-tx
    // staged insert below forces the scan fallback.
    let child_table = resolver.db.get_table("default", "child").await.unwrap();
    child_table
        .create_index("idx_child_parent_id", &["parent_id"])
        .await
        .unwrap();

    // ONE transactional batch: insert a SECOND child referencing parent 1
    // (staged, never in the index until commit), THEN delete parent 1. The
    // cascade must see BOTH children (the committed one + the staged one via
    // the scan overlay) and cascade-delete both.
    let mut b = Batch::new();
    b.id(20);
    b.transactional();
    b.insert(
        "ins_child2",
        write::insert("child").row(doc().set("cid", 11_i64).set("parent_id", 1_i64)),
    );
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let info = resp
        .transaction
        .as_ref()
        .expect("transaction info present for a transactional batch");
    assert!(
        info.is_committed(),
        "cascade tx should commit (Cascade, not Restrict); got: {info:?}"
    );

    assert_eq!(count_rows(&resolver, "parent").await, 0);
    assert_eq!(
        count_rows(&resolver, "child").await,
        0,
        "the staged (unindexed) child insert must be cascaded too — the \
         F-53c 'no staged writes' gate must route to the tx-aware scan \
         fallback rather than trusting the committed-only index"
    );
}

#[tokio::test]
async fn set_null_with_index_fast_path_nulls_same_children() {
    // SET NULL via the F-53c index fast-path must match the existing
    // scan-path test `set_null_nulls_child_field_when_parent_deleted`
    // (which runs with no index). Two children reference parent 1; after
    // the delete both must survive with parent_id == Null.
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
        "child_fk_setnull_idx",
        "parent_id",
        "parent",
        "id",
        FkAction::SetNull,
        true,
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
            .set("parent_id", 1_i64)
            .set("label", "c1"),
    )
    .await;
    insert_helper(
        &resolver,
        "child",
        doc()
            .set("cid", 11_i64)
            .set("parent_id", 1_i64)
            .set("label", "c2"),
    )
    .await;

    // Supporting index on the FK column → F-53c fast-path engages.
    let child_table = resolver.db.get_table("default", "child").await.unwrap();
    child_table
        .create_index("idx_child_parent_id", &["parent_id"])
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(30);
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Parent gone; both children survive with parent_id == Null (same as the
    // scan-path SET NULL test).
    assert_eq!(count_rows(&resolver, "parent").await, 0);
    assert_eq!(count_rows(&resolver, "child").await, 2);
    let nulled = read_all_field(&resolver, "child", "parent_id").await;
    assert!(
        nulled.iter().all(|v| *v == QueryValue::Null),
        "both children's parent_id must be Null after SET NULL, got: {nulled:?}"
    );
}
