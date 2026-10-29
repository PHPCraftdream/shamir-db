//! Commit gate and its supporting machinery: cell reservation, diagnostics,
//! index write operations, predicate sets, and the per-repo transaction gate.

pub mod cell_reservation_guard;
#[path = "repo_tx_gate/gate_diagnostics.rs"]
pub mod gate_diagnostics;
pub mod index_write_op;
pub mod predicate_set;
pub mod repo_tx_gate;

#[cfg(test)]
mod tests;
