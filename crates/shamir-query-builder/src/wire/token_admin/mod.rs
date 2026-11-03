//! Narrow builders for top-level API-token administration requests.
//!
//! This is a second deliberate exception to the crate's no-`DbRequest` rule:
//! without these helpers callers must hand-assemble request literals, contrary
//! to the query-builder-only rule. HMAC tags are optional because only the
//! server knows whether a particular target requires confirmation.

#![warn(missing_docs)]

mod create_api_token;
mod list_api_tokens;
mod revoke_api_token;
mod revoke_user_tokens;

pub use create_api_token::{create_api_token, CreateApiToken};
pub use list_api_tokens::{list_api_tokens, ListApiTokens};
pub use revoke_api_token::{revoke_api_token, RevokeApiToken};
pub use revoke_user_tokens::{revoke_user_tokens, RevokeUserTokens};

#[cfg(test)]
#[path = "tests/mod.rs"]
mod tests;
