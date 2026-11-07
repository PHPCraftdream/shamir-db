//! Rid-demux mux: lifecycle state, the background reader task and the
//! test-only hook seam (moved out of `client/transport.rs`).

pub(crate) mod hooks;
pub(crate) mod lifecycle;
pub(crate) mod reader;
