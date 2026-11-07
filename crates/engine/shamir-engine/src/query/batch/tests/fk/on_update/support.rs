use std::sync::Arc;

use shamir_query_builder::batch::Batch;
use shamir_query_builder::write;
use shamir_query_types::admin::FkAction;
use shamir_types::access::Actor;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;
use smallvec::smallvec;

use crate::db_instance::db_instance::DbInstance;
use crate::query::batch::execute_batch_unchecked as execute_batch;
use crate::query::batch::TableResolver;
use crate::query::TableRef;
use crate::table::TableManager;
use crate::validator::schema::constraints::Constraints;
use crate::validator::schema::field_rule::FieldRule;
use crate::validator::schema::foreign_key::ForeignKeyRef;
use crate::validator::schema::schema_validator::SchemaValidator;
use crate::validator::schema::type_tag::TypeTag;
use crate::validator::{ValidatorBinding, ValidatorRegistry, WriteOp};

// ── Test resolver (mirrors fk_actions_tests / fk_restrict_tests) ─────────────

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

/// Bind a `SchemaValidator` with a single FK field to a child table.
///
/// `validator_id` MUST be unique across the parallel test suite to avoid the
/// shared-registry collision flake (the registry is per-test here, but the
/// convention keeps the intent explicit and future-proofs shared registries).
#[allow(clippy::too_many_arguments)]
pub(super) fn bind_fk_validator(
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
        keyset_safe: false,
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
pub(super) async fn read_field_all(
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
pub(super) async fn insert_row(
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
