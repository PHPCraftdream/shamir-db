//! Phase C1 — no-deadlock proof for `ValidatorDb`.
//!
//! These tests prove that a validator reading database state through
//! `ctx.db()` during an active write transaction does **not** deadlock.
//! The write path runs validators **before** staging/locking, so the
//! validator's reads on `tx.snapshot_version` never contend with the
//! tx's own (not-yet-acquired) write locks.
//!
//! Two scenarios are covered:
//! - **FK (cross-table)**: validator on table A reads table B via
//!   `exists_in` — independent table, independent lock space.
//! - **Unique (self-table)**: validator on table A reads table A via
//!   `exists_in_self` — same table, committed-only read path (no
//!   `acquire_pessimistic_read_lock`), so no self-deadlock.

use std::sync::Arc;

use async_trait::async_trait;

use shamir_storage::error::DbResult;
use shamir_tx::IsolationLevel;
use shamir_types::types::common::new_map;
use shamir_types::types::value::QueryValue;

use crate::query::TableRef;
use crate::query::TableResolver;
use crate::repo::repo_types::BoxRepoFactory;
use crate::repo::{RepoConfig, RepoInstance};
use crate::table::{TableConfig, TableManager};
use crate::validator::{
    encode::Validation, record_fields::RecordFields, record_validator::RecordValidator,
    record_validator::ValidatorCtx, ValidatorBinding, ValidatorRegistry, WriteOp,
};

// ── Test TableResolver ─────────────────────────────────────────────────────

/// Resolver backed by a single `RepoInstance` — resolves any table in it.
/// Injects the validator registry into every resolved table (mirrors
/// `DbTableResolver` in production).
struct TestResolver {
    repo: RepoInstance,
    validators: Arc<ValidatorRegistry>,
}

#[async_trait::async_trait]
impl TableResolver for TestResolver {
    async fn resolve(&self, table_ref: &TableRef) -> DbResult<TableManager> {
        let mut table = self.repo.get_table(&table_ref.table).await?;
        table.set_validator_registry(Arc::clone(&self.validators));
        Ok(table)
    }

    async fn resolve_repo(&self, _repo_name: &str) -> DbResult<RepoInstance> {
        Ok(self.repo.clone())
    }
}

// ── Async validator that reads ctx.db() ────────────────────────────────────

/// A validator that exercises both `exists_in` (cross-table FK) and
/// `exists_in_self` (self-table unique) inside `validate()`.
///
/// Uses `tokio::time::timeout` internally so a deadlock surfaces as a
/// clean test failure rather than a hung nextest process.
struct DbReadingValidator {
    /// The FK target table (cross-table read).
    fk_table: String,
    /// The field to probe on the FK table.
    fk_field: String,
    /// The field to probe on the self table (unique check).
    unique_field: String,
}

#[async_trait]
impl RecordValidator for DbReadingValidator {
    async fn validate(
        &self,
        new: Option<&dyn RecordFields>,
        _old: Option<&dyn RecordFields>,
        ctx: &ValidatorCtx<'_>,
    ) -> Validation {
        let Some(db) = ctx.db() else {
            // No DB handle — accept (fail-open).
            return Validation::accept();
        };
        let Some(record) = new else {
            return Validation::accept();
        };

        // Extract the FK field value from the record being written.
        let fk_value = match record.scalar(&[&self.unique_field]) {
            Some(scalar) => scalar_to_query_value(&scalar),
            None => return Validation::accept(),
        };

        // Wrap in a timeout so a deadlock is caught cleanly.
        let probe = async {
            // 1. Cross-table FK read: check if the value exists in the
            //    referenced table. This reads a DIFFERENT table on the tx's
            //    snapshot — no write locks held on it.
            let fk_ref = TableRef::new(&self.fk_table);
            let _fk_exists = db
                .exists_in(&fk_ref, &self.fk_field, &fk_value)
                .await
                .unwrap_or(false);

            // 2. Self-table unique read: check if the value exists in the
            //    same table being written. This reads committed state only
            //    (no pessimistic lock on write-path keys).
            let _unique_exists = db
                .exists_in_self(&self.unique_field, &fk_value, None)
                .await
                .unwrap_or(false);

            Ok::<(), shamir_storage::error::DbError>(())
        };

        match tokio::time::timeout(std::time::Duration::from_secs(5), probe).await {
            Ok(Ok(())) => Validation::accept(),
            Ok(Err(_)) => Validation::reject("db_read_error"),
            Err(_) => {
                // Timeout = deadlock.
                let mut v = Validation::reject("deadlock_timeout");
                v.stop();
                v
            }
        }
    }
}

/// Convert a `ScalarRef` to a comparable `QueryValue`.
fn scalar_to_query_value(scalar: &shamir_types::record_view::ScalarRef<'_>) -> QueryValue {
    use shamir_types::record_view::ScalarRef;
    match scalar {
        ScalarRef::Null => QueryValue::Null,
        ScalarRef::Bool(b) => QueryValue::Bool(*b),
        ScalarRef::Int(i) => QueryValue::Int(*i),
        ScalarRef::F64(f) => QueryValue::F64(*f),
        ScalarRef::Str(s) => QueryValue::Str((*s).to_string()),
        ScalarRef::Bin(b) => QueryValue::Bin(b.to_vec()),
    }
}

// ── Test setup ─────────────────────────────────────────────────────────────

/// Build a repo with two tables (`child` and `parent`), a `DbReadingValidator`
/// bound to `child`, and return the resolver + table handles.
async fn setup() -> (TestResolver, TableManager) {
    let repo_config = RepoConfig {
        name: "default".to_string(),
        factory: BoxRepoFactory::in_memory(),
        tables: vec![TableConfig::new("parent"), TableConfig::new("child")],
    };
    let repo = RepoInstance::from_factory(
        repo_config.name.clone(),
        repo_config.factory,
        repo_config.tables,
    )
    .await
    .unwrap();

    // Register the DbReadingValidator on the `child` table.
    let validator = Arc::new(DbReadingValidator {
        fk_table: "parent".to_string(),
        fk_field: "ref_id".to_string(),
        unique_field: "ref_id".to_string(),
    }) as Arc<dyn RecordValidator>;

    let reg = Arc::new(ValidatorRegistry::new());
    let validator_id = shamir_types::types::record_id::RecordId::system("db_reading_val");
    reg.register(validator_id, "db_reading_val", validator)
        .unwrap();

    // Persist bindings on the `child` table's info store so the validator
    // binding is loaded on every `get_table("child")` call.
    let child_tmp = repo.get_table("child").await.unwrap();
    let bindings = vec![ValidatorBinding {
        validator_id,
        ops: smallvec::smallvec![WriteOp::Insert],
        priority: 1000,
    }];
    // Access the info_store through the table's public API.
    // The persistence layer writes to the info_store the TableManager was
    // built with — but since get_table returns a fresh instance each time
    // (OnceCell), we need to persist BEFORE the OnceCell is initialized.
    // Use the helper from the persistence module which writes raw metadata.
    crate::validator::persistence::save_validators_metadata(&bindings, child_tmp.info_store())
        .await
        .unwrap();

    let resolver = TestResolver {
        repo,
        validators: reg,
    };
    // Resolve child through the resolver so it picks up the registry.
    let child = resolver.resolve(&TableRef::new("child")).await.unwrap();
    (resolver, child)
}

// ── Tests ──────────────────────────────────────────────────────────────────

/// Insert into `child` while a validator reads both `parent` (FK) and
/// `child` (unique) through `ctx.db()`. The write must complete without
/// deadlocking. nextest's per-test timeout (default 60s) is the backstop.
#[tokio::test]
async fn validator_db_no_deadlock_cross_and_self_read() {
    let (resolver, child) = setup().await;

    // Pre-populate the parent table with a referenced row.
    let mut parent_row = new_map();
    parent_row.insert("ref_id".to_string(), QueryValue::Int(42));
    parent_row.insert("name".to_string(), QueryValue::Str("alpha".into()));

    let parent_op = crate::query::write::InsertOp {
        insert_into: TableRef::new("parent"),
        values: vec![QueryValue::Map(parent_row)],
        records_idmsgpack: Vec::new(),
        select: None,
    };

    let (mut parent_tx, _guard) = resolver
        .repo
        .begin_tx(IsolationLevel::Snapshot)
        .await
        .unwrap();
    parent_tx.set_implicit(true);
    let parent = resolver.resolve(&TableRef::new("parent")).await.unwrap();
    parent
        .execute_insert_tx(
            &parent_op,
            &mut parent_tx,
            false,
            None,
            &shamir_types::access::Actor::System,
        )
        .await
        .unwrap();
    resolver.repo.commit_tx(parent_tx).await.unwrap();

    // Now insert into child — the DbReadingValidator will fire and read
    // both parent (FK) and child (unique) through ctx.db().
    let mut child_row = new_map();
    child_row.insert("ref_id".to_string(), QueryValue::Int(42));

    let child_op = crate::query::write::InsertOp {
        insert_into: TableRef::new("child"),
        values: vec![QueryValue::Map(child_row)],
        records_idmsgpack: Vec::new(),
        select: None,
    };

    let (mut child_tx, _child_guard) = resolver
        .repo
        .begin_tx(IsolationLevel::Snapshot)
        .await
        .unwrap();
    child_tx.set_implicit(true);

    // This is the critical call: it runs the validator, which reads via
    // ctx.db(). If there's a re-entrancy deadlock, this future never
    // completes and nextest kills it after the timeout.
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        child.execute_insert_tx(
            &child_op,
            &mut child_tx,
            false,
            None,
            &shamir_types::access::Actor::System,
        ),
    )
    .await;

    // The write must complete (Ok or Err — but NOT timeout).
    assert!(
        result.is_ok(),
        "execute_insert_tx DEADLOCKED (timed out after 10s) — validator DB read re-entered the write pipeline"
    );
    let inner = result.unwrap();
    assert!(
        inner.is_ok(),
        "execute_insert_tx failed (non-deadlock): {:?}",
        inner.err()
    );

    // Clean up.
    resolver.repo.commit_tx(child_tx).await.unwrap();
}
