//! Transaction isolation level (builder-side).

/// Transaction isolation level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    /// Reads see a consistent snapshot; writes use last-writer-wins.
    Snapshot,
    /// Read-set validated at commit; concurrent write conflict aborts.
    Serializable,
    /// Pessimistic (level-3) isolation: locks guard concurrent writers.
    /// (Exposed in query-language v4; previously wire-only as
    /// `"level3"`/`"pessimistic"`.)
    Pessimistic,
}
