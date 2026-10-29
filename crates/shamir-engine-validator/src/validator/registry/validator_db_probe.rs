use async_trait::async_trait;
use shamir_query_types::TableRef;
use shamir_storage::error::DbResult;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

/// Tx-scoped read-only relational probes for declarative FK / unique rules.
#[async_trait]
pub trait ValidatorDbProbe: Send + Sync {
    async fn exists_in(&self, table: &TableRef, field: &str, value: &QueryValue) -> DbResult<bool>;
    async fn exists_in_self(
        &self,
        field: &str,
        value: &QueryValue,
        exclude_rid: Option<&RecordId>,
    ) -> DbResult<bool>;
}
