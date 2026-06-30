//! Cache-line padding to suppress **false sharing**.

use std::ops::{Deref, DerefMut};

/// Aligns and pads `T` to a cache line so that two `CachePadded` values placed
/// adjacently in memory never share a cache line.
///
/// # Why
///
/// **False sharing** is the MESI/MOESI ping-pong that happens when two cores
/// write independent values that happen to land on the same cache line: each
/// write invalidates the other core's copy, forcing a coherence round-trip
/// even though the data is logically disjoint (Herlihy & Shavit, *The Art of
/// Multiprocessor Programming* §8; Drepper §6.4). On a NUMA box those
/// round-trips cross the inter-socket interconnect — the most expensive coherence
/// traffic there is. [`NodeReplicated`](crate::NodeReplicated) wraps each
/// per-node `ArcSwap` cell in a `CachePadded` so a write on node 0 never
/// disturbs node 1's line.
///
/// # Why 128 bytes, not 64
///
/// The alignment is 128, not the 64-byte x86-64 line size, for two reasons:
/// x86-64's "adjacent cache line prefetcher" pulls the sibling line in a
/// 128-byte-aligned pair, and Apple Silicon uses 128-byte cache lines. 128 is
/// therefore the portable false-sharing-free unit (the same choice
/// `crossbeam_utils::CachePadded` makes on these targets).
#[derive(Clone, Copy, Default)]
#[repr(align(128))]
pub struct CachePadded<T>(pub T);

impl<T> CachePadded<T> {
    /// Wrap `value`, aligning it to a cache line.
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// Unwrap, discarding the padding.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> Deref for CachePadded<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T> DerefMut for CachePadded<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}
