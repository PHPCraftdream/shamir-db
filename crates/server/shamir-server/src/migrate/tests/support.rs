use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use shamir_sync::format_stamp::migration::{
    FormatStep, MigrationError, ModuleDescriptor, PlanLine, StepContext,
};
use shamir_sync::format_stamp::{decode, encode, write_stamp, EpochPolicy, FormatStamp};
use tempfile::TempDir;

use crate::config::{Config, KdfConfig, LoggingConfig, TlsConfig};
use crate::migrate::FormatMigrateArgs;

pub const FIXED_UNIX_MS: u64 = 1_700_000_000_000;

static EMPTY_DESCRIPTOR: ModuleDescriptor = ModuleDescriptor {
    id: "e0001_0002",
    from_epoch: 1,
    to_epoch: 2,
    summary: "empty test step",
    touches: &[],
    work_estimate: "none",
};

/// Test-only format step patterned after shamir-sync's `tests/support/empty_step.rs`.
pub struct EmptyStep;

impl FormatStep for EmptyStep {
    fn descriptor(&self) -> &ModuleDescriptor {
        &EMPTY_DESCRIPTOR
    }

    fn apply(&self, _context: &mut StepContext<'_>) -> Result<(), MigrationError> {
        Ok(())
    }

    fn plan(&self, _context: &StepContext<'_>) -> Result<Vec<PlanLine>, MigrationError> {
        Ok(vec![PlanLine {
            description: "e0001_0002: empty test step".to_owned(),
        }])
    }
}

pub struct CountingStep {
    calls: Arc<AtomicUsize>,
}

impl CountingStep {
    pub fn new(calls: Arc<AtomicUsize>) -> Self {
        Self { calls }
    }
}

impl FormatStep for CountingStep {
    fn descriptor(&self) -> &ModuleDescriptor {
        &EMPTY_DESCRIPTOR
    }

    fn apply(&self, _context: &mut StepContext<'_>) -> Result<(), MigrationError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn plan(&self, _context: &StepContext<'_>) -> Result<Vec<PlanLine>, MigrationError> {
        Ok(vec![PlanLine {
            description: "e0001_0002: counting test step".to_owned(),
        }])
    }
}

pub fn policy() -> EpochPolicy {
    EpochPolicy::new(2, 1)
}

pub fn config(dir: &Path) -> Config {
    Config {
        data_dir: dir.to_path_buf(),
        logging: LoggingConfig::default(),
        durability: Default::default(),
        transactions: Default::default(),
        tokens: Default::default(),
        kdf_defaults: KdfConfig {
            memory_kb: 19_456,
            time: 2,
            parallelism: 1,
            argon2_version: 0x13,
        },
        argon2_concurrent_max: 64,
        listeners: Vec::new(),
        tls: TlsConfig {
            cert_path: dir.join("cert.pem"),
            key_path: dir.join("key.pem"),
        },
        security: Default::default(),
        audit: Default::default(),
        observability: Default::default(),
        replication: None,
    }
}

pub fn older_data_dir(dir: &Path) {
    fs::create_dir_all(dir).expect("create data directory");
    write_stamp(
        dir,
        &FormatStamp::with_epoch(1, "format-migrate-test", FIXED_UNIX_MS),
    )
    .expect("write older stamp");
    fs::write(dir.join("payload.bin"), b"preserve this payload").expect("write payload");
}

pub fn write_epoch(dir: &Path, epoch: u64) {
    fs::create_dir_all(dir).expect("create data directory");
    write_stamp(
        dir,
        &FormatStamp::with_epoch(epoch, "format-migrate-test", FIXED_UNIX_MS),
    )
    .expect("write stamp");
}

pub fn write_adopted_epoch(dir: &Path, epoch: u64) {
    fs::create_dir_all(dir).expect("create data directory");
    write_stamp(
        dir,
        &FormatStamp::adopted(epoch, "adoption-test", FIXED_UNIX_MS),
    )
    .expect("write adopted stamp");
}

pub fn read_stamp(dir: &Path) -> FormatStamp {
    decode(
        &fs::read(dir.join("format.stamp")).expect("read stamp"),
        dir,
    )
    .expect("decode stamp")
}

pub fn fresh_backup(live_dir: &Path, backup_root: &Path) -> PathBuf {
    crate::backup::backup(live_dir, backup_root)
        .expect("produce backup with the real backup implementation")
        .dest_dir
}

pub fn rewrite_backup_time(backup_dir: &Path, unix_ns: u64) {
    let manifest_path = backup_dir.join(crate::backup::MANIFEST_FILE_NAME);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(&manifest_path).expect("read backup manifest"))
            .expect("parse backup manifest");
    manifest["created_at_unix_ns"] = serde_json::Value::from(unix_ns);
    fs::write(
        manifest_path,
        serde_json::to_vec_pretty(&manifest).expect("encode backup manifest"),
    )
    .expect("rewrite backup timestamp");
}

pub fn future_unix_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after Unix epoch")
        .as_nanos() as u64
        + 86_400_000_000_000
}

pub fn args(
    yes: bool,
    plan: bool,
    from_backup: Option<PathBuf>,
    override_gate: bool,
) -> FormatMigrateArgs {
    FormatMigrateArgs {
        plan,
        from_backup,
        max_backup_age_hours: 24,
        i_have_a_backup: override_gate,
        yes,
    }
}

pub fn default_step() -> &'static dyn FormatStep {
    static STEP: EmptyStep = EmptyStep;
    &STEP
}

pub fn integrity_ok(_dir: &Path) -> std::result::Result<(), MigrationError> {
    Ok(())
}

#[derive(Clone, Default)]
pub struct SharedSink(Arc<Mutex<Vec<u8>>>);

impl SharedSink {
    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().expect("output mutex").clone()).expect("output is UTF-8")
    }
}

impl Write for SharedSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("output mutex poisoned"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub struct Captured {
    pub result: Result<()>,
    pub stdout: String,
    pub stderr: String,
}

pub fn invoke(config: &Config, command_args: &FormatMigrateArgs) -> Captured {
    let modules: [&dyn FormatStep; 1] = [default_step()];
    invoke_with(config, command_args, &modules, &integrity_ok)
}

pub fn invoke_with(
    config: &Config,
    command_args: &FormatMigrateArgs,
    modules: &[&dyn FormatStep],
    integrity: &dyn Fn(&Path) -> std::result::Result<(), MigrationError>,
) -> Captured {
    let stdout = SharedSink::default();
    let mut stderr = SharedSink::default();
    let result = super::super::execution::run_with(
        config,
        command_args,
        policy(),
        modules,
        integrity,
        stdout.clone(),
        &mut stderr,
    );
    if let Err(error) = &result {
        writeln!(stderr, "{error:#}").expect("write CLI-equivalent error to captured stderr");
    }
    Captured {
        result,
        stdout: stdout.text(),
        stderr: stderr.text(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SnapshotEntry {
    relative_path: PathBuf,
    directory: bool,
    bytes: Option<Vec<u8>>,
    modified: SystemTime,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirSnapshot(Vec<SnapshotEntry>);

impl DirSnapshot {
    pub fn take(root: &Path) -> Self {
        let mut entries = Vec::new();
        snapshot_walk(root, root, &mut entries);
        entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Self(entries)
    }

    pub fn assert_unchanged(&self, root: &Path) {
        assert_eq!(
            *self,
            Self::take(root),
            "data-directory listing, bytes, or mtime changed"
        );
    }

    /// Compare all pre-existing data byte-for-byte and by mtime while allowing only named top-level artifacts.
    pub fn assert_unchanged_except_top_level_files(&self, root: &Path, allowed: &[&str]) {
        let mut before = self.0.clone();
        let mut after = Self::take(root).0;
        for entries in [&mut before, &mut after] {
            entries.retain(|entry| {
                if entry.relative_path.as_os_str().is_empty() {
                    return false;
                }
                !(entry.relative_path.components().count() == 1
                    && allowed
                        .iter()
                        .any(|name| entry.relative_path == Path::new(*name)))
            });
        }
        assert_eq!(
            before, after,
            "pre-existing data changed outside approved top-level artifacts"
        );
    }
}

fn snapshot_walk(root: &Path, directory: &Path, output: &mut Vec<SnapshotEntry>) {
    let metadata = fs::metadata(directory).expect("directory metadata");
    let relative_path = directory
        .strip_prefix(root)
        .expect("directory is beneath snapshot root")
        .to_path_buf();
    output.push(SnapshotEntry {
        relative_path,
        directory: true,
        bytes: None,
        modified: metadata.modified().expect("directory mtime"),
    });
    for entry in fs::read_dir(directory).expect("read snapshot directory") {
        let entry = entry.expect("read directory entry");
        let path = entry.path();
        let metadata = entry.metadata().expect("entry metadata");
        let relative_path = path
            .strip_prefix(root)
            .expect("entry is beneath snapshot root")
            .to_path_buf();
        if metadata.is_dir() {
            snapshot_walk(root, &path, output);
        } else {
            output.push(SnapshotEntry {
                relative_path,
                directory: false,
                bytes: Some(fs::read(&path).expect("read snapshot file")),
                modified: metadata.modified().expect("file mtime"),
            });
        }
    }
}

pub fn temp() -> TempDir {
    tempfile::tempdir().expect("temporary directory")
}

pub fn mapping_exit_code(result: &Result<()>) -> u8 {
    // Mirrors main.rs: successful command => 0; returned error => ExitCode::FAILURE (1).
    if result.is_ok() {
        0
    } else {
        1
    }
}

pub fn unix_ns_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after Unix epoch")
        .as_nanos() as u64
}

pub fn encoded_stamp(stamp: &FormatStamp) -> Vec<u8> {
    encode(stamp).expect("encode fixture stamp")
}
