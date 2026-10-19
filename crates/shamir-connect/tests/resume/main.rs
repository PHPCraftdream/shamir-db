//! Integration tests for resumption flow (SESSION_RESUMPTION §5).
//!
//! Split by topic: `resume_flow` (ticket acceptance/rejection flows),
//! `authz_rotation` (authorization, rotation overlap, wire compat), with
//! shared builders in `helpers`.

mod authz_rotation;
mod helpers;
mod resume_flow;
