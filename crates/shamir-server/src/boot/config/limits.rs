//! Server-side hard caps: interactive-transaction bytes, result cursors and
//! per-batch query limits.

use serde::Deserialize;
use shamir_query_types::batch::BatchLimits;

/// Server-side hard cap on per-interactive-tx staged bytes.
///
/// Without this block the default 64 MiB cap applies. A malicious client
/// staging unbounded data inside a single interactive tx would grow the WAL
/// entry and in-memory staging until OOM — the cap aborts the tx with
/// `tx_too_large` before that happens.
#[derive(Debug, Clone, Deserialize)]
pub struct TxLimitsConfig {
    /// Maximum total bytes that an interactive tx may stage across all
    /// its `TxExecute` calls. Over-budget aborts the tx with
    /// `tx_too_large`. Default 64 MiB.
    #[serde(default = "default_max_tx_bytes")]
    pub max_tx_bytes: usize,
}

impl Default for TxLimitsConfig {
    fn default() -> Self {
        Self {
            max_tx_bytes: default_max_tx_bytes(),
        }
    }
}

fn default_max_tx_bytes() -> usize {
    64 * 1024 * 1024 // 64 MiB
}

/// FG-5b — server-side hard caps on result cursors.
///
/// Without this block, the defaults in
/// `crate::db_handler::config::CursorLimitsCap::DEFAULT` apply: 16 cursors
/// per session, 60 s idle-timeout eviction. Each open cursor pins an MVCC
/// snapshot (via `shamir_tx::SnapshotGuard`), so an unbounded per-session
/// count or an unbounded idle window would let one client block GC
/// indefinitely.
#[derive(Debug, Clone, Deserialize)]
pub struct CursorLimitsConfig {
    /// Maximum concurrently-open cursors per session. Default 16.
    #[serde(default = "default_max_cursors_per_session")]
    pub max_cursors_per_session: usize,
    /// Seconds a cursor may sit un-fetched before the background reaper
    /// reclaims it. Longer than the interactive-tx idle TTL (30 s) — a
    /// cursor's `FetchNext` cadence is paced by client consumption speed,
    /// not a single round-trip. Default 60.
    #[serde(default = "default_cursor_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
    /// CR-A3: maximum `page_size` a `CreateCursor`/`FetchNext` request may
    /// request. `page_size == 0` is always rejected (it makes `has_more`
    /// compute `0 >= 0 → true` forever, looping the client indefinitely);
    /// `page_size` above this cap is rejected outright (not silently
    /// clamped — a client that thinks it got `page_size` rows per page but
    /// silently got fewer would misinterpret `has_more` semantics). Default
    /// 10,000.
    #[serde(default = "default_max_cursor_page_size")]
    pub max_cursor_page_size: u32,
}

impl Default for CursorLimitsConfig {
    fn default() -> Self {
        Self {
            max_cursors_per_session: default_max_cursors_per_session(),
            idle_timeout_secs: default_cursor_idle_timeout_secs(),
            max_cursor_page_size: default_max_cursor_page_size(),
        }
    }
}

fn default_max_cursors_per_session() -> usize {
    16
}

fn default_cursor_idle_timeout_secs() -> u64 {
    60
}

fn default_max_cursor_page_size() -> u32 {
    10_000
}

/// Server-side hard caps on `BatchRequest.limits`.
///
/// Omitted fields use operator defaults; request limits may lower but never
/// raise these caps or the hard engine ceilings.
#[derive(Debug, Clone, Deserialize)]
pub struct QueryLimitsConfig {
    /// Enforce the cumulative bytes materialized by an execution, estimated
    /// conservatively and checked before commit. Includes intermediates even
    /// when not returned, and ForEach/nested containers; dropping or filtering
    /// results never gives bytes back. Not a serialized-response or RSS cap.
    /// Default true; false keeps reserve-only batch semantics.
    #[serde(default = "default_enforce_result_size")]
    pub enforce_result_size: bool,
    /// Maximum materialized-result budget when enforcement is enabled, and
    /// the RI-15 upfront reserve estimate passed as `BatchLimits::max_result_size`.
    /// Cursor pages retain this hard cap on serialized response bytes.
    /// Default 64 MiB.
    #[serde(default = "default_max_result_size_bytes")]
    pub max_result_size_bytes: usize,
    /// Maximum total execution time (seconds) — clamps
    /// `BatchLimits::max_execution_time_secs`. Default 60.
    #[serde(default = "default_max_execution_time_secs")]
    pub max_execution_time_secs: u64,
    /// Maximum number of queries per batch — clamps `BatchLimits::max_queries`.
    /// Default 100.
    #[serde(default = "default_max_queries_per_batch")]
    pub max_queries_per_batch: usize,
    /// Maximum sub-batch nesting depth. Zero disables nesting. Default 64.
    #[serde(default = "default_max_nesting_depth")]
    pub max_nesting_depth: usize,
    /// Maximum dependency chain depth. Zero allows independent queries only.
    #[serde(default = "default_max_dependency_depth")]
    pub max_dependency_depth: usize,
    /// Maximum loop iterations. Default 10,000.
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
    /// Maximum queries across the entire batch tree. Default 10,000.
    #[serde(default = "default_max_total_queries")]
    pub max_total_queries: usize,
    /// Maximum statically bounded nested/literal loop work. Default 1,000,000.
    /// Dynamic repetition is not charged at runtime (engine gap outside W4);
    /// this is not universal runtime aggregate accounting.
    #[serde(default = "default_max_total_work")]
    pub max_total_work: u64,
    /// Maximum requested query page size. Default 100,000.
    #[serde(default = "default_max_page_size")]
    pub max_page_size: u64,
    /// Maximum pagination offset. Zero disables skipping. Default 1,000,000.
    #[serde(default = "default_max_offset")]
    pub max_offset: u64,
    /// RI-15 — global cap (bytes) on the SUM of in-flight response memory
    /// across every concurrently-executing batch/connection. The
    /// per-batch `max_result_size_bytes` supplies the upfront estimate; without
    /// this field nothing bounds the SUM across every concurrent
    /// connection (`crates/shamir-server/src/byte_budget.rs`'s module doc:
    /// worst case ~64 GiB buffered at `max_active_connections = 1000` × a
    /// 64 MiB per-batch cap).
    ///
    /// F-29 (#822): the key being ABSENT resolves to a finite default,
    /// `4 * default_max_result_size_bytes()` (256 MiB) — matches the
    /// shipped `server.medium.example.ktav`/`server.small.example.ktav`
    /// profiles' own "4x max_result_size_bytes" convention, and is derived
    /// from (not hardcoded independent of) `default_max_result_size_bytes`
    /// so the two stay in lockstep if that default ever changes.
    /// `ByteBudget::acquire` never hard-errors on exhaustion — it waits
    /// (bounded, wakes on release) until room frees up — so this default
    /// change turns unbounded-memory-growth into bounded backpressure for
    /// any deployment that was relying on the old unbounded default,
    /// rather than introducing a new rejection/error path.
    ///
    /// An operator can still opt BACK into unbounded behavior by setting
    /// the key EXPLICITLY to `null` in their `.ktav` — serde's `Option<T>`
    /// deserialization honors an explicit null independently of
    /// `#[serde(default = ...)]` (the default fn only runs when the key is
    /// missing entirely), so this round-trips cleanly: absent → finite
    /// default, explicit `null` → `None`/unbounded. `server_launcher/launch.rs`
    /// logs a `tracing::warn!` at boot whenever the resolved value is
    /// `None`, since that is now an explicit escape hatch rather than "the
    /// default nobody thought about".
    ///
    /// When set (to any `Some(_)`, including the new default), MUST be
    /// `>= max_result_size_bytes` — otherwise no single max-size batch
    /// could ever be admitted; `Config::validate` rejects that
    /// configuration at startup. The derived default trivially satisfies
    /// this (it IS `4 * max_result_size_bytes`'s own default).
    #[serde(default = "default_max_inflight_response_bytes")]
    pub max_inflight_response_bytes: Option<usize>,
}

impl Default for QueryLimitsConfig {
    fn default() -> Self {
        Self {
            enforce_result_size: default_enforce_result_size(),
            max_result_size_bytes: default_max_result_size_bytes(),
            max_execution_time_secs: default_max_execution_time_secs(),
            max_queries_per_batch: default_max_queries_per_batch(),
            max_nesting_depth: default_max_nesting_depth(),
            max_dependency_depth: default_max_dependency_depth(),
            max_iterations: default_max_iterations(),
            max_total_queries: default_max_total_queries(),
            max_total_work: default_max_total_work(),
            max_page_size: default_max_page_size(),
            max_offset: default_max_offset(),
            max_inflight_response_bytes: default_max_inflight_response_bytes(),
        }
    }
}

fn default_enforce_result_size() -> bool {
    true
}

fn default_max_result_size_bytes() -> usize {
    64 * 1024 * 1024 // 64 MiB
}
fn default_max_execution_time_secs() -> u64 {
    60
}
fn default_max_queries_per_batch() -> usize {
    100
}

// Previously uncapped fields default to CEILING, not request defaults:
// clients may still raise their own limits within the hard engine bounds.
fn default_max_nesting_depth() -> usize {
    BatchLimits::CEILING.max_nesting_depth
}
fn default_max_dependency_depth() -> usize {
    BatchLimits::CEILING.max_dependency_depth
}
fn default_max_iterations() -> usize {
    BatchLimits::CEILING.max_iterations
}
fn default_max_total_queries() -> usize {
    BatchLimits::CEILING.max_total_queries
}
fn default_max_total_work() -> u64 {
    BatchLimits::CEILING.max_total_work
}
fn default_max_page_size() -> u64 {
    BatchLimits::CEILING.max_page_size
}
fn default_max_offset() -> u64 {
    BatchLimits::CEILING.max_offset
}

/// F-29 (#822): finite safe default for `max_inflight_response_bytes`,
/// derived from (not hardcoded independent of) `default_max_result_size_bytes`
/// so it automatically tracks that default — currently
/// `4 * 64 MiB = 256 MiB`, matching the shipped medium profile's own value.
fn default_max_inflight_response_bytes() -> Option<usize> {
    Some(default_max_result_size_bytes() * 4)
}
