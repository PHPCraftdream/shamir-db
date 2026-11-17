//! Online backup copy engine (design `online-backup.md` 4.1, 4.2).

pub mod copy_engine;
pub mod manifest;
pub mod progress;
pub mod protocol;
pub mod wal_capture;

#[cfg(test)]
mod tests;
