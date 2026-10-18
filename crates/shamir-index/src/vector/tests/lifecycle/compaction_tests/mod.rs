//! V4.2 (#408) — HNSW compaction tests, split by topic:
//! `basic` (unit / integration / stress) and `quantized` (#428 VR-6 +
//! backfill delete race).

mod basic;
mod quantized;

pub(super) use basic::{random_vec, rid};
