//! Integration tests for the administrative shell (R5 PR1–PR2).
//!
//! Declarations only: the harness owns shared fixtures, `output` covers the
//! output contract, and `transport`, `secure_transports`, and `refusals` cover
//! the live transport and refusal scenarios.

mod harness;
mod output;
// `secure/` holds the transport-policy refusal scenarios; `#[path]` keeps
// the Rust module tree (and every `super::` path inside the file) exactly
// as before the move. The secure-listener fixture moved to the same folder
// but stays anchored at `harness.rs`, which owned it before the move.
#[path = "secure/refusals.rs"]
mod refusals;
mod secure_transports;
// R5 PR3: the state and narrow data views (`status`, `version show`,
// `replication subscriptions`, `data count` / `head` / `get`). The topic
// folder is ONE entry; `#[path]` keeps every inner `super::` path anchored
// at this module, exactly like the `secure/` move. Shared helpers live in
// `state_data_common`; one primary item per sibling file.
#[path = "state_data/common.rs"]
mod state_data_common;
#[path = "state_data/goldens.rs"]
mod state_data_goldens;
#[path = "state_data/subscriptions.rs"]
mod state_data_subscriptions;
#[path = "state_data/token_subset.rs"]
mod state_data_token_subset;
mod transport;

use harness::secure_fixture;
