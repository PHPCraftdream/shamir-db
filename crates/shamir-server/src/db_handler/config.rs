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
/// the client's payload values are clamped DOWN to these caps before
/// the batch is dispatched into `ShamirDb::execute`.
///
/// Set on the handler at boot from `[security.query_limits]`. Tests that
/// don't care about resource limits use [`Self::UNLIMITED`].
#[derive(Debug, Clone, Copy)]
pub struct QueryLimitsCap {
    pub max_result_size_bytes: usize,
    pub max_execution_time_secs: u64,
    pub max_queries_per_batch: usize,
}

impl QueryLimitsCap {
    /// Effectively-no-cap defaults — for unit tests. Matches `BatchLimits::default()`.
    pub const UNLIMITED: Self = Self {
        max_result_size_bytes: usize::MAX,
        max_execution_time_secs: u64::MAX,
        max_queries_per_batch: usize::MAX,
    };
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
