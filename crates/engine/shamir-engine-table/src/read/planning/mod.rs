//! Read planning strategies: index planning, temporal reads, keyset seeks.
//! This module is a manifest: declarations and re-exports only.

pub(crate) mod read_asof_seek;
pub(crate) mod read_index_scan;
pub(crate) mod read_planner;
pub(crate) mod read_temporal;
