//! Client-side registered-numeric field-map cache (Stage 5 minimal).
//!
//! The server interns field names to monotonic `u64` ids per repo. Ids are
//! append-only and NEVER reused or deleted (see
//! `shamir-types::core::interner`), so the client can mirror a repo's
//! `(name ↔ id)` mapping without invalidation logic — only fill-missing.
//!
//! This module owns two lock-free types:
//!
//! * [`FieldMap`] — one per `(db, repo)`. Two `scc::HashMap`s (forward +
//!   reverse) plus an `AtomicU64` epoch. A `tokio::sync::OnceCell<()>`
//!   guards the FIRST full dump so concurrent first-users don't stampede
//!   the server with N identical dump requests; later writers race the
//!   insert benignly (ids are stable).
//! * [`InternerCacheRegistry`] — `scc::HashMap<(db, repo), Arc<FieldMap>>`.
//!
//! Concurrency model: every access is lock-free (`scc` + atomics). The ONLY
//! sanctioned `tokio::sync` primitive is the `OnceCell` dump guard, because
//! it must hold across the `.await` on the server roundtrip — that is the
//! single justified async-mutex-style use in this module (CLAUDE.md pillar 1
//! exception: "Guard across `.await`, bounded contention → tokio::sync").

mod cache;
pub mod ops;

pub use cache::{FieldMap, InternerCacheRegistry};
