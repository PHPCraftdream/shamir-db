// Disk-tier regression suite: every disk-store module is `#[cfg(unix)]`
// (the disk tier is disabled on non-Unix platforms); the single non-Unix
// test asserts that `with_compile_cache` degrades to an uncached engine.
#[cfg(unix)]
pub mod cache_store_tests;
#[cfg(unix)]
pub mod namespace_tests;
#[cfg(unix)]
pub mod trap_parity_tests;
#[cfg(unix)]
pub mod trim_tests;
#[cfg(windows)]
pub mod windows_degrade_tests;
