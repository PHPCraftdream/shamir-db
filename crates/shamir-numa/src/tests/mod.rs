//! Tier-1 unit tests — DI-mock based, platform-independent.
//!
//! These run on every OS (Windows / macOS / Linux) and require no real NUMA
//! hardware, so they ride the workspace's existing
//! `cargo test --workspace --lib` CI matrix. The real `LinuxTopology` against
//! `/sys` (Tier 2) and QEMU NUMA emulation (Tier 3) land in Фаза 1b.

pub mod cache_padded_tests;
pub mod cpulist_tests;
pub mod fallback_tests;
pub mod mock_tests;
pub mod node_replicated_tests;
pub mod node_tests;
