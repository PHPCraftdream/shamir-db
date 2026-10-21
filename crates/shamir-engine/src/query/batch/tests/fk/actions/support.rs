use std::sync::Arc;

use shamir_query_builder::batch::Batch;
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

// ── Test resolver (same as fk_restrict_tests) ────────────────────────────────

pub(super) struct FkTestResolver {
    pub(super) db: DbInstance,
    pub(super) repo: String,
    pub(super) registry: Arc<ValidatorRegistry>,
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
pub(super) fn bind_fk_validator(
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
pub(super) async fn count_rows(resolver: &FkTestResolver, table_name: &str) -> usize {
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
pub(super) async fn read_first_field(
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
// ── local helpers ────────────────────────────────────────────────────────────

/// Insert a single row (mirrors fk_on_update_tests::insert_row but local to
/// this file to avoid a cross-file dependency).
pub(super) async fn insert_helper(
    resolver: &FkTestResolver,
    table: &str,
    doc: impl Into<QueryValue>,
) {
    let mut b = Batch::new();
    b.id(0);
    b.insert("ins", write::insert(table).row(doc));
    let req = b.build();
    execute_batch(&req, resolver, None, None, Actor::System, "test")
        .await
        .unwrap();
}

/// Read a field value from every row of a table (mirrors fk_on_update_tests).
pub(super) async fn read_all_field(
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
pub(super) async fn build_cascade_index_scenario(with_index: bool) -> FkTestResolver {
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
pub(super) async fn assert_cascade_scenario_result(resolver: &FkTestResolver) {
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
