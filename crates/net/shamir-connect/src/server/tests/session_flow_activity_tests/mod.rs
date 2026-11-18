//! R4b-1 tests: minted non-secret session handle + O(1) activity mirror.
//! Lives beside (not inside) `session_flow`, which is at the 7-entry folder
//! limit; `mod.rs` re-exports only.

mod session_handle_tests;
