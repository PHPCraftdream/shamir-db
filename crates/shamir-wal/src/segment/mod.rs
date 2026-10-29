//! Segment files, sinks and append tickets.

pub(crate) mod frame_reader;
mod mem_frames;
pub mod wal_segment;
pub mod wal_sink;
pub mod wal_ticket;
