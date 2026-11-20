use std::fs::Metadata;
use std::io::{self, Write};
use std::path::Path;

use serde_json::{json, Map, Value};
use tempfile::NamedTempFile;

use crate::sync::durability::backup::online::manifest::BACKUP_META_FILE_NAME;

use super::budget::{Budget, Refusal};

const TAG: &str = "scheduled_retention";
const MAX_SIDECAR: u64 = 64 << 10;

pub(super) fn read_sidecar(
    dir: &Path,
    root_meta: &Metadata,
    budget: &mut Budget<'_>,
) -> Result<Map<String, Value>, Refusal> {
    let bytes = budget.read(&dir.join(BACKUP_META_FILE_NAME), root_meta, MAX_SIDECAR)?;
    serde_json::from_slice(&bytes).map_err(|_| Refusal::Unverifiable)
}

pub(super) fn has_tag(sidecar: &Map<String, Value>) -> bool {
    sidecar.contains_key(TAG)
}

pub(super) fn is_owned(sidecar: &Map<String, Value>, owner: &str) -> bool {
    sidecar.get(TAG).is_some_and(|tag| {
        tag.get("version").and_then(Value::as_u64) == Some(1)
            && tag.get("owner").and_then(Value::as_str) == Some(owner)
    })
}

pub(super) fn tag_fresh(
    dir: &Path,
    mut sidecar: Map<String, Value>,
    owner: &str,
    budget: &Budget<'_>,
) -> Result<(), Refusal> {
    if has_tag(&sidecar) {
        return if is_owned(&sidecar, owner) {
            Ok(())
        } else {
            Err(Refusal::Unverifiable)
        };
    }
    sidecar.insert(TAG.to_owned(), json!({ "version": 1, "owner": owner }));
    let mut bytes = serde_json::to_vec(&sidecar).map_err(|_| Refusal::Incomplete)?;
    bytes.push(b'\n');
    if bytes.len() as u64 > MAX_SIDECAR {
        return Err(Refusal::Budget);
    }
    budget.checkpoint()?;
    let mut staged = NamedTempFile::new_in(dir).map_err(|_| Refusal::Incomplete)?;
    staged.write_all(&bytes).map_err(|_| Refusal::Incomplete)?;
    staged
        .as_file()
        .sync_all()
        .map_err(|_| Refusal::Incomplete)?;
    budget.checkpoint()?;
    staged
        .persist(dir.join(BACKUP_META_FILE_NAME))
        .map_err(|_| Refusal::Incomplete)?;
    sync_directory(dir).map_err(|_| Refusal::Incomplete)
}

#[cfg(unix)]
fn sync_directory(dir: &Path) -> io::Result<()> {
    std::fs::File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_dir: &Path) -> io::Result<()> {
    // The staged file was flushed before atomic replacement. Windows does
    // not support the Unix directory fsync pattern.
    Ok(())
}
