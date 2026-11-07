//! Vector similarity index — adapter trait + in-process
//! brute-force implementation (HNSW upgrade path planned).

pub mod hnsw_adapter;
pub mod quant;
pub mod simd;
pub mod snapshot;
pub mod vector_backend;

pub use quant::adapter;
pub use quant::adapter::{SearchOpts, VectorAdapter};
pub use quant::brute_force;
pub use quant::quant_meta;
pub use quant::quantized_dist;
pub use quant::sq8;
pub use vector_backend::VectorBackend;

#[cfg(test)]
mod tests;
