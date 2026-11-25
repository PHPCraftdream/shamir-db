//! Shared fixtures for the format-stamp wiring tests: a minimal boot
//! config, a recursive directory snapshot, and stamp fixtures.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use shamir_sync::format_stamp::{write_stamp, EpochPolicy, FormatStamp};

use crate::config::{Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, TlsConfig};
use crate::server::ServerLauncher;

/// Fixed timestamp for reproducible fixtures.
pub const FIXTURE_UNIX_MS: u64 = 1_700_000_000_000;

/// A minimal single-HTTP-listener config whose `data_dir` is `dir`
/// (same shape as the launcher tests in `server_tests.rs`).
pub fn config(dir: &Path) -> Config {
    Config {
        data_dir: dir.to_path_buf(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        durability: Default::default(),
        transactions: Default::default(),
        tokens: Default::default(),
        kdf_defaults: KdfConfig {
            memory_kb: 19_456,
            time: 2,
            parallelism: 1,
            argon2_version: 0x13,
        },
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Http,
            addr: "127.0.0.1:0".into(),
            profile: None,
            http: crate::config::HttpListenerSettings {
                tls: Some(false),
                ..Default::default()
            },
            path: None,
            kdf_override: None,
            browser_origin_allowlist: Vec::new(),
        }],
        tls: TlsConfig {
            cert_path: dir.join("cert.pem"),
            key_path: dir.join("key.pem"),
        },
        security: Default::default(),
        audit: Default::default(),
        observability: crate::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
        backup: None,
    }
}

/// Launcher with the test config and `BootstrapMode::Skip`.
pub fn launcher(dir: &Path) -> ServerLauncher {
    ServerLauncher {
        config: config(dir),
        bootstrap: crate::server::BootstrapMode::Skip,
        node_mode: crate::db_handler::NodeMode::ReadWrite,
    }
}

/// Recursive listing (relative path, size, mtime) of `dir`, excluding only
/// `.shamir.lock` — the lock file every Writer caller creates first.
pub struct DirSnapshot {
    entries: Vec<(String, u64, SystemTime)>,
}

impl DirSnapshot {
    pub fn take(dir: &Path) -> Self {
        let mut entries = Vec::new();
        walk(dir, dir, &mut entries);
        entries.sort();
        Self { entries }
    }

    /// Listing, sizes and mtimes identical.
    pub fn assert_unchanged(&self, dir: &Path) {
        let mut now = Vec::new();
        walk(dir, dir, &mut now);
        now.sort();
        assert_eq!(self.entries, now, "directory changed");
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, u64, SystemTime)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".shamir.lock" {
            continue;
        }
        let path = entry.path();
        let rel = path
            .strip_prefix(root)
            .expect("entry under root")
            .to_string_lossy()
            .to_string();
        let meta = entry.metadata().expect("metadata");
        let (size, mtime) = if meta.is_dir() {
            (0, meta.modified().unwrap_or(SystemTime::UNIX_EPOCH))
        } else {
            (
                meta.len(),
                meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            )
        };
        out.push((rel, size, mtime));
        if meta.is_dir() {
            walk(root, &path, out);
        }
    }
}

/// Write a valid stamp with `epoch` created by this crate's own version, so
/// a Writer check finds nothing to rewrite (rule D6).
pub fn write_same_version_stamp(dir: &Path, epoch: u64) {
    let stamp = FormatStamp::with_epoch(epoch, env!("CARGO_PKG_VERSION"), FIXTURE_UNIX_MS);
    write_stamp(dir, &stamp).expect("write fixture stamp");
}

/// Path of the compiled constants for fixtures that need "always newer".
pub fn compiled_current_epoch() -> u64 {
    EpochPolicy::compiled().current
}

/// One directory entry that counts as data for `data_present`.
pub fn add_data_entry(dir: &Path) -> PathBuf {
    let p = dir.join("server_meta");
    fs::create_dir_all(&p).expect("create server_meta");
    p
}
