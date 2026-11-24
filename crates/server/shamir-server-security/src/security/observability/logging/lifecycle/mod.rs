//! Lifecycle of a running observability server: the handle, the token holder
//! that cancels it on drop, and the per-side shutdown outcome.
//!
//! `mod.rs` re-exports only — the implementation lives in `handle.rs`. The
//! parent module (`observability/mod.rs`) re-exports the public names, so the
//! public API paths are unchanged.

mod handle;

pub use handle::{ObservabilityHandle, ObservabilityShutdownOutcome, OBS_TASK_JOIN_TIMEOUT};

pub(super) use handle::CancelOnDrop;
