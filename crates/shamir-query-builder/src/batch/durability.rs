//! Per-request durability level (builder-side).

/// Per-request durability tier.
///
/// The former `AsyncIndex` variant moved to the orthogonal
/// [`IndexVisibility`] option (query-language v4) and is gone here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Durability {
    /// Ack after in-memory buffer; durable on background tick.
    Buffered,
    /// Flush durable backing before ack.
    Synced,
}
