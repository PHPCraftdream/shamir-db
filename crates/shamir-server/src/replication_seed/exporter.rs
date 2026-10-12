use std::fs;
use std::path::Path;

use futures_util::StreamExt;
use serde::Serialize;
use sha2::Digest;
use shamir_db::engine::meta::repo_identity::RepoIdentity;
use shamir_db::engine::repo::prepare_seed::prepare_seed;
use shamir_db::ShamirDb;
use shamir_types::core::interner::InternerKey;
use shamir_types::types::value::QueryValue;

use super::artifact_writer::{publish_manifest, sync_directory, ArtifactWriter, FILE_LIMIT};
use super::codec::{digest, referenced, CHUNK_LIMIT, METADATA_LIMIT};
use super::encode_bounded::encode_bounded;
use super::error::invalid;
use super::maintenance_open::MaintenanceOpen;
use super::schema_as_names::schema_as_names;
use super::schema_digest::schema_digest;
use super::{Manifest, SeedError, TableRecord};

pub async fn export(
    data_dir: &Path,
    db: &str,
    repo: &str,
    out: &Path,
) -> Result<String, SeedError> {
    // Explicit durability contract: do not silently claim directory fsync on
    // platforms where the standard library cannot provide it.
    if !cfg!(unix) {
        return Err(invalid(
            "durable seed publication is supported only on Unix",
        ));
    }
    let source = MaintenanceOpen::open(data_dir).await?;
    export_open(&source.db, db, repo, out, CHUNK_LIMIT).await
}

/// Only used under exclusive offline ownership (or isolated unit fixtures).
pub(crate) async fn export_open(
    source: &ShamirDb,
    db_name: &str,
    repo_name: &str,
    out: &Path,
    chunk_limit: u64,
) -> Result<String, SeedError> {
    export_open_with_index_limit(source, db_name, repo_name, out, chunk_limit, METADATA_LIMIT).await
}

/// Shared export path; tests may lower (never raise) the descriptor-set budget.
/// Inventory includes each descriptor's full Serialize shape, respecting its
/// existing serde skips for transient fields. No stable-identity subset is used.
pub(crate) async fn export_open_with_index_limit(
    source: &ShamirDb,
    db_name: &str,
    repo_name: &str,
    out: &Path,
    chunk_limit: u64,
    index_metadata_limit: u64,
) -> Result<String, SeedError> {
    if db_name == "system" || db_name == "__system__" {
        return Err(invalid("system repository seeds are unsupported"));
    }
    if chunk_limit == 0 || chunk_limit > CHUNK_LIMIT {
        return Err(invalid("invalid row chunk limit"));
    }
    if out.try_exists()? {
        return Err(invalid("output already exists"));
    }
    source.check_seed_catalogue().await?;
    let db = source
        .get_db(db_name)
        .ok_or_else(|| invalid("unknown database"))?;
    let repo = db
        .get_repo(repo_name)
        .ok_or_else(|| invalid("unknown repository"))?;
    let version = prepare_seed(&repo).await?;
    source.flush_seed_catalogue().await?;
    let identity = repo.repo_identity().await?;
    let gap = repo.certified_gap_interval().await?;
    let manager = repo.repo_interner().await?;
    let interner = manager.get().await?;
    let generation = interner.generation();
    let mut names = repo.list_table_names();
    names.sort();
    if let Some(catalogue) = source
        .system_store()
        .load_repository_record(db_name, repo_name)
        .await?
    {
        if catalogue.get("engine").and_then(QueryValue::as_str) == Some("hybrid") {
            return Err(invalid("hybrid repositories are unsupported seed sources"));
        }
    }
    let artifact_id = RepoIdentity::new_lineage_id()?;
    let parent = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let staging = parent.join(format!(".seed-{}", hex::encode(artifact_id)));
    fs::create_dir(&staging)?;
    let mut writer = ArtifactWriter::new(staging.clone());
    let mut tables = Vec::new();
    if names.len() > FILE_LIMIT {
        return Err(invalid("table inventory budget exceeded"));
    }
    let mut table_bytes = 5u64;
    let mut catalogue_bytes = 0u64;
    let mut catalogue_before = Vec::new();
    for (ordinal, name) in names.iter().enumerate() {
        let id = u32::try_from(ordinal).map_err(|e| invalid(e.to_string()))?;
        let table = repo.get_table(name).await?;
        let catalogue = source
            .system_store()
            .load_table_record(db_name, repo_name, name)
            .await?
            .ok_or_else(|| invalid("table missing from catalogue"))?;
        let catalogue_encoded = encode_bounded(
            &catalogue,
            usize::try_from(METADATA_LIMIT - catalogue_bytes)
                .map_err(|e| invalid(e.to_string()))?,
        )?;
        catalogue_bytes = catalogue_bytes
            .checked_add(catalogue_encoded.len() as u64)
            .ok_or_else(|| invalid("catalogue metadata overflow"))?;
        if catalogue_bytes > METADATA_LIMIT {
            return Err(invalid("catalogue metadata budget exceeded"));
        }
        // Retain only a fixed-width SHA-256 digest per table, never its bytes.
        catalogue_before.push(sha2::Sha256::digest(&catalogue_encoded));
        let schema = catalogue.get("schema").unwrap_or(&QueryValue::Null);
        let schema = schema_as_names(schema, interner)?;
        // Only the auto-bound declarative validator is portable. Code validators
        // and FK/scalar dependencies require an installer policy outside R2-lite.
        let schema_id = catalogue
            .get("schema_validator_id")
            .and_then(QueryValue::as_str);
        for binding in table.validator_bindings().iter() {
            if schema_id != Some(binding.validator_id.to_string().as_str()) {
                return Err(invalid("unsupported code validator binding"));
            }
        }
        if !table.collect_fk_refs().is_empty() || !table.schema_transforms().is_empty() {
            return Err(invalid("unsupported schema dependency"));
        }
        let mut excluded_indexes = Vec::new();
        let mut index_bytes = 5u64;
        let index_limit = METADATA_LIMIT
            .checked_sub(table_bytes)
            .ok_or_else(|| invalid("table metadata budget exceeded"))?
            .min(index_metadata_limit);
        for definition in table.index_manager_ref().iter_indexes() {
            push_index(
                &mut excluded_indexes,
                &mut index_bytes,
                index_limit,
                "regular",
                &definition,
            )?;
        }
        for definition in table.index_manager_ref().iter_unique_indexes() {
            push_index(
                &mut excluded_indexes,
                &mut index_bytes,
                index_limit,
                "unique",
                &definition,
            )?;
        }
        // These inventories clone descriptors already resident in the open
        // registries. Bound their full Serialize representations by the same
        // metadata budget as regular/unique indexes; never copy index data.
        for definition in table.sorted_indexes().iter_indexes() {
            push_index(
                &mut excluded_indexes,
                &mut index_bytes,
                index_limit,
                "sorted",
                &definition,
            )?;
        }
        for definition in table.index2_registry().all_descriptors().await {
            push_index(
                &mut excluded_indexes,
                &mut index_bytes,
                index_limit,
                "index2",
                &definition,
            )?;
        }
        excluded_indexes.sort();
        let directory = format!("tables/{id:06}");
        fs::create_dir_all(staging.join(&directory))?;
        let mvcc = table
            .mvcc_store()
            .ok_or_else(|| invalid("seed requires MVCC"))?;
        let stream = mvcc.snapshot_stream(1, version);
        futures_util::pin_mut!(stream);
        let mut chunk = Vec::new();
        let mut chunk_rows = 0u64;
        let mut chunk_id = 0u64;
        let mut count = 0u64;
        let mut previous: Option<Vec<u8>> = None;
        while let Some(batch) = stream.next().await {
            for (key, payload) in batch? {
                if previous
                    .as_ref()
                    .is_some_and(|last| last.as_slice() >= key.as_ref())
                {
                    return Err(invalid("duplicate or unordered record key"));
                }
                let ids = referenced(&payload)?;
                for id in ids.keys() {
                    let name = interner
                        .get_str(&InternerKey::new(*id))
                        .ok_or_else(|| invalid("missing row binding"))?;
                    if interner.get_ind(name.as_ref()).map(|key| key.id()) != Some(*id) {
                        return Err(invalid("conflicting row binding"));
                    }
                }
                let frame = encode_bounded(
                    &(
                        serde_bytes::Bytes::new(&key),
                        serde_bytes::Bytes::new(&payload),
                    ),
                    super::codec::FRAME_LIMIT,
                )?;
                if frame.len() > super::codec::FRAME_LIMIT {
                    return Err(invalid("row frame exceeds 16 MiB"));
                }
                let frame_bytes = frame.len() as u64 + 4;
                if frame_bytes > chunk_limit {
                    return Err(invalid("row exceeds chunk budget"));
                }
                if !chunk.is_empty() && chunk.len() as u64 + frame_bytes > chunk_limit {
                    writer.file(
                        format!("{directory}/rows-{chunk_id:06}.mpack"),
                        &chunk,
                        Some(id),
                        chunk_rows,
                    )?;
                    chunk.clear();
                    chunk_rows = 0;
                    chunk_id = chunk_id
                        .checked_add(1)
                        .ok_or_else(|| invalid("chunk overflow"))?;
                }
                let len = u32::try_from(frame.len()).map_err(|e| invalid(e.to_string()))?;
                chunk.extend_from_slice(&len.to_be_bytes());
                chunk.extend_from_slice(&frame);
                chunk_rows += 1;
                count = count
                    .checked_add(1)
                    .ok_or_else(|| invalid("row count overflow"))?;
                previous = Some(key.to_vec());
            }
        }
        if !chunk.is_empty() {
            writer.file(
                format!("{directory}/rows-{chunk_id:06}.mpack"),
                &chunk,
                Some(id),
                chunk_rows,
            )?;
        }
        sync_directory(&staging.join(&directory))?;
        let table_record = TableRecord {
            name: name.clone(),
            source_enable_indexes: catalogue
                .get("enable_indexes")
                .and_then(QueryValue::as_bool)
                .unwrap_or(false),
            follower_enable_indexes: false,
            schema_version: catalogue
                .get("schema_version")
                .and_then(QueryValue::as_i64)
                .map(u64::try_from)
                .transpose()
                .map_err(|e| invalid(e.to_string()))?
                .unwrap_or(0),
            schema,
            excluded_indexes,
            live_records: count,
        };
        let serialized = encode_bounded(&table_record, remaining_metadata(table_bytes)?)?;
        table_bytes = table_bytes
            .checked_add(serialized.len() as u64)
            .ok_or_else(|| invalid("table metadata overflow"))?;
        tables.push(table_record);
    }
    let schema_bytes = encode_bounded(&tables, remaining_metadata(0)?)?;
    writer.file("schema.mpack".into(), &schema_bytes, None, 0)?;
    // Paginate the source interner, including all referenced bindings without
    // a repository-sized collection. Empty interner still emits one page.
    let reverse = interner.reverse_snapshot();
    let mut start = 0usize;
    let mut page = 0u64;
    let mut metadata_bytes = schema_bytes.len() as u64;
    loop {
        let end = start
            .saturating_add(256)
            .min(reverse.len().saturating_sub(1));
        let entries: Vec<(u64, String)> = interner
            .entries_in_id_range(start, end)
            .into_iter()
            .map(|(key, name)| (key.id(), name.as_str().to_string()))
            .collect();
        let bytes = encode_bounded(
            &entries,
            remaining_metadata(metadata_bytes)?.min(1024 * 1024),
        )?;
        metadata_bytes = metadata_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| invalid("metadata overflow"))?;
        if metadata_bytes > METADATA_LIMIT {
            return Err(invalid("metadata budget exceeded"));
        }
        writer.file(format!("bindings-{page:06}.mpack"), &bytes, None, 0)?;
        if end >= reverse.len().saturating_sub(1) {
            break;
        }
        start = end;
        page = page
            .checked_add(1)
            .ok_or_else(|| invalid("binding page overflow"))?;
    }
    for (name, before) in names.iter().zip(catalogue_before) {
        let after = source
            .system_store()
            .load_table_record(db_name, repo_name, name)
            .await?
            .ok_or_else(|| invalid("table disappeared during export"))?;
        let after_bytes = encode_bounded(&after, remaining_metadata(0)?)?;
        if sha2::Sha256::digest(&after_bytes) != before {
            return Err(invalid("schema changed during export"));
        }
    }
    let mut after_names = repo.list_table_names();
    after_names.sort();
    if after_names != names
        || repo.consistent_version().await? != version
        || repo.tx_gate().await?.version_allocation_high_water_mark() != version
        || interner.generation() != generation
        || repo.repo_identity().await? != identity
        || repo.certified_gap_interval().await? != gap
    {
        return Err(invalid("source changed during export"));
    }
    let manifest = Manifest {
        format_version: 1,
        codec_version: 1,
        requirements: vec![
            "mvcc".into(),
            "raw-inner-value".into(),
            "schema-names".into(),
        ],
        source_db: db_name.into(),
        source_repo: repo_name.into(),
        lineage_id: identity.lineage_id,
        history_epoch: identity.history_epoch,
        version,
        certified_gap: gap,
        schema_sha256: schema_digest(&tables)?,
        artifact_id,
        index_policy: "EXCLUDED".into(),
        tables,
        files: writer.files,
    };
    let bytes = encode_bounded(&manifest, remaining_metadata(metadata_bytes)?)?;
    let hash = digest(&bytes);
    publish_manifest(&staging, &bytes, &hash, || Ok(()))?;
    if super::verify(&staging)? != hash {
        return Err(invalid("final artifact verification mismatch"));
    }
    if staging.join("tables").exists() {
        sync_directory(&staging.join("tables"))?;
    }
    sync_directory(&staging)?;
    // Reserve the destination atomically. On Unix rename replaces only this
    // empty reservation, never an existing seed or user directory. A crash in
    // this window leaves an empty directory that verify refuses.
    fs::create_dir(out)?;
    fs::rename(&staging, out)?;
    sync_directory(parent)?;
    Ok(hash)
}

fn remaining_metadata(used: u64) -> Result<usize, SeedError> {
    let remaining = METADATA_LIMIT
        .checked_sub(used)
        .ok_or_else(|| invalid("metadata budget exceeded"))?;
    usize::try_from(remaining).map_err(|e| invalid(e.to_string()))
}

pub(crate) fn push_index<T: Serialize>(
    entries: &mut Vec<(String, Vec<u8>)>,
    used: &mut u64,
    limit: u64,
    family: &str,
    definition: &T,
) -> Result<(), SeedError> {
    let remaining = limit
        .checked_sub(*used)
        .ok_or_else(|| invalid("index metadata budget exceeded"))?;
    let bound = usize::try_from(remaining).map_err(|e| invalid(e.to_string()))?;
    let bytes = encode_bounded(definition, bound)?;
    let entry = (family.to_string(), bytes);
    // Include the family and tuple/byte-vector encoding overhead, not merely
    // the serialized descriptor payload. Refuse before retaining the entry.
    let encoded = encode_bounded(&entry, bound)?;
    let total = used
        .checked_add(encoded.len() as u64)
        .ok_or_else(|| invalid("index metadata overflow"))?;
    *used = total;
    entries.push(entry);
    Ok(())
}
