//! In-memory index store with LRU eviction and state tracking
//!
//! Uses DashMap for lock-free reads and concurrent writes.
//! Based on the same patterns as Interner (TDashMap, FxHasher).

use crate::types::common::{new_dash_map_wc, TDashMap};
use crate::types::record_id::RecordId;
use crate::types::value::UserValue;
use crate::db::storage::types::{Store, RecordKey};
use crate::db::error::{DbError, DbResult};
use std::sync::atomic::{AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use bytes::Bytes;
use async_trait::async_trait;
use futures::stream;
use std::pin::Pin;

/// State of an index entry
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EntryState {
    /// Entry is up-to-date (in memory matches disk)
    Actual = 0,
    /// Entry was modified by Table, needs to be saved
    Update = 1,
    /// Indexer is currently saving this entry to disk
    Saving = 2,
}