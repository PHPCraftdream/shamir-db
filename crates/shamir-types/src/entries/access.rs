//! Shomer access-control primitive types.
//!
//! These types model *who* is acting ([`Actor`]), *what* they target
//! ([`ResourcePath`]), and *how* ([`Action`]). [`trace_access`] is the
//! engine-level OBSERVABILITY trace (always `Ok` — not an enforcement
//! gate). Real enforcement lives in [`permits`] (pure decision) and the
//! facade gate in `shamir-db` (meta resolution + ancestor traversal).
//!
//! The full object & operation hierarchy is specified in
//! `docs/dev-artifacts/roadmap/ACCESS_HIERARCHY.md`.

mod access_error;
mod action;
mod identity;
mod mode;
mod policy;
mod resource_meta;
mod resource_path;

pub use super::resource_path_ref::*;
pub use access_error::*;
pub use action::*;
pub use identity::*;
pub use mode::*;
pub use policy::*;
pub use resource_meta::*;
pub use resource_path::*;
