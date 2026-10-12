use std::fs::File;
use std::io::Read;
use std::path::Path;

use shamir_db::engine::repo::SeedImageWriter;

use crate::replication_seed::codec::{decode, FRAME_LIMIT, METADATA_LIMIT};
use crate::replication_seed::error::invalid;
use crate::replication_seed::{Manifest, SeedError};

use super::bounded::read_bounded;

/// One frame per blocking task; no table-sized allocation. The static artifact
/// is verified before mutation and reverified before completion.
pub(super) async fn import(
    root: &Path,
    manifest: &Manifest,
    writer: &mut SeedImageWriter,
    #[cfg(test)] boundary: &impl Fn(u8) -> Result<(), SeedError>,
    #[cfg(test)] image: &shamir_db::engine::repo::RepoInstance,
) -> Result<(), SeedError> {
    for file in manifest
        .files
        .iter()
        .filter(|file| file.table.is_none() && file.path != "schema.mpack")
    {
        let path = root.join(&file.path);
        let entries: Vec<(u64, String)> = tokio::task::spawn_blocking(move || {
            let bytes = read_bounded(&path, METADATA_LIMIT)?;
            decode(&bytes)
        })
        .await
        .map_err(|e| invalid(e.to_string()))??;
        for (id, name) in entries {
            writer.binding(id, &name).await?;
        }
    }
    for file in manifest.files.iter().filter(|file| file.table.is_some()) {
        let path = root.join(&file.path);
        let mut reader = tokio::task::spawn_blocking(move || File::open(path))
            .await
            .map_err(|e| invalid(e.to_string()))??;
        for _ in 0..file.rows {
            let (returned, frame) = tokio::task::spawn_blocking(move || {
                let result = read_frame(&mut reader);
                (reader, result)
            })
            .await
            .map_err(|e| invalid(e.to_string()))?;
            reader = returned;
            let (key, payload) = frame?;
            let table = file.table.ok_or_else(|| invalid("missing table ordinal"))?;
            writer
                .row(table as usize, key.into(), payload.into())
                .await?;
            #[cfg(test)]
            {
                image.flush_all_history().await?;
                boundary(6)?;
            }
        }
    }
    Ok(())
}

fn read_frame(reader: &mut File) -> Result<(Vec<u8>, Vec<u8>), SeedError> {
    let mut prefix = [0; 4];
    reader.read_exact(&mut prefix)?;
    let len = u32::from_be_bytes(prefix) as usize;
    if len == 0 || len > FRAME_LIMIT {
        return Err(invalid("invalid seed frame length"));
    }
    let mut bytes = vec![0; len];
    reader.read_exact(&mut bytes)?;
    let (key, payload): (serde_bytes::ByteBuf, serde_bytes::ByteBuf) = decode(&bytes)?;
    Ok((key.into_vec(), payload.into_vec()))
}
