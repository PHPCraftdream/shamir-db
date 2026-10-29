//! Read visibility and result-size accounting interfaces.

use std::time::Duration;

use shamir_storage::error::DbError;

use crate::query::read::QueryRecord;

/// Shared read-visibility budget of an enclosing execution.
pub trait ReadVisibilityScope: Send + Sync {
    fn read_visibility_budget(&self) -> Duration;
}

/// Policy-enabled per-read byte meter (result-size accounting).
pub trait ReadByteMeter: Send + Sync {
    fn charge(&self, bytes: u64) -> Result<(), DbError>;
    fn charge_record(&self, record: &QueryRecord) -> Result<(), DbError>;
    fn credit_survivors(&self, records: &[QueryRecord]);
}
