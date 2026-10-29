//! HMAC-gate integration tests.
//!
//! Every destructive admin op (drop_db, drop_repo, drop_table,
//! drop_index, drop_user, group changes, function/validator drops and
//! replication drops) MUST carry a hex-encoded HMAC-SHA256 tag over the
//! canonical bytes for that op, keyed by
//! `SHA256("shamir-db hmac key v1\0" || session_id)`. Conditional function
//! and validator replacements are gated as well, as are function creations
//! with definer security, replacement, or non-empty secret grants.
//!
//! These tests confirm the gate at the wire boundary:
//!   * missing tag → `hmac_required`
//!   * wrong tag   → `hmac_mismatch`
//!   * correct tag → op executes normally
//!
//! Non-destructive ops are untouched by the gate.

mod acl;
mod common;
mod drop_ops;
mod groups;
mod misc;
mod retention;
