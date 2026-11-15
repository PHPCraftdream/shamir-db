//! `shamir-metrics` — fixed-bucket atomic histogram with a torn-free seqlock
//! snapshot for S.H.A.M.I.R. (design `docs/dev-artifacts/design/engine-metrics.md` §3).
//!
//! Zero production dependencies beyond `std`: every engine layer can record
//! into it without a cycle. The engine keeps state, the server polls
//! snapshots (`docs/dev-artifacts/prompts/release/04-observability.md` §2) —
//! no metrics library, no locks on hot paths.

mod atomics;
mod bounds;
mod histogram;

#[cfg(test)]
mod tests;

pub use bounds::{BYTES_BOUNDS, LATENCY_NS_BOUNDS, MAX_BOUNDS};
pub use histogram::{Histogram, HistogramSnapshot};
