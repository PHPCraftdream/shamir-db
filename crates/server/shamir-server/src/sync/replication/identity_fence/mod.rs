//! S5c-X PR7 — the universal leader-identity fence.
//!
//! EVERY subscription records the leader's `(lineage, history_epoch)` at its
//! first successful Hello/Pull and refuses (typed, fail-closed, terminal,
//! visible in the subscription status) when a later Hello or Pull reports a
//! different lineage or epoch — regardless of whether the subscription was
//! created with a seed receipt. This is the prerequisite of S5c-X PR8
//! (`completed_reseed`), which bumps the history epoch after an unclean
//! restart and relies on this fence to stop followers.
//!
//! - [`recorded`] — the durable fence record (same value as the
//!   bookmark, one atomic `set`+`flush`).
//! - [`checks`] — the per-response verification + first-contact adoption.
//!
//! Each loop start reflushes an existing record BEFORE contacting the leader:
//! a readable set from a failed/cancelled adoption does not prove durability.
//! Adoption/observation persistence errors terminate that loop, so no retry
//! inside it can bypass the flush. A restarted loop repeats the startup barrier;
//! matching Pulls within a prepared loop incur no extra flush. This assumes
//! serialized subscription ownership of the bookmark, as its writer requires.
//! On every Pull identity is decided before leader-epoch regression, so a
//! combined violation records the observed incarnation and returns its typed
//! identity refusal; unchanged identity retains the stale-leader refusal.

pub(crate) mod checks;
pub(crate) mod recorded;

#[cfg(test)]
mod tests;
