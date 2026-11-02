//! HTTP connection serving and private request/response machinery.

/// Listener accept loop and connection task lifecycle.
pub mod accept;
pub(crate) mod body;
pub(crate) mod response;
pub(crate) mod service;
