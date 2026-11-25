//! Read visibility and result-size accounting interfaces.

use std::time::Duration;

use shamir_storage::error::DbError;
use shamir_tx::CancelHandle;

use crate::query::read::QueryRecord;

/// Shared read-visibility budget of an enclosing execution.
pub trait ReadVisibilityScope: Send + Sync {
    fn read_visibility_budget(&self) -> Duration;

    fn cancel_handle(&self) -> Option<&CancelHandle> {
        None
    }
}

/// Policy-enabled per-read byte meter (result-size accounting).
pub trait ReadByteMeter: Send + Sync {
    fn charge(&self, bytes: u64) -> Result<(), DbError>;
    fn charge_record(&self, record: &QueryRecord) -> Result<(), DbError>;
    fn credit_survivors(&self, records: &[QueryRecord]);

    /// Whether any result-size estimate should be computed for this read.
    fn byte_policy_enabled(&self) -> bool {
        true
    }

    /// Record rows examined by a read, coalesced at existing scan batch boundaries.
    fn viewed(&self, _rows: u64) {}

    /// Record final result rows once a read has completed successfully.
    fn finish_rows(&self, _returned: u64) {}
}

/// Query operation categories with fixed exporter labels and indexes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueryOp {
    Read,
    Write,
    Batch,
    Tx,
    Function,
}

impl QueryOp {
    /// Stable lowercase label for the operation category.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Batch => "batch",
            Self::Tx => "tx",
            Self::Function => "function",
        }
    }

    /// Position in [`QUERY_OPS`] and the associated fixed-size metric arrays.
    pub const fn index(self) -> usize {
        match self {
            Self::Read => 0,
            Self::Write => 1,
            Self::Batch => 2,
            Self::Tx => 3,
            Self::Function => 4,
        }
    }
}

/// All query operation categories in stable index order.
pub const QUERY_OPS: [QueryOp; 5] = [
    QueryOp::Read,
    QueryOp::Write,
    QueryOp::Batch,
    QueryOp::Tx,
    QueryOp::Function,
];

/// Low-level, engine-independent contract for query telemetry.
pub trait QueryMetrics: Send + Sync {
    fn record_duration(&self, op: QueryOp, elapsed_ns: u64);
    fn record_error(&self, code: Option<&str>);
    fn record_rows(&self, viewed: u64, returned: u64);

    /// Record one data-op execution under its canonical shape key.
    ///
    /// The default is a no-op for sinks that do not keep shapes.
    fn record_shape(&self, _key: [u8; 16], _elapsed_ns: u64, _failed: bool) {}
}
