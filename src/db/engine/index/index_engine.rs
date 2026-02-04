//! Index engine - in-memory index with state flags and LRU eviction
//!
//! This module provides the core data structures for managing indexes in memory.
//! Each index entry has a state flag (ACTUAL, UPDATE, SAVING) and usage tracking.

use std::sync::atomic::AtomicU8;
use std::sync::Arc;

use crate::types::record_id::RecordId;
use crate::types::value::UserValue;

/// State of an index entry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryState {
    /// Entry matches disk (can be evicted)
    Actual = 0,
    /// Entry was modified, needs to be saved (never evict)
    Update = 1,
    /// Entry is being saved to disk (never evict)
    Saving = 2,
}

impl EntryState {
    pub fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Actual),
            1 => Some(Self::Update),
            2 => Some(Self::Saving),
            _ => None,
        }
    }

    pub fn as_u8(&self) -> u8 {
        *self as u8
    }

    /// Can this entry be evicted?
    pub fn can_evict(&self) -> bool {
        matches!(self, Self::Actual)
    }
}

/// Tracks usage metadata for an index entry
#[derive(Debug, Clone)]
pub struct IndexUsageTracker {
    state: Arc<AtomicU8>,
}

impl IndexUsageTracker {
    pub fn new() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(EntryState::Actual.as_u8())),
        }
    }

    /// Get current state
    pub fn state(&self) -> EntryState {
        EntryState::from_u8(self.state.load(std::sync::atomic::Ordering::Acquire))
            .unwrap_or(EntryState::Actual)
    }

    /// Set state
    pub fn set_state(&self, state: EntryState) {
        self.state.store(state.as_u8(), std::sync::atomic::Ordering::Release);
    }

    /// Check if can be evicted
    pub fn can_evict(&self) -> bool {
        self.state().can_evict()
    }
}

/// Single entry in an index
#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub record_id: RecordId,
    pub value: UserValue,
    pub tracker: IndexUsageTracker,
}

impl IndexEntry {
    pub fn new(record_id: RecordId, value: UserValue) -> Self {
        Self {
            record_id,
            value,
            tracker: IndexUsageTracker::new(),
        }
    }

    /// Check if this entry can be evicted
    pub fn can_evict(&self) -> bool {
        self.tracker.can_evict()
    }

    /// Get current state
    pub fn state(&self) -> EntryState {
        self.tracker.state()
    }

    /// Set state
    pub fn set_state(&self, state: EntryState) {
        self.tracker.set_state(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_state_roundtrip() {
        assert_eq!(EntryState::Actual.as_u8(), 0);
        assert_eq!(EntryState::Update.as_u8(), 1);
        assert_eq!(EntryState::Saving.as_u8(), 2);

        assert_eq!(EntryState::from_u8(0), Some(EntryState::Actual));
        assert_eq!(EntryState::from_u8(1), Some(EntryState::Update));
        assert_eq!(EntryState::from_u8(2), Some(EntryState::Saving));
        assert_eq!(EntryState::from_u8(3), None);
    }

    #[test]
    fn test_entry_state_can_evict() {
        assert!(EntryState::Actual.can_evict());
        assert!(!EntryState::Update.can_evict());
        assert!(!EntryState::Saving.can_evict());
    }

    #[test]
    fn test_index_usage_tracker_state() {
        let tracker = IndexUsageTracker::new();
        assert_eq!(tracker.state(), EntryState::Actual);
        assert!(tracker.can_evict());

        tracker.set_state(EntryState::Update);
        assert_eq!(tracker.state(), EntryState::Update);
        assert!(!tracker.can_evict());
    }
}
