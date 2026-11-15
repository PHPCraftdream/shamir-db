//! Atomic alias module — every atomic access goes through here so the loom
//! model can swap the implementation without touching the protocol.
//!
//! Under `cfg(loom)` atomics come from `loom` (an optional dependency enabled
//! by the `loom` cargo feature); production builds use `core::sync::atomic`.

#[cfg(not(loom))]
pub use core::sync::atomic::{fence, AtomicU64, Ordering};
#[cfg(loom)]
pub use loom::sync::atomic::{fence, AtomicU64, Ordering};
