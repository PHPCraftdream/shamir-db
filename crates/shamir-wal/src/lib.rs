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
//! WAL is a lightweight intent journal: before starting a batch
//! operation we write a marker listing the affected `record_id`s;
//! after successful completion we remove the marker. On next open:
//!
//! - No markers → everything is clean, normal operation.
//! - Marker present → there was a crash. Recovery checks
//!   consistency point-by-point and rolls forward or back.
//!
//! # Extensibility
//!
//! The design supports future extensions:
//!
//! - **Explicit transactions** — user calls `begin()`, N operations,
//!   `commit()`. The marker is open between begin and commit; on
//!   crash at any point recovery rolls back all planned changes.
//! - **Full-text search** — FTS index operations are no different from
//!   regular `IndexEntry` operations.
//! - **Schema migrations** — `WalOp::CreateIndex`, `DropIndex` etc.
//!
//! # Storage layout
//!
//! WAL lives in the same `info_store` under the fixed prefix
//! `b"__wal_active_"`. One marker = one KV record:
//!
//! ```text
//! key   = b"__wal_active_" || txn_id (8 bytes BE)         = 21 bytes
//! value = bincode(WalEntry { txn_id, started_at_ns, ops }) — contains
//!         the full set of intended operations for this transaction
//! ```
//!
//! The marker is written with a single `info_store.set(...)` before
//! the batch starts and removed with a single `info_store.remove(...)`
//! after. On backends with buffered durability (sled, redb with
//! `Durability::None`) both writes go through the buffer — the actual
//! fsync is amortised in the background. Performance overhead on the
//! happy path is close to zero.
//!
//! # Recovery scope
//!
//! One marker describes one batch operation (or one explicit
//! transaction). Recovery runs in O(operations_per_marker), not
//! in O(table_size).

pub mod active_key;
pub mod wal_entry;
pub mod wal_entry_v2;
pub mod wal_manager;

pub use active_key::WalActiveKey;
pub use wal_entry::{WalEntry, WalOp};
pub use wal_entry_v2::{WalEntryV2, WalOpV2, WAL_V2_MAGIC, WAL_V2_VERSION};
pub use wal_manager::WalManager;
