// Disk-tier regression suite. Since Q5-c the disk tier is enabled and
// private-directory-verified on BOTH Unix and Windows; the tests below
// run cross-platform except where they exercise a truly Unix-only API
// (mode bits, symlinks) or a Windows-only one (DACL, junctions).
pub mod cache_store_tests;
pub mod namespace_tests;
pub mod trap_parity_tests;
pub mod trim_tests;
#[cfg(windows)]
pub mod windows_trust_tests;
