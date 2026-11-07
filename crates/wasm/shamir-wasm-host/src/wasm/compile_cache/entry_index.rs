use scc::HashMap;
use shamir_collections::THasher;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::Arc;

/// Shared tick handle: `Arc` lets `get` clone the handle, drop the map entry
/// guard, and bump recency afterwards without ever touching a removed entry.
#[derive(Debug)]
pub(crate) struct EntryMeta {
    pub(crate) len: u64,
    pub(crate) tick: Arc<AtomicU64>,
}

#[derive(Debug)]
pub(crate) struct EntryIndex {
    pub(crate) entries: HashMap<[u8; 32], EntryMeta, THasher>,
    pub(crate) bytes: AtomicU64,
    pub(crate) count: AtomicUsize,
}

impl EntryIndex {
    pub(crate) fn new() -> Self {
        Self {
            entries: HashMap::with_hasher(THasher::default()),
            bytes: AtomicU64::new(0),
            count: AtomicUsize::new(0),
        }
    }
}
