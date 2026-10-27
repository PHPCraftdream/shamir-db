//! Platform-specific topology discovery and pinning backends.

pub(crate) mod cpulist;
pub(crate) mod detect;
pub(crate) mod fallback;
#[cfg(target_os = "linux")]
pub(crate) mod linux;
pub(crate) mod mock;
#[cfg(test)]
mod tests;

pub use cpulist::*;
pub use detect::*;
pub use fallback::*;
#[cfg(target_os = "linux")]
pub use linux::*;
pub use mock::*;
