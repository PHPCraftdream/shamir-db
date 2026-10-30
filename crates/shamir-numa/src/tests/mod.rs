//! Platform-independent unit tests run in the workspace CI matrix.
//! Linux integration tests exercise sysfs; QEMU CI is a topology smoke test,
//! not full Rust integration inside the guest.

pub mod cache_padded_tests;
pub mod cpulist_tests;
pub mod fallback_tests;
pub mod mock_tests;
pub mod node_replicated_tests;
pub mod node_tests;
