//! Integration tests for the administrative shell (R5 PR1–PR2).
//!
//! Declarations only: the harness owns shared fixtures, `output` covers the
//! output contract, and `transport`, `secure_transports`, and `refusals` cover
//! the live transport and refusal scenarios.

mod harness;
mod output;
mod refusals;
mod secure_transports;
mod transport;

use harness::secure_fixture;
