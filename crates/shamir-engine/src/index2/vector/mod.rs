//! Vector similarity index — adapter trait + in-process
//! brute-force implementation (HNSW upgrade path planned).

pub mod adapter;
pub mod brute_force;
pub mod hnsw_adapter;
pub mod vector_backend;

pub use adapter::VectorAdapter;
pub use vector_backend::VectorBackend;
