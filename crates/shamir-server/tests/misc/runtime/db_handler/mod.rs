//! Integration tests for the `RequestHandler` ↔ `ShamirDb` bridge.
//!
//! These tests prove that the wire-side `Execute { db, batch }` path
//! preserves the full feature set of the underlying `BatchRequest`/
//! `BatchResponse` API: multi-record reads, projections, ordering,
//! pagination, multi-query batches with `$query` references, admin DDL,
//! and the superuser permission gate.
//!
//! # Migration note
//!
//! All batch requests (DML + DDL) are constructed with
//! `shamir_query_builder` and round-tripped through MessagePack.

mod acl_tx;
mod common;
mod gating;
mod protocol;
mod reads;
mod scram;
