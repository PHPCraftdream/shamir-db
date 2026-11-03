//! HTTP-login resource admission, protocol services and route adapter.

pub mod admission;
mod metrics;
pub mod pending;
pub mod routes;
pub mod service;
pub mod wire;

pub(crate) use metrics::set_ineligible_users;
pub use routes::HttpLoginRoutes;
