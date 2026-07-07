//! Bench helpers for the S.H.A.M.I.R. workspace.
//!
//! The workspace's benches run on `bench-scale-tool` (a fixed-iteration
//! harness — see `D:/dev/rust/bench-scale-tool`), not Criterion. This crate
//! now only carries the pieces that harness doesn't provide: shared test
//! fixture generation ([`vector_data`]) and optional peak-RSS sampling
//! ([`peak_mem`]).
//!
//! The former Criterion SMOKE/QUICK/FULL tier-tuning API (`tune`,
//! `tune_tiered`, `sample_size`, etc.) was removed once the last consumer
//! migrated off Criterion — bench-scale-tool's calibrate/run model replaces
//! it entirely (see `bench-iters.txt` at the workspace root).

#[cfg(feature = "peak_mem")]
pub mod peak_mem;

pub mod vector_data;
