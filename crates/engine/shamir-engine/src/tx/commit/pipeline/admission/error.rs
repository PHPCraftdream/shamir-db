//! Typed errors of the commit admission gate (design section 2, settled).

/// Admission / close failures. All are retryable in the design sense: they
/// carry no side effect and the caller may re-attempt later. No variant is
/// produced after any durable state change (the gate refuses BEFORE the
/// leader touches the base interner, a version or WAL; the follower refuses
/// before the receipt pipeline's write-domain admission).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CommitAdmissionError {
    /// The gate is CLOSED and the caller was refused: a leader that waited
    /// past the reopen-wait cap (`ADMISSION_REOPEN_WAIT_CAP`) or a follower
    /// apply (Q13: immediate refusal). Nothing was mutated.
    #[error("commit admission is closed; retry after the capture barrier reopens")]
    Closed,
    /// The bounded ACTIVE count reached `ACTIVE_MAX` (`i32::MAX`), far below
    /// the 62-bit ceiling. Unreachable in practice; fails closed.
    #[error("commit admission lease count overflow")]
    Overflow,
    /// A close was attempted while another close owner still holds the
    /// CLOSE_TAKEN right. Single-owner by construction. Test-gated with the
    /// close API until the seal step consumes it.
    #[cfg(test)]
    #[error("a commit admission close is already in progress")]
    Busy,
}
