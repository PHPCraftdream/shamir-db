//! Owner-private directory primitive shared by the wasm compile cache and
//! the Named-Pipe IPC transport (DEC-3 Q5-c).
//!
//! [`create_private_dir`] creates a directory that only the current OS
//! account can access; [`verify_private_dir`] re-checks an existing path at
//! every open: it must be a real directory (never a symlink / reparse
//! point), owned by the current user, with no access for anyone else
//! (Unix: mode `0700`; Windows: protected DACL with exactly one
//! full-access ACE for the current user SID). A tampered cache entry is
//! executable code, so the directory is verified at every open, not only
//! at creation.
//!
//! Platform parity is "the OS account this process runs as": on Unix the
//! euid, on Windows the current process token's user SID (not the logon
//! SID and not `OWNER RIGHTS` — a service account has no stable logon SID
//! across restarts, so the user SID is the stable anchor).
//!
//! This crate is the ONLY place for the `unsafe` Windows FFI of this
//! feature; the module holding it is `#[cfg(windows)]` and every `unsafe`
//! block carries a `// SAFETY:` comment. All public APIs are safe.

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub use unix::{create_private_dir, verify_private_dir};
#[cfg(windows)]
pub use windows::{create_private_dir, verify_private_dir, OwnerOnlySecurityDescriptor};

#[cfg(test)]
mod tests;
