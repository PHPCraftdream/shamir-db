//! Supporting types for the tx layer: basic tx types, metrics, value
//! remapping, and the layered interner.

pub mod id_remap;
pub mod layered_interner;
#[path = "core/metrics.rs"]
pub mod metrics;
pub mod promote_ticket;
pub(crate) mod staged_field_index;
#[path = "core/types.rs"]
pub mod types;

#[cfg(test)]
mod tests;
