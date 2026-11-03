//! Bounded, strict MessagePack wire contract for HTTP login.

mod begin_request;
mod finish_request;
mod reader;
mod response;
mod scope;

#[cfg(test)]
mod tests;

pub use begin_request::BeginRequest;
pub use finish_request::FinishRequest;
pub use reader::WireError;
pub use response::{challenge, error, response, success, Challenge, Success};
pub use scope::{TokenScope, TokenScopeChannels, TokenScopeMode};
pub use shamir_transport_http::LoginResponse;
