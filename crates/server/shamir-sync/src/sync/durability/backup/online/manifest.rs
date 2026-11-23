use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

#[cfg(test)]
use super::copy_engine::test_seams;
use crate::sync::durability::backup::ManifestFileEntry;

/// Current online-backup manifest format.
pub const MANIFEST_FORMAT_VERSION_V2: u32 = 2;

/// Whether the snapshot was captured online or offline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackupKind {
    Offline,
    Online,
}

/// Captured consistency point for one repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConsistency {
    pub wal_position: u64,
    pub store_dir: String,
    pub schema_version: u32,
}

/// Consistency point for all captured stores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsistencySection {
    pub per_repo: BTreeMap<String, RepoConsistency>,
    pub system_store_version: u64,
}

/// One captured WAL segment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalSegmentEntry {
    pub seq: u64,
    pub file: String,
    pub sidecar: Option<String>,
    pub sha256: String,
    pub size: u64,
}

/// WAL segments captured from one repository directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalDirEntry {
    pub repo: String,
    pub dir: String,
    pub segments: Vec<WalSegmentEntry>,
}

/// Captured WAL directories.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalSection {
    pub dirs: Vec<WalDirEntry>,
}

/// Fields unknown to this version, retained as top-level manifest keys.
/// serde_json::Map defaults to BTree ordering for deterministic output.
#[derive(Debug, Clone, Default)]
pub struct UnknownKeys(pub Map<String, Value>);

impl Serialize for UnknownKeys {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for UnknownKeys {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct UnknownKeysVisitor;

        impl<'de> Visitor<'de> for UnknownKeysVisitor {
            type Value = UnknownKeys;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a map of unknown manifest fields")
            }

            fn visit_map<A>(self, mut access: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut keys = Map::new();
                while let Some((key, value)) = access.next_entry()? {
                    keys.insert(key, value);
                }
                Ok(UnknownKeys(keys))
            }
        }

        deserializer.deserialize_map(UnknownKeysVisitor)
    }
}

/// Versioned snapshot manifest; flattened unknown fields survive decode/encode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestV2 {
    pub format_version: u32,
    #[serde(default = "offline_kind")]
    pub kind: BackupKind,
    pub created_at_unix_ns: u128,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format_epoch: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binary_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub consistency: Option<ConsistencySection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wal: Option<WalSection>,
    pub files: Vec<ManifestFileEntry>,
    #[serde(flatten)]
    pub extra: UnknownKeys,
}

fn offline_kind() -> BackupKind {
    BackupKind::Offline
}

/// Decode a v1 offline or v2 manifest, refusing unsupported versions.
pub fn decode(raw: &str) -> Result<ManifestV2, ManifestError> {
    let value: Value = serde_json::from_str(raw)?;
    let version = value
        .get("format_version")
        .ok_or(ManifestError::MissingField("format_version"))?;
    let found: u32 = serde_json::from_value(version.clone())?;
    match found {
        1 | MANIFEST_FORMAT_VERSION_V2 => Ok(serde_json::from_value(value)?),
        found => Err(ManifestError::UnsupportedVersion { found }),
    }
}

/// Encode a manifest as pretty JSON with a trailing newline.
pub fn encode(manifest: &ManifestV2) -> Result<String, ManifestError> {
    let mut encoded = serde_json::to_string_pretty(manifest)?;
    encoded.push('\n');
    Ok(encoded)
}

/// Construct an online v2 manifest.
pub fn build_v2(
    created_at_unix_ns: u128,
    files: Vec<ManifestFileEntry>,
    format_epoch: Option<u64>,
    binary_version: Option<String>,
    consistency: ConsistencySection,
    wal: WalSection,
) -> ManifestV2 {
    ManifestV2 {
        format_version: MANIFEST_FORMAT_VERSION_V2,
        kind: BackupKind::Online,
        created_at_unix_ns,
        format_epoch,
        binary_version,
        consistency: Some(consistency),
        wal: Some(wal),
        files,
        extra: UnknownKeys::default(),
    }
}

/// Snapshot metadata file name (design 4.6).
pub const BACKUP_META_FILE_NAME: &str = "backup.meta.json";
const BACKUP_META_TMP_NAME: &str = "backup.meta.json.tmp";

/// Write the backup engine metadata file atomically into the destination directory.
pub fn write_backup_meta(dest_dir: &Path) -> std::io::Result<()> {
    let target_path = dest_dir.join(BACKUP_META_FILE_NAME);
    let tmp_path = dest_dir.join(BACKUP_META_TMP_NAME);
    let contents = "{\n  \"encryption\": \"none\",\n  \"engine\": \"copy+wal-replay\"\n}\n";
    match fs::remove_file(&tmp_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut tmp = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp_path)?;
    tmp.write_all(contents.as_bytes())?;
    tmp.sync_all()?;
    drop(tmp);
    fs::rename(&tmp_path, &target_path)?;
    fsync_dest_dir(dest_dir)
}

/// Write an encoded manifest into the snapshot root atomically (tmp file,
/// fsync, rename) — written LAST (design 4.2 step F).
pub fn write_manifest(dest_dir: &Path, encoded: &str) -> std::io::Result<()> {
    let tmp = dest_dir.join("manifest.json.tmp");
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(encoded.as_bytes())?;
        file.sync_all()?;
    }
    let target = dest_dir.join("manifest.json");
    // Test-only seam: an injected result REPLACES the rename result the
    // publication consumes, so the injected failure is classified by the
    // protocol exactly like a genuine rename failure.
    #[cfg(test)]
    let renamed =
        test_seams::fire_manifest_rename(&target).unwrap_or_else(|| fs::rename(&tmp, &target));
    #[cfg(not(test))]
    let renamed = fs::rename(&tmp, &target);
    renamed?;
    fsync_dest_dir(dest_dir)
}

#[cfg(unix)]
fn fsync_dir_raw(dir: &Path) -> std::io::Result<()> {
    fs::File::open(dir)?.sync_all()
}

/// Windows cannot open a directory handle for fsync: nudge the metadata file
/// inside `dir` instead when it exists.
#[cfg(not(unix))]
fn fsync_dir_raw(dir: &Path) -> std::io::Result<()> {
    if let Ok(file) = fs::File::open(dir.join(BACKUP_META_FILE_NAME)) {
        let _ = file.sync_all();
    }
    Ok(())
}

/// Fsync a destination directory so the entries created inside it are
/// durable across a crash. This is the ONE directory-fsync rule of the online
/// copy: the copy passes (`copy_engine::fsync_dir_fallible`), [`write_manifest`]
/// and [`write_backup_meta`] all go through it and through
/// [`tolerate_dir_sync_capability`], so a filesystem that cannot fsync a
/// directory can never pass the copy and then fail the publication. Two kinds
/// stay tolerated by design: `Unsupported` (the platform cannot fsync a
/// directory at all) and `InvalidInput` (some filesystems, e.g. network/FUSE
/// mounts, reject an fsync of a directory handle): failing the backup for them
/// would make it impossible on such a filesystem while the published bytes are
/// as durable as that filesystem offers. Every other failure propagates to the
/// caller, which types it as a destination failure naming the DIRECTORY it
/// could not sync. Under `cfg(test)` the seam `test_seams::fire_fsync_dir`
/// substitutes the result, and the substituted result is processed by
/// [`tolerate_dir_sync_capability`] — the very same rule a genuine result
/// takes.
pub(crate) fn fsync_dest_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    let result = match test_seams::fire_fsync_dir(dir) {
        Some(injected) => injected,
        None => fsync_dir_raw(dir),
    };
    #[cfg(not(test))]
    let result = fsync_dir_raw(dir);
    tolerate_dir_sync_capability(result)
}

/// The tolerance of [`fsync_dest_dir`] as a function of one fsync result, so
/// the test seam that substitutes the result passes through the same rule.
pub(crate) fn tolerate_dir_sync_capability(result: std::io::Result<()>) -> std::io::Result<()> {
    match result {
        Ok(()) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::Unsupported | std::io::ErrorKind::InvalidInput
            ) =>
        {
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Manifest codec and file writes can fail on malformed input or storage errors.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest io: {0}")]
    Io(#[from] std::io::Error),
    #[error("manifest json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported manifest format_version: found {found}")]
    UnsupportedVersion { found: u32 },
    #[error("manifest is missing required field: {0}")]
    MissingField(&'static str),
}
