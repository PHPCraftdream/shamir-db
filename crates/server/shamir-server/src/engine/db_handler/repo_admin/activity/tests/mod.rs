//! Unit tests for the `ListActivity` serving path (R4b-1).
//!
//! `continuation_token_tests` — pure wire-format tests: round trip,
//! bounded opaque output, strict validation of every forged/garbled form,
//! and the chain-nonce and section-order invariants.
//! `paging_tests` — the production keyset selection helper
//! (`list::select_window`) with duplicate ids, exact limits and
//! continuation boundaries. No server harness — these exercise the same
//! code the paging worker calls.
//! `refusal_fairness_tests` — one principal cannot occupy the whole
//! refusal-audit queue while another principal is still recorded.
//! `refusal_close_race_tests` — a refusal that reserved a slot before
//! `close()` is never spawned after the discard sweep.
//! `tx_release_tests` — every end of an interactive transaction (owner rollback, reaper, session close, an in-flight request at session close) releases the Level-3 record locks it held; the release is idempotent and keyed by the transaction's own id.

mod cancellation_tests;
mod continuation_token_tests;
mod paging_tests;
mod refusal_close_race_tests;
mod refusal_fairness_tests;
mod tx_release_tests;
