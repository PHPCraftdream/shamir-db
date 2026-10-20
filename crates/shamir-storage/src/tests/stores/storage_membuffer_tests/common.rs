//! Shared helpers for the `storage_membuffer_tests` split.

use crate::storage_membuffer::MemBufferConfig;
use crate::types::RecordKey;

/// Config with a long flush interval. `@fl` review nit (task #530): this
/// alone does NOT stop the background flusher from firing — every
/// `set`/`remove` calls `notify.notify_one()`, and the flusher's `select!`
/// wakes on that notification regardless of `flush_interval_ms`. The `!
/// is_dirty_empty()` assertions below only hold because these tests run on
/// the current-thread `#[tokio::test]` runtime, where every `.await` in the
/// test body resolves `Ready` without ever yielding to the spawned flusher
/// task — so the flusher provably never gets scheduled during the test. This
/// is intentionally test-only groundedness, not a runtime-flavor-independent
/// guarantee; do not port these tests to a multi-thread runtime without
/// re-deriving why the flusher still can't preempt them.
pub(crate) fn no_flush_config() -> MemBufferConfig {
    MemBufferConfig {
        max_bytes: 64 * 1024 * 1024,
        max_entries: 1_000_000,
        ttl_ms: None,
        flush_interval_ms: 600_000,
        flush_batch_size: 256,
    }
}

pub(crate) fn rk(b: &[u8]) -> RecordKey {
    RecordKey::from_slice(b)
}
