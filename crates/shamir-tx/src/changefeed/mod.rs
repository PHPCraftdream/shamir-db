//! Changefeed (Phase 3b) — hybrid live-push + durable journal.
//!
//! From the commit point a committed transaction's write footprint is
//! projected ONCE into a [`ChangelogEvent`] and fanned out down two
//! independent, non-blocking tracks:
//!
//! 1. **Live push** — a per-repo [`tokio::sync::broadcast`] channel. Each
//!    subscriber gets the event the instant it is published. A slow
//!    subscriber that falls behind the channel's bounded ring receives a
//!    `RecvError::Lagged(n)` and SKIPS those events — that is acceptable
//!    because it can re-read the missed window from the durable journal
//!    (track 2). Emission is `Sender::send`, which never waits for
//!    receivers; with zero subscribers it returns `Err` which the
//!    commit-path ignores.
//!
//! 2. **Durable journal** — a per-repo append log keyed by
//!    `commit_version` (big-endian 8 bytes, so lexicographic store order
//!    equals numeric order). The commit-path hands the event to a bounded
//!    [`tokio::sync::mpsc`] channel via `try_send` (NEVER blocks); a
//!    background task batches pending events and writes them durably to
//!    the changelog store. Engine pulls confirm a writer barrier and range-read
//!    only the settled version prefix.
//!
//! NEITHER track blocks the hot commit-path. The projection is done once
//! and reused for both tracks; a commit with an empty footprint emits
//! nothing.
//!
//! ## Durability trade-off (journal)
//!
//! Local commits and replicated applies enqueue (or mark a gap) immediately
//! before version publication, without an await or fallible step between them.
//! Engine journal pulls capture the contiguous completion watermark W (every
//! version through W is settled), then confirm the preceding channel prefix
//! with a writer barrier before reading events at or below the confirmed bound.
//! Confirmation has a bounded budget; timeout returns an older safe prefix.
//! A version still committing holds back the prefix rather than being skipped.
//!
//! Persistence is asynchronous: crash may lose the unflushed tail; cold start
//! rebuilds derivable events from surviving WAL. Missing/legacy entries,
//! closure, write failures or corruption mark gaps; channel overflow instead
//! defers the event to a repair-pending set (see `repair_pending`) for
//! re-enqueue by the engine drainer, and only declares a gap when the writer
//! is closed. With follower
//! provenance, unknown in-flight range makes unresolved (J, H] conservatively
//! gapped; H is the recovered gate floor. Failed shutdown alone adds no blanket
//! gap: WAL rebuilds through H. Gap metadata persists asynchronously; failed
//! marker writes stay dirty for retry. The journal is not the source of truth.
//!
//! Live broadcast occurs after publication in commit-completion order, not
//! necessarily version order. A consumer that discards versions at or below
//! its last delivered version can skip a late lower-version event. Use bounded
//! engine journal pulls for ordered history, and handle reported gaps.

pub mod cold;

mod feed;
mod install;
mod journal_read;
mod repair_pending;
#[cfg(test)]
mod tests;

pub use feed::events::{
    clean_horizon_key, decode_clean_horizon, decode_gap_marker, encode_clean_horizon,
    encode_gap_marker, gap_marker_key, nontx_event, project_event, version_key, ChangeOp,
    ChangelogEvent, RecordChange, BROADCAST_CAPACITY, JOURNAL_CHANNEL_CAPACITY,
};
pub use feed::{ChangelogError, ChangelogResult, ChangelogStore, RepoChangefeed};
pub use journal_read::JournalRead;
pub use repair_pending::RepairPending;

pub(crate) use feed::core::JournalMsg;
#[cfg(test)]
pub(crate) use feed::core::{drain_and_persist, journal_writer_loop, GapMarker};
