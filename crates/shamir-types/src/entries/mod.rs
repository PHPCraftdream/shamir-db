//! Top-level entry types (access control, secrets, timestamps) — re-exports
//! only (CLAUDE.md §test-organisation); logic lives in siblings.

pub mod access;
pub mod secret;
pub mod time;

#[cfg(test)]
pub mod tests;
