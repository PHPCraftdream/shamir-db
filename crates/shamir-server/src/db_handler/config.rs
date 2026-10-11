use shamir_query_types::batch::{BatchLimits, ExecPolicy};

/// Per-batch slow-query threshold (in microseconds, matching
/// `BatchResponse::execution_time_us`). `0` disables the warning.
/// Set on the handler at boot from `[logging] slow_query_threshold_ms`.
#[derive(Debug, Clone, Copy)]
pub struct SlowQueryConfig {
    pub threshold_us: u64,
}

impl SlowQueryConfig {
    pub const DISABLED: Self = Self { threshold_us: 0 };
    pub fn from_ms(ms: u64) -> Self {
        Self {
            threshold_us: ms.saturating_mul(1_000),
        }
    }
}

/// Server-side hard caps on `BatchRequest.limits`. Applied as a max:
/// the engine clamps the client's payload values DOWN to these caps.
///
/// Set on the handler at boot from `[security.query_limits]`. Tests that
/// don't care about resource limits use [`Self::UNLIMITED`].
#[derive(Debug, Clone, Copy)]
pub struct QueryLimitsCap {
    pub enforce_result_size: bool,
    pub max_result_size_bytes: usize,
    pub max_execution_time_secs: u64,
    pub max_queries_per_batch: usize,
    pub max_nesting_depth: usize,
    pub max_dependency_depth: usize,
    pub max_iterations: usize,
    pub max_total_queries: usize,
    pub max_total_work: u64,
    pub max_page_size: u64,
    pub max_offset: u64,
}

impl QueryLimitsCap {
    /// No operator caps; hard engine ceilings still apply.
    pub const UNLIMITED: Self = Self {
        enforce_result_size: false,
        max_result_size_bytes: usize::MAX,
        max_execution_time_secs: u64::MAX,
        max_queries_per_batch: usize::MAX,
        max_nesting_depth: usize::MAX,
        max_dependency_depth: usize::MAX,
        max_iterations: usize::MAX,
        max_total_queries: usize::MAX,
        max_total_work: u64::MAX,
        max_page_size: u64::MAX,
        max_offset: u64::MAX,
    };

    pub fn policy(&self) -> ExecPolicy {
        ExecPolicy {
            caps: BatchLimits {
                max_result_size: self.max_result_size_bytes,
                max_execution_time_secs: self.max_execution_time_secs,
                max_queries: self.max_queries_per_batch,
                max_nesting_depth: self.max_nesting_depth,
                max_dependency_depth: self.max_dependency_depth,
                max_iterations: self.max_iterations,
                max_total_queries: self.max_total_queries,
                max_total_work: self.max_total_work,
                max_page_size: self.max_page_size,
                max_offset: self.max_offset,
            },
            enforce_result_size: self.enforce_result_size,
        }
    }
}

impl From<&crate::config::QueryLimitsConfig> for QueryLimitsCap {
    fn from(config: &crate::config::QueryLimitsConfig) -> Self {
        Self {
            enforce_result_size: config.enforce_result_size,
            max_result_size_bytes: config.max_result_size_bytes,
            max_execution_time_secs: config.max_execution_time_secs,
            max_queries_per_batch: config.max_queries_per_batch,
            max_nesting_depth: config.max_nesting_depth,
            max_dependency_depth: config.max_dependency_depth,
            max_iterations: config.max_iterations,
            max_total_queries: config.max_total_queries,
            max_total_work: config.max_total_work,
            max_page_size: config.max_page_size,
            max_offset: config.max_offset,
        }
    }
}

/// Read/write mode of this node. A replica follower runs `ReadOnly` and
/// rejects client writes (they must go to the leader). Default `ReadWrite`.
///
/// The gate lives in [`ShamirDbHandler::execute`](super::handler::ShamirDbHandler::execute):
/// when `ReadOnly`, any batch entry whose [`BatchOp`](shamir_query_types::batch::BatchOp)
/// returns `true` from `is_write()` is rejected with `code = "read_only_replica"`
/// before reaching the engine. Reads (SELECT, introspection) pass through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NodeMode {
    #[default]
    ReadWrite,
    ReadOnly,
}

/// Server-side hard cap on per-interactive-tx staged bytes. Checked on
/// each `TxExecute`; over-budget aborts the tx with `tx_too_large`.
/// Default 64 MiB; tests use [`Self::UNLIMITED`].
#[derive(Debug, Clone, Copy)]
pub struct TxLimitsCap {
    pub max_tx_bytes: usize,
}

impl TxLimitsCap {
    pub const UNLIMITED: Self = Self {
        max_tx_bytes: usize::MAX,
    };
}

/// FG-5b — server-side hard caps on result cursors.
///
/// `max_cursors_per_session` bounds how many cursors ONE session may have
/// open concurrently (each pins an MVCC snapshot, so an unbounded count
/// would let a single client block GC indefinitely). `idle_timeout_secs`
/// bounds how long a cursor may sit un-fetched before the background
/// reaper reclaims it. `max_cursor_page_size` (CR-A3) bounds the
/// `page_size` a `CreateCursor`/`FetchNext` request may ask for — rejecting
/// `page_size == 0` (an infinite-loop hazard — see
/// `crate::db_handler::cursor_handlers`) and anything above the cap.
/// Default 16 cursors / 60 s idle / 10,000 page_size; tests use
/// [`Self::UNLIMITED`].
#[derive(Debug, Clone, Copy)]
pub struct CursorLimitsCap {
    pub max_cursors_per_session: usize,
    pub idle_timeout_secs: u64,
    pub max_cursor_page_size: u32,
}

impl CursorLimitsCap {
    /// Effectively-no-cap defaults — for unit tests.
    pub const UNLIMITED: Self = Self {
        max_cursors_per_session: usize::MAX,
        idle_timeout_secs: u64::MAX,
        max_cursor_page_size: u32::MAX,
    };

    /// Operator-facing defaults: 16 cursors/session, 60 s idle TTL, 10,000
    /// max page_size. See `crate::cursor_registry::DEFAULT_CURSOR_IDLE_TTL`
    /// for why 60 s (longer than the interactive-tx idle TTL — cursor fetch
    /// cadence is client-paced, not a single round-trip).
    pub const DEFAULT: Self = Self {
        max_cursors_per_session: 16,
        idle_timeout_secs: 60,
        max_cursor_page_size: 10_000,
    };
}
