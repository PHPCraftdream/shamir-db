//! Versioned KV layer over the history version log.
//!
//! [`MvccStore`] wraps a single [`Store`] instance:
//! - `history` — the sole version log. Every write appends a version-key
//!   entry `<key>::0xFF::<version_be>` (see [`version_codec`]). All reads
//!   resolve from this log.
//!
//! Every write (set/delete/batch/apply_committed) performs exactly ONE
//! durable append to `history`. MVCC-2 cannot occur because `publish_cell`
//! fires BEFORE the log write, so any snapshot opened mid-write sees the
//! bumped cell version and correctly range-scans the log for the prior entry.
//!
//! Snapshot reads via [`MvccStore::get_at`]: if the cached version ≤ snapshot,
//! read the log at `key‖0xFF‖version`; otherwise range-scan the log for the
//! newest version ≤ snapshot (see [`MvccStore::resolve_read`]).
mod drain;
mod history;
pub mod key_lock;
pub mod mvcc_locks;
mod store;
pub mod write_receipt;

pub use history::{mvcc_gc, mvcc_history, retention, version_entry};
pub use key_lock::{KeyLock, LockMode};
pub use retention::Retention;
pub use store::MvccStore;
pub use version_entry::VersionEntry;

#[cfg(test)]
pub(crate) use store::decode_ts_key;
pub(crate) use store::ts_key;
use store::RecordCell;
