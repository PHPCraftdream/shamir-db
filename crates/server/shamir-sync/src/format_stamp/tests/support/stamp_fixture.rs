//! Shared stamp fixtures: temp dirs, stamp writers, corrupt frames.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;

use tempfile::TempDir;

pub use super::dir_snapshot::DirSnapshot;

use crate::format_stamp::codec::{encode, STAMP_FILE_NAME, STAMP_TMP_NAME};
use crate::format_stamp::model::{EpochPolicy, FormatStamp};

/// A binary version different from this crate's, for D6 tests.
pub const OTHER_VERSION: &str = "9.9.9-test";
/// Fixed timestamp for reproducible fixtures.
pub const FIXTURE_UNIX_MS: u64 = 1_700_000_000_000;

pub fn temp_dir() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

/// Fixture policy: `current` as current epoch, `base` as base epoch.
pub fn policy(current: u64, base: u64) -> EpochPolicy {
    EpochPolicy::new(current, base)
}

pub fn stamp_with_epoch(epoch: u64) -> FormatStamp {
    FormatStamp::with_epoch(epoch, OTHER_VERSION, FIXTURE_UNIX_MS)
}

/// Write a valid stamp with `epoch` through the public encoder.
pub fn write_stamp_with_epoch(dir: &Path, epoch: u64) {
    let stamp = stamp_with_epoch(epoch);
    crate::format_stamp::io::write_stamp(dir, &stamp).expect("write fixture stamp");
}

/// Write a valid stamp with `epoch` created by this crate's own version, so a
/// Writer check finds nothing to rewrite (D6: rewritten only on a version change).
pub fn write_same_version_stamp_with_epoch(dir: &Path, epoch: u64) {
    let stamp = FormatStamp::with_epoch(epoch, env!("CARGO_PKG_VERSION"), FIXTURE_UNIX_MS);
    crate::format_stamp::io::write_stamp(dir, &stamp).expect("write fixture stamp");
}

/// Write raw bytes as `format.stamp` (bypasses the encoder).
pub fn write_raw_stamp(dir: &Path, bytes: &[u8]) {
    fs::write(dir.join(STAMP_FILE_NAME), bytes).expect("write raw stamp");
}

/// Encode a valid frame for `epoch` (in-memory, for codec tests).
pub fn valid_frame(epoch: u64) -> Vec<u8> {
    encode(&stamp_with_epoch(epoch)).expect("encode fixture stamp")
}

/// Simulate an interrupted write: a leftover tmp with junk.
pub fn add_leftover_tmp(dir: &Path) {
    fs::write(dir.join(STAMP_TMP_NAME), b"junk").expect("write tmp");
}

/// Simulate data: one directory entry outside the ignore set.
pub fn add_data(dir: &Path) {
    fs::create_dir_all(dir.join("server_meta")).expect("create server_meta");
}

/// Simulate the lock file the caller creates before the check.
pub fn add_lock(dir: &Path) {
    fs::write(dir.join(".shamir.lock"), b"1234").expect("write lock");
}

/// Rewrite the header epoch of a frame and recompute the checksum, so the
/// frame decodes up to (and reports) the body/header epoch mismatch.
pub fn with_header_epoch(mut frame: Vec<u8>, epoch: u64) -> Vec<u8> {
    frame[6..14].copy_from_slice(&epoch.to_le_bytes());
    let body_end = frame.len() - 32;
    let digest = Sha256::digest(&frame[..body_end]);
    frame[body_end..].copy_from_slice(&digest);
    frame
}
