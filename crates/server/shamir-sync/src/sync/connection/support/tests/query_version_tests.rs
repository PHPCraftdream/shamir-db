//! Query-version gate tests.
//!
//! HONEST LIMIT (E): these tests exercise the `checked_query_version` gate
//! that the request routes call. The production call sites themselves (the
//! per-route handlers in `connection`) run the gate inline; the private
//! handshake/run helpers are not referenceable from here, so a revert that
//! re-routes a call site around `checked_query_version` is NOT directly
//! discriminated by this file — the gate function's own contract is.

use crate::connection::checked_query_version;

#[test]
fn rv_server_9b8fc4f8_wire_query_version_rejects_u8_overflow() {
    assert_eq!(checked_query_version(255), Ok(255));
    assert!(checked_query_version(256).is_err());
}
