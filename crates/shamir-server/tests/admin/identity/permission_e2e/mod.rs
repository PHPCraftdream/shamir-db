//! End-to-end permission enforcement tests (Shomer access fabric).
//!
//! Exercises the full wire path: SCRAM-authenticated session →
//! `ShamirDbHandler::execute` → `ShamirDb::execute_as` → `authorize_access`.
//!
//! Scenarios:
//!   1. **deny/allow by mode** — admin creates table, chmod 0o700 (owner-only);
//!      non-admin user is DENIED; admin is ALLOWED.
//!   2. **group grant** — admin creates group, adds user, chgrp + chmod group-read;
//!      user is now ALLOWED via group; non-member is DENIED.
//!   3. **open default** — new resources default to 0o777; any authenticated
//!      user is ALLOWED.

mod coarse_gate;
mod common;
mod modes;
mod nested_batch;
