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
use shamir_types::types::value::QueryValue;
use smallvec::smallvec;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::execute_batch_unchecked as execute_batch;
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
        keyset_safe: false,
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
    b.try_delete(
        "del_child",
        write::delete("child").where_(filter::eq("parent_id", 1)),
    )
    .unwrap();
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
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    )
    .unwrap();
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
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    )
    .unwrap();
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
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(
        resp.results.contains_key("del_parent"),
        "parent delete should succeed with no children"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Fix 1 (Finding 11) — Int↔F64 coercion in RESTRICT child matching.
//
// A float-typed child FK value referencing an int-typed parent key must
// correctly BLOCK the delete (previously the strict same-variant match made the
// child invisible to the restrict scan, so the delete wrongly succeeded).
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn restrict_blocks_delete_with_int_parent_f64_child_coercion() {
    let resolver = setup_fk_test(FkAction::Restrict).await;

    // Parent key stored as Int(1).
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_parent",
        write::insert("parent").row(doc().set("id", 1_i64).set("name", "Alice")),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Child FK field stored as F64(1.0) — cross-type reference.
    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins_child",
        write::insert("child").row(doc().set("parent_id", 1.0_f64).set("label", "x")),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Delete the parent → should be REJECTED by RESTRICT despite the type
    // mismatch (coercion must make the child visible to the restrict scan).
    let mut b = Batch::new();
    b.id(3);
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;

    match resp {
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("fk_restrict"),
                "error should contain 'fk_restrict' (coercion), got: {msg}"
            );
        }
        Ok(_) => panic!("Expected fk_restrict error — F64 child must coerce-match Int parent"),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// Fix 2 Site A — self-referential ON DELETE RESTRICT.
//
// `check_fk_restrict` / `discover_restrict_refs` is a flat, non-recursive scan,
// so self-referential RESTRICT is 100% safe: removing the self-ref skip means
// the table is scanned as its own potential child.
// ═══════════════════════════════════════════════════════════════════════════════

/// Build a test environment with a single self-referential table.
async fn setup_self_ref_fk_test(on_delete: FkAction) -> FkTestResolver {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("employees")],
    };
    let db = DbInstance::with_repos(vec![repo_config]).await.unwrap();

    let registry = Arc::new(ValidatorRegistry::new());

    // employees.manager_id → employees.id with the given on_delete action.
    let schema = SchemaValidator::new(vec![FieldRule {
        path: vec!["manager_id".to_string()],
        ty: TypeTag::Int,
        constraints: Constraints {
            foreign_key: Some(ForeignKeyRef::with_on_delete("employees", "id", on_delete)),
            required: false,
            nullable: true,
            ..Default::default()
        },
        keyset_safe: false,
    }]);

    let validator_id = RecordId::from_ts(9002);
    registry
        .register(validator_id, "self_ref_fk_schema", Arc::new(schema))
        .unwrap();

    let binding = ValidatorBinding {
        validator_id,
        ops: smallvec![WriteOp::Delete],
        priority: 1000,
    };

    let mut table = db.get_table("default", "employees").await.unwrap();
    table.set_validator_registry(Arc::clone(&registry));
    table.add_validator_binding(binding).await.unwrap();

    FkTestResolver {
        db,
        repo: "default".to_string(),
        registry,
    }
}

#[tokio::test]
async fn self_referential_restrict_blocks_delete_when_subordinate_exists() {
    let resolver = setup_self_ref_fk_test(FkAction::Restrict).await;

    // CEO (id=1, no manager).
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins_ceo",
        write::insert("employees").row(
            doc()
                .set("id", 1_i64)
                .set("name", "CEO")
                .set("manager_id", QueryValue::Null),
        ),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Subordinate references CEO.
    let mut b = Batch::new();
    b.id(2);
    b.insert(
        "ins_sub",
        write::insert("employees").row(
            doc()
                .set("id", 2_i64)
                .set("name", "Sub")
                .set("manager_id", 1_i64),
        ),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Delete CEO → must be REJECTED (subordinate still references CEO).
    let mut b = Batch::new();
    b.id(3);
    b.try_delete(
        "del_ceo",
        write::delete("employees").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test").await;

    match resp {
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("fk_restrict"),
                "self-ref restrict should block delete, got: {msg}"
            );
        }
        Ok(_) => panic!("Expected fk_restrict — self-ref restrict must fire"),
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// F-28 Step 2 (D1) — transactional [delete child; delete parent] under RESTRICT
// must SUCCEED: the RESTRICT gate must see the child's staged delete (this
// same tx), not the still-committed child row.
//
// Before F-28 Step 2, `check_fk_restrict`'s row probes read committed-only
// state (`TableManager::list_stream` / a committed-only index lookup), so a
// child row deleted EARLIER IN THE SAME transactional batch was still visible
// to the RESTRICT gate — wrongly rejecting the parent delete with
// `fk_restrict` even though, by commit time, the child would already be gone.
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn transactional_delete_child_then_parent_succeeds_under_restrict() {
    let resolver = setup_fk_test(FkAction::Restrict).await;

    // Insert parent + child (separate, committed, autocommit batches).
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
        write::insert("child").row(doc().set("parent_id", 1).set("label", "x")),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // ONE transactional batch: delete the child, THEN delete the parent.
    // The RESTRICT gate on the parent delete must see the child's OWN staged
    // delete (read-your-own-writes) and allow the parent delete to proceed.
    let mut b = Batch::new();
    b.id(3);
    b.transactional();
    b.try_delete(
        "del_child",
        write::delete("child").where_(filter::eq("parent_id", 1)),
    )
    .unwrap();
    b.try_delete(
        "del_parent",
        write::delete("parent").where_(filter::eq("id", 1)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let info = resp.transaction.expect("transaction info present");
    assert!(
        info.is_committed(),
        "transactional [delete child; delete parent] under RESTRICT must commit, got: {info:?}"
    );
    assert!(resp.results.contains_key("del_child"));
    assert!(resp.results.contains_key("del_parent"));
}

#[tokio::test]
async fn self_referential_restrict_allows_delete_when_no_subordinates() {
    let resolver = setup_self_ref_fk_test(FkAction::Restrict).await;

    // CEO (id=1) + a leaf employee (id=2, manager_id=null).
    let mut b = Batch::new();
    b.id(1);
    b.insert(
        "ins",
        write::insert("employees").row(
            doc()
                .set("id", 1_i64)
                .set("name", "CEO")
                .set("manager_id", QueryValue::Null),
        ),
    );
    let req = b.build();
    execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    // Delete the leaf employee (id=1, no one references them) → must succeed.
    let mut b = Batch::new();
    b.id(2);
    b.try_delete(
        "del",
        write::delete("employees").where_(filter::eq("id", 1_i64)),
    )
    .unwrap();
    let req = b.build();
    let resp = execute_batch(&req, &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(
        resp.results.contains_key("del"),
        "self-ref restrict should allow delete when no subordinates"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Defect 1 (fan-out collapse) — a deduped single-pass child scan must still
// catch a match on ANY of several distinct matched parent values, not just
// the first/last one iterated. `check_fk_restrict` used to call
// `child_has_reference` once PER (un-deduped) parent value, each doing its
// own full child-table scan; the fix collapses this to one scan per child
// table testing every row against the whole deduped value set via
// `any_child_references`. This test deletes THREE parent rows in one batch
// where only the MIDDLE one (by insertion order) still has a referencing
// child — a regression here would most plausibly manifest as only the
// first-or-last deduped value being checked.
// ═══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn restrict_blocks_on_middle_value_among_several_deduped_parents() {
    let resolver = setup_fk_test(FkAction::Restrict).await;

    // Three parent rows, none sharing a ref_field value.
    for (id, name) in [(10, "P10"), (20, "P20"), (30, "P30")] {
        let mut b = Batch::new();
        b.id(id);
        b.insert(
            "ins",
            write::insert("parent").row(doc().set("id", id).set("name", name)),
        );
        execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();
    }

    // TWO children both reference the MIDDLE parent value (20) — proves
    // multiple child rows referencing the same deduped value are also
    // correctly detected, not just the first one scanned. Parents 10 and 30
    // have no referencing children at all.
    for (cid, label) in [(200, "c200a"), (201, "c200b")] {
        let mut b = Batch::new();
        b.id(cid + 1000);
        b.insert(
            "ins",
            write::insert("child").row(doc().set("parent_id", 20).set("label", label)),
        );
        execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
            .await
            .unwrap();
    }

    // Delete all three parents in ONE batch op (matches all 3 distinct
    // ref_field values in a single scan) → must be rejected because of the
    // still-referenced middle value (20).
    let mut b = Batch::new();
    b.id(999);
    b.try_delete(
        "del_parents",
        write::delete("parent").where_(filter::in_("id", [10_i64, 20, 30])),
    )
    .unwrap();
    let resp = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test").await;

    match resp {
        Err(e) => {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("fk_restrict"),
                "expected fk_restrict error for the still-referenced middle \
                 value, got: {msg}"
            );
        }
        Ok(_) => panic!(
            "expected fk_restrict rejection (parent 20 still has 2 \
             referencing children) — got success, which means the deduped \
             single-pass scan silently missed a non-first/last value"
        ),
    }

    // Now delete BOTH children of parent 20, then retry the same 3-parent
    // delete → must succeed. This closes the loop: the fix neither
    // under-detects (previous assertion) NOR over-restricts (this one) once
    // every matched value is genuinely unreferenced.
    let mut b = Batch::new();
    b.id(1000);
    b.try_delete(
        "del_children",
        write::delete("child").where_(filter::eq("parent_id", 20_i64)),
    )
    .unwrap();
    execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();

    let mut b = Batch::new();
    b.id(1001);
    b.try_delete(
        "del_parents_retry",
        write::delete("parent").where_(filter::in_("id", [10_i64, 20, 30])),
    )
    .unwrap();
    let resp = execute_batch(&b.build(), &resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
    assert!(
        resp.results.contains_key("del_parents_retry"),
        "all three parents should delete cleanly once no children reference \
         any of them"
    );
}
