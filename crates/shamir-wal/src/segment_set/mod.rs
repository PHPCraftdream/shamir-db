//! Segmented WAL sink — see [`SegmentSet`].

mod sealed_retained_bytes;
#[path = "segment_set.rs"]
mod set;
mod sync_snapshot;
#[cfg(test)]
pub(crate) mod sync_test_seam;

pub use set::SegmentSet;
