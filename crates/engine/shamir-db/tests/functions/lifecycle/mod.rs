//! Function lifecycle end-to-end tests (split from the former
//! `functions_lifecycle.rs` binary).

mod basic;
mod context_and_calls;
mod db_gateway;
mod http_egress;
mod secrets_meta;
mod support;
pub(crate) use support::compile_or_skip;
