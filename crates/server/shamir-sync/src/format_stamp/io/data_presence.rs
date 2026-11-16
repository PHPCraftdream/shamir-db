//! "Data present" test and its ignore set (decision D8, design 3.1).

use std::fs;
use std::path::Path;

use crate::format_stamp::codec::{STAMP_FILE_NAME, STAMP_TMP_NAME};

/// Names never counted as data (the lock file the caller created before the
/// check, the stamp and its tmp). A configured log file name is added by the
/// caller; the module knows no config (design 3.1).
pub struct IgnoreSet {
    log_file: Option<String>,
}

impl IgnoreSet {
    /// Only the built-in ignore names.
    pub fn none() -> Self {
        Self { log_file: None }
    }

    /// Built-in names plus one configured log file name (e.g. `audit.log`).
    pub fn with_log_file(log_file: impl Into<String>) -> Self {
        Self {
            log_file: Some(log_file.into()),
        }
    }

    fn ignores(&self, name: &str) -> bool {
        name == STAMP_FILE_NAME
            || name == STAMP_TMP_NAME
            || name == ".shamir.lock"
            || self.log_file.as_deref() == Some(name)
    }
}

/// True when `dir` contains at least one entry outside the ignore set.
/// A missing directory counts as empty (the caller creates it first).
pub fn data_present(dir: &Path, ignore: &IgnoreSet) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        if !ignore.ignores(&entry.file_name().to_string_lossy()) {
            return true;
        }
    }
    false
}
