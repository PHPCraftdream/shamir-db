//! Server-side API-token administration request handling.

mod logic;
#[cfg(test)]
pub(in crate::engine::db_handler) use logic::finalize_created;
pub(crate) use logic::handle;
