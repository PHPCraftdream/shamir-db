//! History version-log modules: the durable version log (`mvcc_history`),
//! garbage collection (`mvcc_gc`), the retention policy (`retention`), and
//! per-version entry / streaming types (`version_entry`).

mod history_many;
pub mod mvcc_gc;
pub mod mvcc_history;
pub mod retention;
pub mod version_entry;
