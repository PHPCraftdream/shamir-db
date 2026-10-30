use bytes::Bytes;
use futures::StreamExt;
use shamir_collections::{TFxMap, TFxSet};
use shamir_storage::error::DbResult;
use shamir_tunables::store_defaults::HISTORY_SCAN_BATCH;

use super::super::{ts_key, MvccStore};
use super::version_entry::VersionEntry;
use crate::version_codec::{decode_version_key, encode_version_key};

/// Read timelines in input order, including duplicate keys. Each sorted,
/// bounded-size group uses one inclusive archive range; gaps between keys
/// can still contain arbitrarily many unrelated records. On a scan error,
/// retry each original key sequentially to select the first per-key error.
pub(super) async fn history_of_many(
    mvcc: &MvccStore,
    keys: &[&[u8]],
) -> DbResult<Vec<Vec<VersionEntry>>> {
    let mut overlay: TFxMap<&[u8], Vec<(u64, Bytes)>> = TFxMap::default();
    for &key in keys {
        overlay
            .entry(key)
            .or_insert_with(|| mvcc.overlay.versions_of(key));
    }
    let mut groups: TFxMap<&[u8], Vec<(u64, Bytes)>> =
        overlay.keys().map(|&key| (key, Vec::new())).collect();
    let mut sorted: Vec<&[u8]> = groups.keys().copied().collect();
    sorted.sort_by_key(|key| encode_version_key(key, 0));
    for chunk in sorted.chunks(HISTORY_SCAN_BATCH) {
        let chunk_keys: TFxSet<&[u8]> = chunk.iter().copied().collect();
        let lo = encode_version_key(chunk[0], 0);
        let hi = chunk
            .iter()
            .map(|key| encode_version_key(key, u64::MAX))
            .max()
            .expect("nonempty chunk");
        let stream = mvcc
            .history
            .iter_range_stream(Some(lo), Some(hi), HISTORY_SCAN_BATCH);
        futures::pin_mut!(stream);
        while let Some(batch) = stream.next().await {
            let batch = match batch {
                Ok(batch) => batch,
                Err(_) => {
                    let mut timelines = Vec::with_capacity(keys.len());
                    for key in keys {
                        timelines.push(mvcc.history_of(key).await?);
                    }
                    return Ok(timelines);
                }
            };
            for (physical, value) in batch {
                if let Some((original, version)) = decode_version_key(&physical) {
                    if chunk_keys.contains(original) {
                        if let Some(entries) = groups.get_mut(original) {
                            entries.push((version, value));
                        }
                    }
                }
            }
        }
    }
    for (key, entries) in overlay {
        groups
            .get_mut(key)
            .expect("overlay key is indexed")
            .extend(entries);
    }
    // Archive entries precede overlay entries in history_of's stable sort.
    // Both copies have the same value, including tombstones.
    for entries in groups.values_mut() {
        entries.sort_by_key(|(version, _)| *version);
        entries.dedup_by_key(|(version, _)| *version);
    }
    let mut versions: Vec<u64> = groups
        .values()
        .flat_map(|entries| entries.iter().map(|(v, _)| *v))
        .collect();
    versions.sort_unstable();
    versions.dedup();
    let mut stamps: TFxMap<u64, Option<u64>> = versions
        .iter()
        .map(|&v| (v, mvcc.pending_ts.get_sync(&v).map(|entry| *entry.get())))
        .collect();
    let misses: Vec<u64> = versions
        .into_iter()
        .filter(|v| stamps[v].is_none())
        .collect();
    for chunk in misses.chunks(HISTORY_SCAN_BATCH) {
        let ts_keys = chunk.iter().map(|&v| ts_key(v).into()).collect();
        match mvcc.history.get_many(ts_keys).await {
            Ok(values) => {
                for (&version, value) in chunk.iter().zip(values) {
                    let ts = value.and_then(|value| {
                        let bytes: [u8; 8] = value.as_ref().try_into().ok()?;
                        Some(u64::from_le_bytes(bytes))
                    });
                    stamps.insert(version, ts);
                }
            }
            Err(error) => {
                log::warn!("batch timestamp lookup failed: {error}; retrying individually");
                for &version in chunk {
                    stamps.insert(version, mvcc.lookup_ts(version).await);
                }
            }
        }
    }
    Ok(keys
        .iter()
        .map(|key| {
            groups[key]
                .iter()
                .map(|(version, value)| VersionEntry {
                    version: *version,
                    value: value.clone(),
                    ts_millis: stamps[version],
                })
                .collect()
        })
        .collect())
}
