use std::time::Duration;

/// Observed owner of the lowest unsettled version. Ownership transfers can
/// race a diagnostic read; absence of a known owner is not evidence of abort.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GateBlockerClass {
    ActiveTail,
    OrphanAwaitingReplay,
    UnresolvedTicket,
    PendingUnclassified,
}

/// The lowest unsettled version in a best-effort gate snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateBlocker {
    pub version: u64,
    pub class: GateBlockerClass,
    /// Lower bound since first diagnostic observation of this version as the
    /// oldest blocker, NOT time since allocation or WAL acceptance.
    pub age: Duration,
}

/// Read-only, best-effort repository progress telemetry, not a coherent cut.
/// Never use this snapshot to settle versions or make durability decisions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct GateDiagnostics {
    pub allocation_high_water: u64,
    pub visible_floor: u64,
    pub last_committed: u64,
    pub durable_watermark: u64,
    pub requires_reopen: bool,
    pub visibility_waiters: usize,
    /// W+1 when H>W; classification can race ownership transfer/settlement.
    pub oldest_blocker: Option<GateBlocker>,
    pub unsettled_versions: u64,
    /// Explicit Pending entries in the sparse completion tracker.
    pub pending_versions: u64,
    /// Unsettled versions without an explicit completion entry.
    pub missing_versions: u64,
    /// Default zero/None at the gate; the engine accessor may enrich these.
    pub orphan_replay_attempts_total: u64,
    pub orphan_replay_failures_total: u64,
    pub orphan_replay_consecutive_failures: u64,
    pub orphan_replay_last_error: Option<String>,
}
