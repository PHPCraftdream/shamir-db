//! `IndexBackend` wrapper around any `VectorAdapter`.
//!
//! Extracts the vector field from records, delegates to the adapter
//! for similarity search. Returns `IndexResult::Ranked`.

mod backend;
mod maintenance;

#[cfg(test)]
pub(crate) use backend::AdapterSlot;
pub use backend::VectorBackend;
