//! Wire-level application DTOs that ride inside the post-handshake
//! `RequestEnvelope.req` / `ResponseEnvelope.res` payload — i.e. the
//! "what does the client say to the database?" layer, distinct from
//! the SCRAM handshake (in `shamir-connect`) and the in-batch query
//! ops (in `crate::batch`).
//!
//! Lives here so that both the server (`shamir-server::db_handler`)
//! and the client SDK (`shamir-client`) reference the same definitions
//! without one depending on the other.

pub mod db_message;
pub mod repl;

#[cfg(test)]
mod tests;

pub use db_message::{DbRequest, DbResponse, CURRENT_QUERY_LANG_VERSION};
pub use repl::{ReplRepoInfo, ReplRequest, ReplResponse};
