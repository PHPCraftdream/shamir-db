//! Unit tests for the `ListActivity` serving path (R4b-1).
//!
//! `continuation_token_tests` — pure wire-format tests: round trip,
//! bounded opaque output, strict validation of every forged/garbled form,
//! and the chain-nonce and section-order invariants.
//! `paging_tests` — the production keyset selection helper
//! (`list::select_window`) with duplicate ids, exact limits and
//! continuation boundaries. No server harness — these exercise the same
//! code the paging worker calls.

mod continuation_token_tests;
mod paging_tests;
