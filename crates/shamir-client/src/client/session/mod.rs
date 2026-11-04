//! Client session assembly: the [`Client`] type, its connection entry
//! points, the connection option structs and the rid-demux mux (`mux/`).

mod client;
pub(in crate::client) mod entry;
mod entry_ws;
pub(crate) mod mux;
mod options;
pub mod token;

pub use client::Client;
pub use options::{
    bound_until, resolve_host, ConnectLocalOptions, ConnectOptions, ResumeOptions,
    SecureResumeOptions,
};
pub use token::{ApiToken, AuthKind, TokenHttpClient, TokenHttpOptions, TokenWsOptions};
