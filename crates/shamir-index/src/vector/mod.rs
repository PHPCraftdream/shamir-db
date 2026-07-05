//! Vector similarity index — adapter trait + in-process
//! brute-force implementation (HNSW upgrade path planned).

pub mod adapter;
pub mod brute_force;
pub mod hnsw_adapter;
pub mod quant_meta;
pub mod quantized_dist;
pub mod simd;
pub mod snapshot;
pub mod sq8;
pub mod vector_backend;

pub use adapter::{SearchOpts, VectorAdapter};
pub use vector_backend::VectorBackend;

#[cfg(test)]
mod tests;
