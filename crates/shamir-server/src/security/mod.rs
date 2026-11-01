//! Security and platform surface: access control, TLS, the user directory,
//! response byte budgeting, observability endpoints and OS service
//! management.

pub mod access_tree;
pub mod observability;
mod perimeter;
pub mod service;
pub mod user_directory;

pub use perimeter::{byte_budget, tls};
