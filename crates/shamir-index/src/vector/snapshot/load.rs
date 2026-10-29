//! Load path (manifest → chunks → sidecar → `HnswIo`) and the V2.3
//! delta-log (append / replay / generation flip) of the snapshot codec.

use crate::meta_envelope::MetaEnvelope;
use crate::vector::adapter::VectorAdapter;
use crate::vector::hnsw_adapter::{HnswAdapter, ShamirDist};
use crate::vector::quantized_dist::ShamirDistU8;
use crate::vector::sq8::Sq8Quantizer;
use bytes::Bytes;
use futures::StreamExt;
use hnsw_rs::hnsw::Hnsw;
use hnsw_rs::hnswio::HnswIo;
use serde::{Deserialize, Serialize};
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;
use std::io::Write;
use std::sync::Arc;

use super::codec::{
    chunk_key, manifest_key, map_meta_err, sidecar_key, ChunkHeader, HNSW_RS_VERSION,
    QUANT_METHOD_SQ8, SNAPSHOT_SUPPORTED_VERSIONS,
};
use super::quantized_load::load_quantized_graph;
use super::{OldGenerationChunks, QuantMeta, SnapshotError, SnapshotManifest, SnapshotSidecar};

// ============================================================================
// Load path
// ============================================================================

/// Read + decode the manifest for `keyspace`. Returns `NotFound` when no
/// snapshot exists (the manifest key is absent). Used by the V2.3 delta-
/// replay path (`restore_on_open` reads the manifest to learn
/// `delta_applied_upto` BEFORE loading the snapshot) and by the background
/// snapshot flip (`run_background_snapshot` reads the current gen + chunk
/// counts to drive the prune).
///
/// V5.3 (#412): accepts both v1 and v2 manifests (see
/// [`SNAPSHOT_SUPPORTED_VERSIONS`]). A v3+ manifest is refused with
/// `VersionMismatch`.
pub async fn read_manifest(
    store: &Arc<dyn Store>,
    keyspace: &str,
) -> Result<SnapshotManifest, SnapshotError> {
    let manifest_bytes = store.get(manifest_key(keyspace)).await?;
    let manifest: SnapshotManifest = MetaEnvelope::open(&manifest_bytes).map_err(map_meta_err)?;
    if !SNAPSHOT_SUPPORTED_VERSIONS.contains(&manifest.format_version) {
        return Err(SnapshotError::VersionMismatch(format!(
            "snapshot format version {} not in supported set {:?}",
            manifest.format_version, SNAPSHOT_SUPPORTED_VERSIONS
        )));
    }
    Ok(manifest)
}

/// Load the snapshot stored under `keyspace` and rebuild a working
/// `HnswAdapter`. Reads manifest → chunks (with per-chunk crc verify) →
/// sidecar (with cross-section crc verify) → temp files → `HnswIo` →
/// `Box::leak` for loader lifetime → `Hnsw<'static>` → adapter.
///
/// V5.3 (#412): for a quantized v2 snapshot (`sidecar.quantization.is_some()`
/// AND `manifest.qgraph_chunks > 0`), this ALSO:
///  * decodes the `QuantMeta` from `sidecar.quantization` and reconstructs
///    the [`Sq8Quantizer`];
///  * fetches + verifies the `qgraph`/`qdata` chunks, writes them to temp
///    files, and reloads the u8 graph via `HnswIo::load_hnsw_with_dist` with
///    a fresh [`ShamirDistU8`] built from the reconstructed quantizer;
///  * assembles the adapter via [`HnswAdapter::from_parts_with_quantization`]
///    so `is_fitted == true`, the u8 graph + quantizer + `vectors_u8` are
///    all live, and post-load search goes the quantized path.
///
/// Accepts both v1 and v2 sidecars (back-compat). A v1 sidecar has
/// `quantization == None` and no qgraph sections → the load is identical to
/// the pre-#412 path.
pub async fn load_snapshot(
    store: &Arc<dyn Store>,
    keyspace: &str,
) -> Result<HnswAdapter, SnapshotError> {
    load_snapshot_with_intent(store, keyspace, None).await
}

/// Load a snapshot while preserving the descriptor's quantization intent.
pub(crate) async fn load_snapshot_with_intent(
    store: &Arc<dyn Store>,
    keyspace: &str,
    quantization: Option<crate::kind::VectorQuantization>,
) -> Result<HnswAdapter, SnapshotError> {
    // ---- 1. manifest -----------------------------------------------------
    let manifest = read_manifest(store, keyspace).await?;
    let gen = manifest.gen;

    // ---- 2. sidecar ------------------------------------------------------
    let sidecar_bytes = store.get(sidecar_key(keyspace, gen)).await?;
    let sidecar: SnapshotSidecar = MetaEnvelope::open(&sidecar_bytes).map_err(map_meta_err)?;
    if !SNAPSHOT_SUPPORTED_VERSIONS.contains(&sidecar.format_version) {
        return Err(SnapshotError::VersionMismatch(format!(
            "sidecar format version {} not in supported set {:?}",
            sidecar.format_version, SNAPSHOT_SUPPORTED_VERSIONS
        )));
    }

    // ---- 3. fetch + verify + reassemble both f32 files -------------------
    //
    // The chunk keys for a given (gen, section) are sequential 0..N — we
    // build them from the manifest's chunk count, fetch them via `get_many`
    // (a single vectored read on backends that override it), and verify each
    // crc before stitching. We do NOT use `scan_prefix_stream` here because
    // that would also pick up chunks of OTHER generations sharing the
    // keyspace (gen-flip #402); the manifest is the single source of truth
    // for which chunks belong to the active gen.
    let mut graph_chunk_keys: Vec<RecordKey> = Vec::with_capacity(manifest.graph_chunks as usize);
    for i in 0..manifest.graph_chunks as usize {
        graph_chunk_keys.push(chunk_key(keyspace, gen, "graph", i));
    }
    let mut data_chunk_keys: Vec<RecordKey> = Vec::with_capacity(manifest.data_chunks as usize);
    for i in 0..manifest.data_chunks as usize {
        data_chunk_keys.push(chunk_key(keyspace, gen, "data", i));
    }

    let graph_vals = store.get_many(graph_chunk_keys).await?;
    let data_vals = store.get_many(data_chunk_keys).await?;

    let graph_bytes = reassemble_and_verify(&graph_vals, "graph")?;
    let data_bytes = reassemble_and_verify(&data_vals, "data")?;

    // Cross-section crc32 verify (independent of the per-chunk checks).
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&graph_bytes);
    hasher.update(&data_bytes);
    if graph_bytes.len() as u64 != sidecar.graph_len {
        return Err(SnapshotError::Corrupt(format!(
            "graph length mismatch: expected {}, got {}",
            sidecar.graph_len,
            graph_bytes.len()
        )));
    }
    if data_bytes.len() as u64 != sidecar.data_len {
        return Err(SnapshotError::Corrupt(format!(
            "data length mismatch: expected {}, got {}",
            sidecar.data_len,
            data_bytes.len()
        )));
    }

    // ---- 3b. V5.3 (#412): fetch + verify the u8 graph sections -----------
    //
    // For a quantized snapshot the qgraph/qdata chunks are present. We
    // reassemble + crc them here, then extend the cross-section crc32 to
    // cover them (the dump path hashed all four sections together).
    let quant_meta: Option<QuantMeta> = match &sidecar.quantization {
        Some(blob) => {
            let meta: QuantMeta = bincode::deserialize(blob)?;
            // Refuse an unknown method — we only know "sq8" today.
            if meta.method != QUANT_METHOD_SQ8 {
                return Err(SnapshotError::VersionMismatch(format!(
                    "unknown quantization method {} (supported: {})",
                    meta.method, QUANT_METHOD_SQ8
                )));
            }
            Some(meta)
        }
        None => None,
    };

    let (qgraph_bytes, qdata_bytes) = if manifest.qgraph_chunks > 0 {
        let mut qgraph_keys: Vec<RecordKey> = Vec::with_capacity(manifest.qgraph_chunks as usize);
        for i in 0..manifest.qgraph_chunks as usize {
            qgraph_keys.push(chunk_key(keyspace, gen, "qgraph", i));
        }
        let mut qdata_keys: Vec<RecordKey> = Vec::with_capacity(manifest.qdata_chunks as usize);
        for i in 0..manifest.qdata_chunks as usize {
            qdata_keys.push(chunk_key(keyspace, gen, "qdata", i));
        }
        let qg_vals = store.get_many(qgraph_keys).await?;
        let qd_vals = store.get_many(qdata_keys).await?;
        let qg = reassemble_and_verify(&qg_vals, "qgraph")?;
        let qd = reassemble_and_verify(&qd_vals, "qdata")?;
        (qg, qd)
    } else {
        (Vec::new(), Vec::new())
    };

    // Extend the cross-section crc32 over the u8 sections (the dump path
    // hashed all four sections together; a non-quant snapshot hashes only
    // the f32 pair, matching the pre-#412 behaviour).
    hasher.update(&qgraph_bytes);
    hasher.update(&qdata_bytes);
    if hasher.finalize() != sidecar.sections_crc32 {
        return Err(SnapshotError::Corrupt(
            "cross-section crc32 mismatch".to_string(),
        ));
    }

    // ---- 4. write temp files, Box::leak HnswIo, load f32 graph (CPU/IO) --
    //
    // #418 — for a quantized snapshot (`quant_meta.is_some()`) the dump path
    // emitted ZERO-length f32 sections (the fitted adapter had dropped its
    // f32 graph post-fit). We skip the f32 file load entirely and substitute
    // a throwaway empty `Hnsw::new(...)` stub: `from_parts_with_quantization`
    // drops it immediately on assembly, so its contents are irrelevant — it
    // exists only to satisfy the function's `Arc<Hnsw<f32>>` parameter shape.
    // A non-quant snapshot always has real f32 bytes and loads as before.
    let load_dir = tempfile::tempdir()?;
    let basename = manifest.basename.clone();
    let dist = ShamirDist {
        metric: sidecar.metric,
    };
    let dim = sidecar.dim;
    let metric = sidecar.metric;
    let ef_search = sidecar.ef_search;
    let m = sidecar.m;
    let max_layer = sidecar.max_layer;
    let ef_construction = sidecar.ef_construction;
    let hnsw_rs_version = sidecar.hnsw_rs_version.clone();
    let next_id = sidecar.next_id;
    let rid_map_pairs = sidecar.rid_map.clone();
    let rid_to_internal_pairs = sidecar.rid_to_internal.clone();
    let tombstones = sidecar.tombstones.clone();
    let vectors_pairs = sidecar.vectors.clone();
    let vectors_u8_pairs = sidecar.vectors_u8.clone();

    // Verify the dump came from a compatible `hnsw_rs` BEFORE we hand it to
    // the loader — `load_hnsw_with_dist` will panic on an unknown format.
    // `hnsw_rs` does not export its version as a const, so we compare the
    // sidecar-stamped version against our build-time pin.
    if hnsw_rs_version != HNSW_RS_VERSION {
        return Err(SnapshotError::VersionMismatch(format!(
            "dump produced by hnsw_rs {}, but this build pins {}",
            hnsw_rs_version, HNSW_RS_VERSION
        )));
    }

    let hnsw: Arc<Hnsw<'static, f32, ShamirDist>> = if quant_meta.is_some() {
        // #418 — quantized snapshot: no f32 graph on disk. Build a tiny empty
        // stub (capacity 1) — `from_parts_with_quantization` drops it on
        // assembly, so the size is irrelevant; we just need the right shape.
        Arc::new(Hnsw::new(m as usize, 1, max_layer, ef_construction, dist))
    } else {
        let graph_path = load_dir.path().join(format!("{basename}.hnsw.graph"));
        let data_path = load_dir.path().join(format!("{basename}.hnsw.data"));
        tokio::task::spawn_blocking(move || {
            // Write the reassembled bytes to the temp files. `file_dump` writes
            // both files with `create+truncate+write`, so we mirror that.
            let mut gf = std::fs::File::create(&graph_path)?;
            gf.write_all(&graph_bytes)?;
            gf.flush()?;
            let mut df = std::fs::File::create(&data_path)?;
            df.write_all(&data_bytes)?;
            df.flush()?;
            drop(gf);
            drop(df);

            // Boot-only, intentional leak: `load_hnsw_with_dist<'b,'a>` ties
            // `'a: 'b` where `'a` is the borrow of the `HnswIo` loader. To hand
            // the reloaded graph to long-lived storage as `Arc<Hnsw<'static,...>>`
            // (the shape HnswAdapter already uses), the loader must be `'static`.
            // We leak exactly ONE `HnswIo` per snapshot load; the loader is tiny
            // (~tens of bytes) and lives for the process. The dump files are the
            // durable artefact — the leak does NOT grow across snapshots, only
            // across shard count. Mirrors the V0.0 contract test
            // `leaked_loader_yields_static_hnsw`.
            //
            // MUST STAY NON-MMAP. `HnswIo::new` defaults to `datamap: false`, so
            // `load_hnsw_with_dist` reads the graph fully INTO MEMORY — the
            // returned `Hnsw` borrows the leaked `HnswIo` struct, NOT the
            // on-disk files. That is why `load_dir` can be dropped when the
            // closure returns: the graph is already resident. Enabling mmap
            // here would require keeping the files alive too.
            let leaked_io: &'static HnswIo =
                Box::leak(Box::new(HnswIo::new(load_dir.path(), &basename)));
            let hnsw: Hnsw<'static, f32, ShamirDist> =
                leaked_io.load_hnsw_with_dist(dist).map_err(|e| {
                    SnapshotError::Io(format!("hnsw_rs load_hnsw_with_dist failed: {e}"))
                })?;
            Ok::<_, SnapshotError>(Arc::new(hnsw))
        })
        .await
        .map_err(|e| SnapshotError::Io(format!("spawn_blocking join error: {e}")))??
    };

    // Sanity: the loaded graph retained every point the sidecar claims, and
    // the build params match what we persisted. (`m` and `max_layer` are
    // advisory — `get_*` reads them off the loaded structure.) Skipped for a
    // quant stub (#418): the stub is empty by design.
    if quant_meta.is_none() {
        debug_assert_build_params(&hnsw, m, max_layer, ef_construction);
    }

    // ---- 4b. V5.3 (#412): load the u8 graph (if quantized) ---------------
    //
    // Decode the QuantMeta → Sq8Quantizer → ShamirDistU8, write the qgraph/
    // qdata temp files, and reload the u8 graph via a second `HnswIo`. The
    // leaked HnswIo mirrors the f32 pattern (boot-only, one per shard).
    //
    // The carrier is a tuple `(u8 graph Arc, reconstructed quantizer Arc)` —
    // both are needed to assemble the fitted adapter in step 5. We allow the
    // `type_complexity` lint because the tuple is a local carrier, not a
    // public signature.
    #[allow(clippy::type_complexity)] // local carrier tuple — see comment above
    let hnsw_u8_opt: Option<(Arc<Hnsw<'static, u8, ShamirDistU8>>, Arc<Sq8Quantizer>)> =
        if let Some(meta) = &quant_meta {
            // Reconstruct the quantizer from the stored params.
            let quantizer = Arc::new(meta.to_quantizer());
            let qbasename = manifest.qbasename.clone();
            let qload_dir = tempfile::tempdir()?;
            let qgraph_bytes_move = qgraph_bytes.clone();
            let qdata_bytes_move = qdata_bytes.clone();
            let quantizer_for_load = Arc::clone(&quantizer);
            let hnsw_u8 = tokio::task::spawn_blocking(move || {
                load_quantized_graph(
                    qload_dir,
                    &qbasename,
                    &qgraph_bytes_move,
                    &qdata_bytes_move,
                    quantizer_for_load,
                    metric,
                )
            })
            .await
            .map_err(|e| SnapshotError::Io(format!("spawn_blocking join error: {e}")))??;
            Some((hnsw_u8, quantizer))
        } else {
            None
        };

    // ---- 5. rebuild the adapter maps -------------------------------------
    let cap = rid_map_pairs.len().max(64);
    let rid_map = scc::HashMap::with_capacity_and_hasher(cap, THasher::default());
    let rid_to_internal = scc::HashMap::with_capacity_and_hasher(cap, THasher::default());
    let vectors = scc::HashMap::with_capacity_and_hasher(cap, THasher::default());
    let deleted = scc::HashMap::with_capacity_and_hasher(cap, THasher::default());
    for (internal, rid) in rid_map_pairs {
        let _ = rid_map.insert_sync(internal, rid);
    }
    for (rid, internal) in rid_to_internal_pairs {
        let _ = rid_to_internal.insert_sync(rid, internal);
    }
    for (internal, v) in vectors_pairs {
        let _ = vectors.insert_sync(internal, v);
    }
    for internal in tombstones {
        let _ = deleted.insert_sync(internal, ());
    }

    // V5.3 (#412): if a u8 graph was loaded, assemble the fitted adapter.
    if let Some((hnsw_u8, quantizer)) = hnsw_u8_opt {
        // Rebuild the vectors_u8 map from the sidecar pairs.
        let vectors_u8 = scc::HashMap::with_capacity_and_hasher(cap, THasher::default());
        for (internal, codes) in vectors_u8_pairs {
            let _ = vectors_u8.insert_sync(internal, codes);
        }
        let adapter = HnswAdapter::from_parts_with_quantization(
            dim,
            metric,
            ef_search,
            hnsw,
            rid_map,
            rid_to_internal,
            vectors,
            deleted,
            next_id,
            hnsw_u8,
            quantizer,
            vectors_u8,
        );
        return Ok(adapter);
    }

    let adapter = HnswAdapter::from_parts(
        dim,
        metric,
        ef_search,
        hnsw,
        rid_map,
        rid_to_internal,
        vectors,
        deleted,
        next_id,
        quantization,
    );
    Ok(adapter)
}

/// Decode each chunk header, verify its crc32, and concatenate the raw bytes
/// back into the original file. Returns `Corrupt` on the first mismatch with
/// the chunk index in the message.
fn reassemble_and_verify(
    vals: &[Option<Bytes>],
    section: &'static str,
) -> Result<Vec<u8>, SnapshotError> {
    let mut out = Vec::new();
    for (i, opt) in vals.iter().enumerate() {
        let bytes = opt
            .clone()
            .ok_or_else(|| SnapshotError::Corrupt(format!("missing {section} chunk {i}")))?;
        let header: ChunkHeader = bincode::deserialize(&bytes)
            .map_err(|e| SnapshotError::Corrupt(format!("{section} chunk {i} decode: {e}")))?;
        if header.idx as usize != i {
            return Err(SnapshotError::Corrupt(format!(
                "{section} chunk idx mismatch: expected {i}, got {}",
                header.idx
            )));
        }
        let actual = crc32fast::hash(&header.bytes);
        if actual != header.crc32 {
            return Err(SnapshotError::Corrupt(format!(
                "{section} chunk {i} crc32 mismatch: expected {}, got {}",
                header.crc32, actual
            )));
        }
        out.extend_from_slice(&header.bytes);
    }
    Ok(out)
}

// ============================================================================
// Delta-log (V2.3 / #402)
// ============================================================================
//
// Between full snapshots, every Phase 5d promote appends a `DeltaOp` chunk to
// the info store. The chunk is a `Vec<DeltaOp>` bincode'd inside a
// `MetaEnvelope`, written under a monotonic zero-padded key
// `<keyspace>.delta.NNNNNNNNNN`. On restart, after `load_snapshot` rebuilds
// the base graph, the replay walks every chunk with index >
// `manifest.delta_applied_upto` and applies each `DeltaOp` to the freshly-
// loaded adapter. The HWM pattern mirrors `InternerManager` (last_chunk_idx
// in-memory + scan on boot); the atomic generation flip (below) prunes
// superseded chunks as part of the same `Store::transact` that publishes the
// new manifest.

/// Width of the decimal zero-padded delta-chunk index in the keyspace key.
/// 10 digits = 10^10 chunks. At one chunk per commit-Phase-5d (each carrying
/// up to a tx's worth of vector mutations) that is ~300 years of commits at
/// 1k commits/sec, well past any realistic working set; the zero-padding
/// keeps lexicographic key order == numeric order, so a prefix scan walks
/// chunks in append order without a comparator.
const DELTA_CHUNK_IDX_WIDTH: usize = 10;

/// One vector mutation captured by the delta-log. Phase 5d translates the
/// tx's `staged_vectors` slice into `Upsert` ops and (on a tx that deleted a
/// vector-backed row) emits `Delete`. P0 delta-prune (brief 164) — ordering
/// correction: the chunk is appended BEFORE the graph apply (the engine's
/// delta half runs pre-publish; the HNSW promote runs post-lock), so a chunk
/// may exist while its mutation has NOT yet reached any dumped graph. The
/// snapshot prune boundary is therefore the backend's contiguous APPLIED
/// prefix (`IndexBackend::mark_delta_applied`), never the allocation
/// counter: a snapshot that pruned by allocation order could drop a chunk
/// whose vector never made it into the dumped graph and lose it on restart.
/// A crash AFTER the chunk append but BEFORE the next snapshot sees the
/// mutation replayed on restart.
#[derive(Clone, Serialize, Deserialize)]
pub enum DeltaOp {
    /// Insert or replace `rid`'s vector.
    Upsert(RecordId, Vec<f32>),
    /// Soft-delete `rid`.
    Delete(RecordId),
}

/// Build the keyspace key for delta chunk `idx`. `<keyspace>.delta.NNNNNNNNNN`.
fn delta_chunk_key(keyspace: &str, idx: u64) -> RecordKey {
    Bytes::from(format!(
        "{ks}.delta.{idx:0width$}",
        ks = keyspace,
        idx = idx,
        width = DELTA_CHUNK_IDX_WIDTH,
    ))
    .into()
}

/// Prefix used by `scan_prefix_stream` to enumerate every delta chunk in a
/// keyspace (across all generations — the manifest's `delta_applied_upto`
/// filters which ones to apply). `<keyspace>.delta.`
fn delta_scan_prefix(keyspace: &str) -> RecordKey {
    Bytes::from(format!("{ks}.delta.", ks = keyspace)).into()
}

/// Append a delta chunk (`Vec<DeltaOp>`) to the info store under `keyspace`
/// at index `idx`. The caller (the snapshot coordinator inside
/// `VectorBackend`) owns the monotonic `idx` — typically the in-memory HWM
/// counter. The chunk is a single `Store::set` (one memtable insert on every
/// backend we ship): cheap enough to run synchronously inside commit Phase 5d
/// without stalling the ack (§5.6 — see the rationale on
/// `apply_staged_vectors`).
pub async fn append_delta(
    store: &Arc<dyn Store>,
    keyspace: &str,
    idx: u64,
    ops: &[DeltaOp],
) -> Result<(), SnapshotError> {
    if ops.is_empty() {
        // An empty promote (no vector rows in this tx) writes nothing. This
        // keeps the chunk stream dense — every chunk corresponds to a real
        // mutation — and lets the replay short-circuit on an absent chunk
        // without a "decode empty envelope" special case.
        return Ok(());
    }
    let env = MetaEnvelope::new(ops.to_vec());
    let bytes = env
        .encode()
        .map_err(|e| SnapshotError::Serde(e.to_string()))?;
    store
        .set(delta_chunk_key(keyspace, idx), Bytes::from(bytes))
        .await?;
    Ok(())
}

/// Decode a delta chunk's bytes into its `Vec<DeltaOp>`. Shared by the
/// restart-replay path and the tests.
fn decode_delta_chunk(bytes: &[u8]) -> Result<Vec<DeltaOp>, SnapshotError> {
    MetaEnvelope::<Vec<DeltaOp>>::open(bytes).map_err(map_meta_err)
}

/// Replay every delta chunk with index strictly greater than
/// `delta_applied_upto` against the live `adapter`. Used by
/// `VectorBackend::restore_on_open` after a successful `load_snapshot`: the
/// base graph reflects the snapshot's generation, and the delta chunks carry
/// every mutation committed since that snapshot was taken.
///
/// Walks the keyspace via `scan_prefix_stream` (lexicographic key order ==
/// numeric chunk order thanks to the zero-padding). Each chunk's ops are
/// applied via `VectorAdapter::upsert` / `delete` — the same path Phase 5d
/// uses, so the replayed graph is byte-for-byte equivalent to the live graph
/// that produced the chunks. A chunk with index ≤ `delta_applied_upto` is
/// skipped (it was already absorbed into the snapshot's graph).
pub async fn replay_delta(
    store: &Arc<dyn Store>,
    keyspace: &str,
    delta_applied_upto: u64,
    adapter: &HnswAdapter,
) -> Result<u64, SnapshotError> {
    use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;
    let prefix = delta_scan_prefix(keyspace);
    let mut stream = store.scan_prefix_stream(prefix.into(), MAINT_SCAN_BATCH);
    let mut highest_seen: u64 = delta_applied_upto;
    while let Some(batch_res) = stream.next().await {
        let batch: Vec<(RecordKey, Bytes)> = batch_res?;
        for (key, val) in batch {
            // Parse the trailing decimal index off the key. The key layout
            // is `<keyspace>.delta.NNNNNNNNNN`; the chunk index is the
            // suffix after the LAST `.`. A malformed key (no trailing
            // decimal) is skipped — it cannot have been written by
            // `append_delta`, so it is not ours.
            let key_bytes: &[u8] = key.as_ref();
            let key_str = std::str::from_utf8(key_bytes)
                .map_err(|e| SnapshotError::Corrupt(format!("delta key not utf8: {e}")))?;
            let idx_str = key_str.rsplit('.').next().ok_or_else(|| {
                SnapshotError::Corrupt(format!("delta key missing index: {key_str}"))
            })?;
            let idx: u64 = idx_str
                .parse()
                .map_err(|e| SnapshotError::Corrupt(format!("delta key index parse: {e}")))?;
            if idx < delta_applied_upto {
                // Already absorbed into the snapshot's base graph. We use
                // `<` (strictly-less-than) because `delta_applied_upto`
                // counts the number of absorbed chunks: a value of N means
                // chunks 0..N (indices 0..N-1) were absorbed, so chunk N is
                // the first to replay.
                continue;
            }
            let ops = decode_delta_chunk(&val)?;
            for op in ops {
                match op {
                    DeltaOp::Upsert(rid, vec) => {
                        adapter
                            .upsert(rid, &vec)
                            .await
                            .map_err(|e| SnapshotError::Backend(format!("delta upsert: {e}")))?;
                    }
                    DeltaOp::Delete(rid) => {
                        adapter
                            .delete(rid)
                            .await
                            .map_err(|e| SnapshotError::Backend(format!("delta delete: {e}")))?;
                    }
                }
            }
            if idx > highest_seen {
                highest_seen = idx;
            }
        }
    }
    Ok(highest_seen)
}

/// Atomically flip the manifest to a new generation, prune the superseded
/// generation's chunks, and prune every delta chunk with index ≤
/// `new_delta_applied_upto`. The whole flip is ONE `Store::transact` so a
/// backend that overrides `transact` exposes the new generation + the prune
/// as a single all-or-nothing batch.
///
/// Crash-safety: a crash between this call and a later one is benign. If the
/// flip landed but the prune of orphan chunks from a PREVIOUS flip did not
/// (the `transact` covers THIS flip's prune; an earlier flip's prune may
/// have been interrupted), the orphans sit harmlessly in the store — the
/// manifest points unambiguously at the new gen, and the next
/// `dump_snapshot_with_gen` run prunes them idempotently.
pub async fn flip_generation(
    store: &Arc<dyn Store>,
    keyspace: &str,
    old_gen: u32,
    old_chunks: OldGenerationChunks,
    new_manifest: SnapshotManifest,
    new_delta_applied_upto: u64,
) -> Result<(), SnapshotError> {
    let mut ops: Vec<KvOp> = Vec::new();

    // Prune the OLD generation's chunks + sidecar. The manifest's chunk
    // counts tell us exactly how many keys to remove — no scan needed.
    for i in 0..old_chunks.graph as usize {
        ops.push(KvOp::Remove(chunk_key(keyspace, old_gen, "graph", i)));
    }
    for i in 0..old_chunks.data as usize {
        ops.push(KvOp::Remove(chunk_key(keyspace, old_gen, "data", i)));
    }
    for i in 0..old_chunks.qgraph as usize {
        ops.push(KvOp::Remove(chunk_key(keyspace, old_gen, "qgraph", i)));
    }
    for i in 0..old_chunks.qdata as usize {
        ops.push(KvOp::Remove(chunk_key(keyspace, old_gen, "qdata", i)));
    }
    ops.push(KvOp::Remove(sidecar_key(keyspace, old_gen)));

    // Prune every delta chunk absorbed by the new snapshot (indices
    // `0..new_delta_applied_upto`, exclusive). `delta_applied_upto` counts
    // the absorbed chunks, so chunk `delta_applied_upto - 1` is the last one
    // absorbed; chunk `delta_applied_upto` survives (it was written AFTER
    // the snapshot captured its HWM). Chunks in this range that were never
    // written (an empty promote) are a no-op `KvOp::Remove` on every backend.
    for idx in 0..new_delta_applied_upto {
        ops.push(KvOp::Remove(delta_chunk_key(keyspace, idx)));
    }

    // Publish the new manifest — the generation flip itself.
    let manifest_env = MetaEnvelope::new(new_manifest);
    let manifest_bytes = manifest_env
        .encode()
        .map_err(|e| SnapshotError::Serde(e.to_string()))?;
    ops.push(KvOp::Set(
        manifest_key(keyspace),
        Bytes::from(manifest_bytes),
    ));

    store.transact(ops).await?;
    Ok(())
}

/// Scan the keyspace for the highest delta-chunk index currently present.
/// Used by `restore_on_open` to seed the in-memory HWM after a restart so
/// the next `append_delta` does not collide with an existing chunk. Returns
/// `0` when no delta chunks exist.
pub async fn highest_delta_index(
    store: &Arc<dyn Store>,
    keyspace: &str,
) -> Result<u64, SnapshotError> {
    use shamir_tunables::store_defaults::MAINT_SCAN_BATCH;
    let prefix = delta_scan_prefix(keyspace);
    let mut stream = store.scan_prefix_stream(prefix.into(), MAINT_SCAN_BATCH);
    let mut highest: u64 = 0;
    while let Some(batch_res) = stream.next().await {
        let batch: Vec<(RecordKey, Bytes)> = batch_res?;
        for (key, _val) in batch {
            let key_bytes: &[u8] = key.as_ref();
            let key_str = match std::str::from_utf8(key_bytes) {
                Ok(s) => s,
                Err(_) => continue,
            };
            if let Some(idx_str) = key_str.rsplit('.').next() {
                if let Ok(idx) = idx_str.parse::<u64>() {
                    if idx > highest {
                        highest = idx;
                    }
                }
            }
        }
    }
    Ok(highest)
}

/// Debug-only build-param sanity check. `get_*` on the loaded graph reads
/// the persisted structure; a mismatch would mean our sidecar lied about the
/// build params (a codec bug, not a corrupt-graph case — the per-chunk and
/// cross-section crcs already vouched for byte fidelity).
#[cfg(debug_assertions)]
fn debug_assert_build_params(
    hnsw: &Hnsw<'static, f32, ShamirDist>,
    m: u8,
    max_layer: usize,
    ef_construction: usize,
) {
    debug_assert_eq!(hnsw.get_max_nb_connection(), m, "sidecar m mismatch");
    debug_assert_eq!(
        hnsw.get_max_level(),
        max_layer,
        "sidecar max_layer mismatch"
    );
    debug_assert_eq!(
        hnsw.get_ef_construction(),
        ef_construction,
        "sidecar ef_construction mismatch"
    );
}

#[cfg(not(debug_assertions))]
fn debug_assert_build_params(
    _hnsw: &Hnsw<'static, f32, ShamirDist>,
    _m: u8,
    _max_layer: usize,
    _ef_construction: usize,
) {
}
