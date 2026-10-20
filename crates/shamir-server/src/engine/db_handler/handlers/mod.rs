//! Thin grouping layer for the db_handler request-family modules:
//! admin glue, replication (pull/repl) handling, subscribe handling and
//! visibility handling. Re-exports only; the public API is re-exported
//! from `crate::db_handler`.

pub(crate) mod admin;
pub(crate) mod repl_handler;
pub(crate) mod subscribe_handler;
pub(crate) mod visibility_handlers;
