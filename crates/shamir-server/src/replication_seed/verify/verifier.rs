use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

use shamir_types::types::value::QueryValue;

use crate::replication_seed::codec::{
    decode, digest, encode, hash_reader, referenced, ARTIFACT_LIMIT, CHUNK_LIMIT, FRAME_LIMIT,
    METADATA_LIMIT,
};
use crate::replication_seed::error::invalid;
use crate::replication_seed::schema_digest::schema_digest;
use crate::replication_seed::{Manifest, SeedError, TableRecord};

/// Verify a static, operator-owned tree. No repository is opened. SHA-256 is
/// integrity, not provenance; the printed digest needs a trusted operator channel.
pub fn verify(root: &Path) -> Result<String, SeedError> {
    let metadata = fs::symlink_metadata(root)?;
    reject_link(&metadata)?;
    if !metadata.is_dir() {
        return Err(invalid("seed is not a directory"));
    }
    let mut actual = BTreeSet::new();
    let mut actual_dirs = BTreeSet::new();
    inventory(root, "", &mut actual, &mut actual_dirs, 0)?;
    let bytes = read_bounded(&root.join("manifest.mpack"), METADATA_LIMIT)?;
    let manifest: Manifest = decode(&bytes)?;
    if encode(&manifest)? != bytes {
        return Err(invalid("noncanonical manifest"));
    }
    let hash = digest(&bytes);
    if read_bounded(&root.join("Complete"), 64)? != hash.as_bytes() {
        return Err(invalid("Complete digest mismatch"));
    }
    if manifest.format_version != 1
        || manifest.codec_version != 1
        || manifest.index_policy != "EXCLUDED"
        || manifest.requirements != vec!["mvcc", "raw-inner-value", "schema-names"]
        || manifest
            .tables
            .windows(2)
            .any(|pair| pair[0].name >= pair[1].name)
        || manifest.source_db.is_empty()
        || manifest.source_repo.is_empty()
        || manifest.source_db == "system"
        || manifest.source_db == "__system__"
    {
        return Err(invalid("unsupported seed format or contents"));
    }
    if manifest
        .certified_gap
        .is_some_and(|(first, through)| first == 0 || first > through || through > manifest.version)
    {
        return Err(invalid("invalid certified gap"));
    }
    let mut expected = BTreeSet::from(["manifest.mpack".to_string(), "Complete".to_string()]);
    let mut expected_dirs = BTreeSet::new();
    let mut total = bytes.len() as u64 + 64;
    let mut metadata_total = bytes.len() as u64;
    let mut bindings = BTreeMap::new();
    let mut binding_names = BTreeSet::new();
    let mut binding_page = 0u64;
    let mut schema_seen = false;
    let mut row_files: BTreeMap<u32, Vec<&crate::replication_seed::FileRecord>> = BTreeMap::new();
    let mut names = BTreeSet::new();
    for (id, table) in manifest.tables.iter().enumerate() {
        if !names.insert(&table.name) || table.follower_enable_indexes || table.name.is_empty() {
            return Err(invalid("invalid table inventory"));
        }
        validate_schema(&table.schema)?;
        expected_dirs.insert("tables".to_string());
        expected_dirs.insert(format!("tables/{id:06}"));
    }
    for file in &manifest.files {
        if file.path != "schema.mpack" && file.table.is_none() && file.bytes > 1024 * 1024 {
            return Err(invalid("binding page exceeds byte bounds"));
        }
        if !expected.insert(file.path.clone()) {
            return Err(invalid("duplicate file"));
        }
        total = total
            .checked_add(file.bytes)
            .ok_or_else(|| invalid("artifact overflow"))?;
        if total > ARTIFACT_LIMIT {
            return Err(invalid("artifact byte budget exceeded"));
        }
        let path = root.join(&file.path);
        match file.table {
            Some(id) => {
                if id as usize >= manifest.tables.len()
                    || file.bytes > CHUNK_LIMIT
                    || file.rows == 0
                {
                    return Err(invalid("invalid row file"));
                }
                let files = row_files.entry(id).or_default();
                if file.path != format!("tables/{id:06}/rows-{:06}.mpack", files.len()) {
                    return Err(invalid("unsafe or nonsequential row path"));
                }
                files.push(file);
            }
            None => {
                if file.rows != 0 {
                    return Err(invalid("metadata has row count"));
                }
                metadata_total = metadata_total
                    .checked_add(file.bytes)
                    .ok_or_else(|| invalid("metadata overflow"))?;
                if metadata_total > METADATA_LIMIT {
                    return Err(invalid("metadata budget exceeded"));
                }
                if file.path == "schema.mpack" && !schema_seen {
                    schema_seen = true;
                    let schema = read_bounded(&path, file.bytes.min(METADATA_LIMIT))?;
                    let schema_tables = decode::<Vec<TableRecord>>(&schema)?;
                    if schema_digest(&schema_tables)? != manifest.schema_sha256
                        || encode(&schema_tables)? != encode(&manifest.tables)?
                    {
                        return Err(invalid("schema digest/inventory mismatch"));
                    }
                } else if file.path == format!("bindings-{binding_page:06}.mpack") {
                    let entries: Vec<(u64, String)> =
                        decode(&read_bounded(&path, file.bytes.min(1024 * 1024))?)?;
                    if entries.len() > 256 {
                        return Err(invalid("binding page exceeds bounds"));
                    }
                    for (id, name) in entries {
                        if id == 0
                            || name.is_empty()
                            || bindings.contains_key(&id)
                            || !binding_names.insert(name.clone())
                        {
                            return Err(invalid("conflicting binding"));
                        }
                        bindings.insert(id, name);
                    }
                    binding_page = binding_page
                        .checked_add(1)
                        .ok_or_else(|| invalid("page overflow"))?;
                } else {
                    return Err(invalid("unsafe or unknown metadata path"));
                }
            }
        }
        let mut opened = open_regular(&path)?;
        let (size, digest) = hash_reader(&mut opened)?;
        if size != file.bytes || digest != file.sha256 {
            return Err(invalid("file size/hash mismatch"));
        }
    }
    if !schema_seen || binding_page == 0 {
        return Err(invalid("missing schema or bindings"));
    }
    for table in &manifest.tables {
        if let QueryValue::List(rules) = &table.schema {
            for rule in rules {
                let path = rule
                    .get("path")
                    .and_then(QueryValue::as_array)
                    .ok_or_else(|| invalid("schema path missing"))?;
                for name in path {
                    let name = name
                        .as_str()
                        .ok_or_else(|| invalid("invalid schema name"))?;
                    if !binding_names.contains(name) {
                        return Err(invalid("missing schema binding"));
                    }
                }
            }
        }
    }
    if actual != expected || actual_dirs != expected_dirs {
        return Err(invalid("missing or extra artifact entry"));
    }
    for (id, table) in manifest.tables.iter().enumerate() {
        let mut count = 0u64;
        let mut previous: Option<Vec<u8>> = None;
        if let Some(files) = row_files.get(&(id as u32)) {
            for file in files {
                let mut reader = open_regular(&root.join(&file.path))?;
                let mut file_count = 0u64;
                while reader.stream_position()? < file.bytes {
                    let mut prefix = [0; 4];
                    reader.read_exact(&mut prefix)?;
                    let len = u32::from_be_bytes(prefix) as usize;
                    if len == 0 || len > FRAME_LIMIT {
                        return Err(invalid("invalid row frame length"));
                    }
                    let remaining = file
                        .bytes
                        .checked_sub(reader.stream_position()?)
                        .ok_or_else(|| invalid("row file overflow"))?;
                    if len as u64 > remaining {
                        return Err(invalid("truncated row frame"));
                    }
                    let mut frame = vec![0; len];
                    reader.read_exact(&mut frame)?;
                    let (key, payload): (serde_bytes::ByteBuf, serde_bytes::ByteBuf) =
                        decode(&frame)?;
                    if previous
                        .as_ref()
                        .is_some_and(|last| last.as_slice() >= key.as_ref())
                    {
                        return Err(invalid("duplicate or unordered key"));
                    }
                    for id in referenced(&payload)?.keys() {
                        if !bindings.contains_key(id) {
                            return Err(invalid("missing row binding"));
                        }
                    }
                    previous = Some(key.into_vec());
                    file_count = file_count
                        .checked_add(1)
                        .ok_or_else(|| invalid("row overflow"))?;
                }
                if file_count != file.rows {
                    return Err(invalid("file row count mismatch"));
                }
                count = count
                    .checked_add(file_count)
                    .ok_or_else(|| invalid("table count overflow"))?;
                reader.seek(SeekFrom::Start(0))?;
                let (size, hash) = hash_reader(&mut reader)?;
                if size != file.bytes || hash != file.sha256 {
                    return Err(invalid("row file changed"));
                }
            }
        }
        if count != table.live_records {
            return Err(invalid("table row count mismatch"));
        }
    }
    Ok(hash)
}

fn validate_schema(schema: &QueryValue) -> Result<(), SeedError> {
    if matches!(schema, QueryValue::Null) {
        return Ok(());
    }
    let QueryValue::List(rules) = schema else {
        return Err(invalid("invalid schema"));
    };
    for rule in rules {
        for dependency in [
            "foreign_key",
            "references",
            "default",
            "transform",
            "scalar",
            "auto_now",
            "auto_now_add",
        ] {
            if rule.get(dependency).is_some() {
                return Err(invalid("unsupported schema dependency"));
            }
        }
        let path = rule
            .get("path")
            .and_then(QueryValue::as_array)
            .ok_or_else(|| invalid("schema path missing"))?;
        if path.is_empty()
            || path
                .iter()
                .any(|name| name.as_str().is_none_or(str::is_empty))
        {
            return Err(invalid("schema path is not names"));
        }
    }
    Ok(())
}

fn reject_link(metadata: &fs::Metadata) -> Result<(), SeedError> {
    if metadata.file_type().is_symlink() {
        return Err(invalid("symlink refused"));
    }
    #[cfg(windows)]
    {
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(invalid("reparse point refused"));
        }
    }
    Ok(())
}

fn open_regular(path: &Path) -> Result<File, SeedError> {
    let metadata = fs::symlink_metadata(path)?;
    reject_link(&metadata)?;
    if !metadata.is_file() {
        return Err(invalid("not a regular file"));
    }
    Ok(File::open(path)?)
}

fn read_bounded(path: &Path, limit: u64) -> Result<Vec<u8>, SeedError> {
    let file = open_regular(path)?;
    if file.metadata()?.len() > limit {
        return Err(invalid("metadata exceeds bounds"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(invalid("metadata exceeds bounds"));
    }
    Ok(bytes)
}

fn inventory(
    root: &Path,
    relative: &str,
    files: &mut BTreeSet<String>,
    directories: &mut BTreeSet<String>,
    depth: usize,
) -> Result<(), SeedError> {
    if depth > 2 {
        return Err(invalid("unexpected directory depth"));
    }
    for entry in fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        reject_link(&metadata)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("non UTF-8 path"))?;
        let path = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        if metadata.is_dir() {
            directories.insert(path.clone());
            inventory(root, &path, files, directories, depth + 1)?;
        } else if metadata.is_file() {
            files.insert(path);
        } else {
            return Err(invalid("special file refused"));
        }
        if files.len() + directories.len() > 1_000_000 {
            return Err(invalid("inventory entry budget exceeded"));
        }
    }
    Ok(())
}
