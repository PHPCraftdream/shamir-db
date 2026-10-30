//! Write-Ahead Log primitives for ShamirDB.
//!
//! Extracted from `shamir-engine/src/wal/` to its own crate so the
//! upcoming MVCC layer (`shamir-tx`) can depend on WAL without
//! pulling the whole engine.
//!
//! # Architecture
//!
//! Each Store backend (sled, redb, persy, …) guarantees durability
//! and atomicity of its own writes. But between `data_store` and
//! `info_store` there is no built-in transactional link — a partial
//! crash can leave data without indexes (or vice versa).
//!
//! The WAL bridges that gap as an append-only file-segment journal
//! (the F5c/F6 cutover retired the earlier KV-marker design that
//! stored intent records under `b"__wal_active_"` in `info_store`;
//! production no longer uses such markers). Each committed tx is one
//! [`WalEntryV2`] appended to a [`SegmentSet`] of [`WalSegment`]s via
//! [`WalGroupCommit`]. Appends land in the OS page cache (level 2,
//! surviving a process crash) and are promoted to level 3 (fsync) on
//! the truncation/sync gate. On next open, [`SegmentSet::recover`]
//! walks every sealed + active segment in order and replays the
//! entries that have not yet been truncated.
//!
//! # Filesystem trust boundary
//!
//! WAL checksums detect accidental corruption; CRC does not authenticate
//! writers. Anyone able to write the WAL can inject transactions that recovery
//! replays. TLS and volume encryption do not authenticate WAL contents against
//! local writers.
//!
//! The data directory, any sibling WAL directory, WAL segments and sidecars,
//! and restored copies must be writable only by the database identity and
//! explicitly trusted operators or tooling. Protect parent directories against
//! unauthorized rename or replacement of these paths. On Unix, use `0700` for
//! directories, `0600` for files, and `umask 0077`; grant exceptions only to
//! trusted operators or tooling using appropriately restricted permissions.
//! On Windows, restrict ACLs to the database identity and explicitly trusted
//! operators or tooling, including on parents and restored copies; remove or
//! restrict inherited broad write, delete, and delete-child permissions.
//!
//! # Truncation model
//!
//! Entries live in the segments until the background drainer, gated
//! by `interner_delta_safe_to_truncate` (the A5 interner-hwm gate),
//! confirms the entry's data is durably materialised in history AND
//! its interner delta is durably checkpointed. Only then does it call
//! `wal.commit(txn_id)` to advance the truncation watermark. There
//! are no per-entry markers to remove on the happy path — truncation
//! is a watermark advance over the segment set.
//!
//! # Recovery scope
//!
//! One entry describes one transaction (its full op set + interner
//! delta). Recovery replays entries in `commit_version` order and
//! runs in O(operations_per_entry), not in O(table_size). Replayed
//! ops are idempotent at the data layer, so a re-replay after a
//! mid-recovery failure converges.

#[cfg(test)]
mod tests;

pub mod event_header;

mod entry;
mod segment;
pub mod segment_set;
pub mod wal_group_commit;

// Crate-internal namespace (RV-WAL-0843B96B): every item in the sidecar
// metadata module is `pub(crate)`; it is not public API.
pub(crate) use entry::segment_meta;
pub use entry::wal_entry_v2;
pub use segment::wal_segment;
pub use segment::wal_sink;
pub use segment::wal_ticket;

pub use event_header::{WalEventHeader, WalExtraChange};
pub use segment_set::SegmentSet;
pub use wal_entry_v2::{WalEntryV2, WalOpV2, WAL_V2_MAGIC, WAL_V2_VERSION};
pub use wal_group_commit::{WalDurability, WalGroupCommit};
pub use wal_segment::WalSegment;
pub use wal_sink::WalSink;
pub use wal_ticket::{WalAppendError, WalTicket, WalTicketState};
