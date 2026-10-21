//! Read planning strategies: index planning, temporal reads, keyset seeks.
//! This module is a manifest: declarations and re-exports only.

pub(in crate::table) mod read_asof_seek;
pub(in crate::table::read) mod read_index_scan;
pub(in crate::table::read) mod read_planner;
pub(in crate::table::read) mod read_temporal;
