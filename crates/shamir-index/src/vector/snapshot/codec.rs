//! On-wire types, keyspace key layout, version constants and the dump
//! (write) path of the snapshot codec. The load path + delta-log live in
//! [`super::load`].

use super::manifest::{integrity_bytes, integrity_key};
use crate::kind::VectorMetric;
use crate::meta_envelope::MetaEnvelope;
use crate::vector::hnsw_adapter::HnswAdapter;
use bytes::Bytes;
use hnsw_rs::api::AnnT;
use serde::{Deserialize, Serialize};
use shamir_storage::error::DbError;
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_types::types::record_id::RecordId;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

/// Target chunk size. ~1 MiB keeps each store record comfortably under the
/// memtable page size of every backend we ship (redb/sled/fjall all default
/// to multi-MiB pages) while bounding the per-chunk crc32 cost to a single
/// scan over 1 MiB.
const CHUNK_SIZE: usize = 1 << 20; // 1 MiB

/// Format version stamped into the sidecar and manifest. Bumped only on a
/// breaking layout change. The envelope ALSO carries a version (for the
/// outer wrapper); this one is the inner snapshot-layout version.
///
/// V5.3 (#412): bumped 1 → 2. A v2 sidecar MAY carry a `quantization:
/// Some(bincode(QuantMeta))` field; the u8 graph (if quantized) is stored
/// as additional chunks under the same generation. A v2 sidecar with
/// `quantization == None` is semantically equivalent to a v1 snapshot
/// (non-quantized f32 path). The load path accepts BOTH v1 and v2
/// (back-compat — see [`SNAPSHOT_SUPPORTED_VERSIONS`]).
pub(crate) const SNAPSHOT_FORMAT_VERSION: u16 = 2;

/// Snapshot format versions this build can load. V5.3 (#412) accepts both
/// v1 (the original f32-only codec) and v2 (the quantization-aware codec).
/// A dump from a future v3 is refused with `VersionMismatch`. A v1 dump
/// loads on a v2 build via the back-compat path: no quantization meta is
/// present, so the adapter is constructed un-fitted (f32-only) — exactly
/// the pre-#412 behaviour.
pub(crate) const SNAPSHOT_SUPPORTED_VERSIONS: &[u16] = &[1, 2];

/// The V5.3 (#412) marker stamped into `QuantMeta::method` for the SQ8
/// quantizer. A future method (PQ, BQ) would use a different string and a
/// different load branch.
pub(crate) const QUANT_METHOD_SQ8: &str = "sq8";

/// The `hnsw_rs` crate version this codec understands. Pinned to the exact
/// patch in `Cargo.toml` (the pin comment there explains why). `hnsw_rs`
/// does NOT export its version as a const, so we hard-code it here and
/// assert parity at load time — a dump written by a different `hnsw_rs`
/// version is refused with `VersionMismatch` rather than handed to a
/// loader that may panic on an unknown on-disk format.
pub(crate) const HNSW_RS_VERSION: &str = "0.3.5";

// The workspace builds hnsw_rs from `vendor/hnsw_rs` (lock-order fix for concurrent
// inserts). A build against the plain crates.io crate has no such marker and fails here.
const _: () = assert!(hnsw_rs::SHAMIR_REVERSE_UPDATE_LOCK_FIX);

/// Default basename passed to `hnsw_rs::file_dump`. The loader records the
/// ACTUAL basename (which `DumpInit` may uniquify on collision) in the
/// manifest; this constant is only the seed.
const DUMP_BASENAME_SEED: &str = "shamir";

/// 6-digit zero-padded decimal chunk index. 10^6 chunks × 1 MiB = 1 TiB per
/// dump file — comfortably past any realistic graph size, and the zero-padding
/// makes lexicographic key order == numeric order (so a prefix scan walks
/// chunks in order without a comparator).
const CHUNK_IDX_WIDTH: usize = 6;

/// V5.3 (#412) — Carrier for the quantized-dump manifest fields produced by
/// the u8-graph dump branch of [`dump_snapshot_with_gen`]. Stashed in a local
/// so the manifest writer below can populate `qgraph_chunks`/`qdata_chunks`/
/// `qbasename` without re-reading the sidecar.
struct QuantDumpInfo {
    /// The `QuantMeta` to be bincode-encoded into the sidecar's
    /// `quantization` field.
    meta: super::QuantMeta,
    /// Basename returned by `file_dump` for the u8 graph.
    qbasename: String,
    /// Number of chunks the `.hnsw.graph` (u8) file was sliced into.
    qgraph_chunks: u32,
    /// Number of chunks the `.hnsw.data` (u8) file was sliced into.
    qdata_chunks: u32,
}

/// Keyspace tag layout. `<keyspace>` is caller-supplied (the engine will pass
/// the vector index's own keyspace). All records the codec writes live under
/// `<keyspace>.g<N>.{graph|data}.KKKK`, `<keyspace>.g<N>.sidecar`, and
/// `<keyspace>.manifest`.
pub(super) fn chunk_key(keyspace: &str, gen: u32, section: &str, idx: usize) -> RecordKey {
    // `<keyspace>.g<N>.<section>.KKKKKK`
    Bytes::from(format!(
        "{ks}.g{gen}.{section}.{idx:0width$}",
        ks = keyspace,
        gen = gen,
        section = section,
        idx = idx,
        width = CHUNK_IDX_WIDTH,
    ))
    .into()
}

pub(super) fn sidecar_key(keyspace: &str, gen: u32) -> RecordKey {
    Bytes::from(format!("{ks}.g{gen}.sidecar", ks = keyspace)).into()
}

pub(super) fn manifest_key(keyspace: &str) -> RecordKey {
    Bytes::from(format!("{ks}.manifest", ks = keyspace)).into()
}

// `pub(crate)` test-only accessors — the codec owns the keyspace layout, so
// the snapshot tests ask it for the exact keys they need to mutate (corrupt
// a chunk, tamper with the sidecar). These are NOT part of the public API.

#[cfg(test)]
pub(crate) fn chunk_key_for_test(keyspace: &str, gen: u32, section: &str, idx: usize) -> RecordKey {
    chunk_key(keyspace, gen, section, idx)
}

#[cfg(test)]
pub(crate) fn sidecar_key_for_test(keyspace: &str, gen: u32) -> RecordKey {
    sidecar_key(keyspace, gen)
}

#[cfg(test)]
pub(crate) fn manifest_key_for_test(keyspace: &str) -> RecordKey {
    manifest_key(keyspace)
}

// ============================================================================
// On-wire types (bincode inside a MetaEnvelope)
// ============================================================================

/// crc32-checked chunk payload.
///
/// `bytes` is the raw file slice; `crc32` is `crc32fast::hash(&bytes)`. Load
/// verifies each chunk independently so a single bit-flip fails fast with the
/// exact chunk index in the error, rather than corrupting the whole load.
#[derive(Clone, Serialize, Deserialize)]
pub(super) struct ChunkHeader {
    pub(super) idx: u32,
    pub(super) crc32: u32,
    pub(super) bytes: Vec<u8>,
}

/// Sidecar — everything the `hnsw_rs` graph dump does NOT carry.
///
/// Reserved-for-P5 fields are `Option`s left `None` so the layout is stable
/// when quantization lands.
#[derive(Clone, Serialize, Deserialize)]
pub struct SnapshotSidecar {
    /// Inner snapshot-layout version (`SNAPSHOT_FORMAT_VERSION`).
    pub format_version: u16,
    pub dim: u32,
    pub metric: VectorMetric,
    pub ef_search: usize,
    /// HNSW build parameters (read off the live graph via `Hnsw::get_*`).
    pub m: u8,
    pub max_layer: usize,
    pub ef_construction: usize,
    /// `next_id` counter — restored verbatim so post-load upserts do not
    /// collide with existing internals.
    pub next_id: usize,
    /// `internal -> RecordId` forward map.
    pub rid_map: Vec<(usize, RecordId)>,
    /// `RecordId -> internal` reverse map.
    pub rid_to_internal: Vec<(RecordId, usize)>,
    /// Tombstoned internals (soft-deleted graph nodes still referenced by
    /// the loaded graph; search filters them out).
    pub tombstones: Vec<usize>,
    /// `internal -> raw vector`. Kept so the small-index exact brute-force
    /// path (see `BRUTE_FORCE_MAX`) works immediately after load without a
    /// re-scan.
    pub vectors: Vec<(usize, Vec<f32>)>,
    /// `hnsw_rs` crate version that produced the dump. Loaded into the
    /// adapter only for diagnostics today; a future bump can refuse to load
    /// a foreign version by comparing against the build-time constant.
    pub hnsw_rs_version: String,
    /// RESERVED (P5 quantization). Always `None` in V2.1.
    ///
    /// V5.3 (#412): `Some(bincode(QuantMeta))` for a quantized v2 snapshot;
    /// `None` for a non-quantized (or v1) snapshot.
    #[serde(default)]
    pub quantization: Option<Vec<u8>>,
    /// V5.3 (#412): `internal -> u8 codes` for a fitted quantized adapter.
    /// Empty for a non-quantized snapshot. The u8 graph dump carries its own
    /// copy of the codes inside `Point<T>` DataPoints, but the graph does
    /// NOT expose a `get_vector(id)` accessor — so this map is the
    /// authoritative source for rescore + brute-force on load.
    #[serde(default)]
    pub vectors_u8: Vec<(usize, Vec<u8>)>,
    /// crc32 over the concatenation `[graph_file_bytes, data_file_bytes]` —
    /// a cross-section integrity check independent of the per-chunk crc32s.
    pub sections_crc32: u32,
    /// Total byte length of the `.hnsw.graph` file (sum of `graph` chunks).
    pub graph_len: u64,
    /// Total byte length of the `.hnsw.data` file (sum of `data` chunks).
    pub data_len: u64,
}

/// Manifest — points at the active generation.
///
/// `gen` is the snapshot generation index. V2.1 always writes a single
/// generation (`gen = 0`); the generation-flip protocol (#402) bumps it on
/// each new snapshot and atomically flips the manifest to the new gen once
/// the chunks + sidecar are durable.
///
/// `delta_applied_upto` (V2.3 / #402) records the number of delta chunks the
/// snapshot already accounts for: the snapshot was taken from a graph that
/// had absorbed delta chunks `0..delta_applied_upto` (i.e. chunks with index
/// `< delta_applied_upto`). On restart the replay therefore walks only chunks
/// with index `>= delta_applied_upto`, and a generation flip (which writes a
/// fresh snapshot over a fully-rebuilt graph) sets this to the current
/// `next_delta_idx` (so every chunk written so far is absorbed + pruned).
/// A `0` value means "no delta has been absorbed" — either a fresh V2.1
/// snapshot, or a V2.3 snapshot taken before any delta chunks existed.
///
/// V5.3 (#412): `qgraph_chunks`/`qdata_chunks` carry the chunk counts for the
/// quantized u8 graph dump. They are `0` for a non-quantized snapshot (the
/// `qgraph`/`qdata` sections are absent). For a quantized v2 snapshot they
/// point at the `<keyspace>.g<N>.{qgraph|qdata}.KKKK` chunks.
#[derive(Clone, Serialize, Deserialize)]
pub struct SnapshotManifest {
    /// Inner snapshot-layout version.
    pub format_version: u16,
    /// Active generation index.
    pub gen: u32,
    /// Number of chunks the `.hnsw.graph` file was sliced into.
    pub graph_chunks: u32,
    /// Number of chunks the `.hnsw.data` file was sliced into.
    pub data_chunks: u32,
    /// V5.3 (#412): number of chunks the quantized `.hnsw.graph` (u8) file
    /// was sliced into. `0` for a non-quantized snapshot.
    #[serde(default)]
    pub qgraph_chunks: u32,
    /// V5.3 (#412): number of chunks the quantized `.hnsw.data` (u8) file
    /// was sliced into. `0` for a non-quantized snapshot.
    #[serde(default)]
    pub qdata_chunks: u32,
    /// Basename `hnsw_rs::file_dump` ACTUALLY used for this dump. `DumpInit`
    /// can uniquify the basename on a name collision, so we MUST store the
    /// returned name — the load path rebuilds the temp files under this name.
    pub basename: String,
    /// V5.3 (#412): basename used for the quantized u8 graph dump (when
    /// `qgraph_chunks > 0`). Stored separately because `DumpInit` may
    /// uniquify it independently of the f32 basename.
    #[serde(default)]
    pub qbasename: String,
    /// Number of delta chunks absorbed into the snapshot's graph (V2.3 / #402).
    /// On restart, replay walks only chunks with index `>= delta_applied_upto`.
    /// `0` = no delta absorbed yet (fresh snapshot). Set to `next_delta_idx`
    /// by a generation flip so every chunk written so far is absorbed + pruned.
    pub delta_applied_upto: u64,
}

// ============================================================================
// Errors
// ============================================================================

/// All snapshot codec failures. `Corrupt` is the crc32 mismatch;
/// `VersionMismatch` is either the envelope version or the inner snapshot
/// layout version disagreeing with what this build understands.
#[derive(Debug, Error)]
pub enum SnapshotError {
    #[error("snapshot corrupt: {0}")]
    Corrupt(String),
    #[error("snapshot version mismatch: {0}")]
    VersionMismatch(String),
    #[error("snapshot I/O error: {0}")]
    Io(String),
    #[error("snapshot serde error: {0}")]
    Serde(String),
    #[error("snapshot backend error: {0}")]
    Backend(String),
    #[error("snapshot not found (no manifest for keyspace)")]
    NotFound,
}

impl From<std::io::Error> for SnapshotError {
    fn from(e: std::io::Error) -> Self {
        SnapshotError::Io(e.to_string())
    }
}

impl From<DbError> for SnapshotError {
    fn from(e: DbError) -> Self {
        match e {
            DbError::NotFound(_) => SnapshotError::NotFound,
            other => SnapshotError::Backend(other.to_string()),
        }
    }
}

impl From<bincode::Error> for SnapshotError {
    fn from(e: bincode::Error) -> Self {
        SnapshotError::Serde(e.to_string())
    }
}

// ============================================================================
// Dump path
// ============================================================================

/// Dump the adapter's live HNSW graph + sidecar + manifest into `store` under
/// `keyspace`. Overwrites any existing snapshot (the generation-flip protocol
/// of #402 will make this incremental; for V2.1 a single full dump is fine).
///
/// CPU/IO-bound work (file_dump, file reads) runs under `spawn_blocking`.
/// The chunk + sidecar + manifest write is a single `transact` so backends
/// that override `transact` expose the dump atomically.
pub async fn dump_snapshot(
    adapter: &HnswAdapter,
    store: &Arc<dyn Store>,
    keyspace: &str,
) -> Result<(), SnapshotError> {
    dump_snapshot_with_gen(adapter, store, keyspace, 0).await
}

/// Same as [`dump_snapshot`] but lets the caller pick the generation index.
/// The generation flip in #402 will call this with the next gen; V2.1's
/// `dump_snapshot` always uses `0`.
///
/// V5.3 (#412): For a fitted quantized adapter (`adapter.is_quantized()`),
/// this dumps the u8 graph into a SECOND pair of chunk sections
/// (`qgraph`/`qdata`) under the same generation AND stamps the quantizer
/// params into the sidecar (`quantization = Some(bincode(QuantMeta))`). For
/// a non-quantized adapter, the dump is identical to the pre-#412 codec
/// (only `graph`/`data` sections, `quantization = None`).
pub async fn dump_snapshot_with_gen(
    adapter: &HnswAdapter,
    store: &Arc<dyn Store>,
    keyspace: &str,
    gen: u32,
) -> Result<(), SnapshotError> {
    // ---- 1. file_dump the f32 graph in a TempDir (CPU/IO) ----------------
    //
    // The f32 graph is dumped UNLESS the adapter is a fitted quantized one
    // (#418): such an adapter has DROPPED its f32 graph post-fit (the u8
    // graph + `vectors_u8` codes are the authoritative post-fit state, and
    // retaining the f32 graph would defeat SQ8's 4× memory win). For a
    // fitted quantized adapter we emit ZERO-length graph/data sections; the
    // load path (`load_snapshot`) recognises a quantized sidecar +
    // `qgraph_chunks > 0` and assembles the adapter via
    // `from_parts_with_quantization`, which drops the throwaway f32 graph
    // immediately.
    //
    // `file_dump` is a TRAIT method — only callable after `use AnnT`.
    let (graph_bytes, data_bytes, basename): (Vec<u8>, Vec<u8>, String) =
        if let Some(hnsw) = adapter.hnsw_load() {
            let dump_dir = tempfile::tempdir()?;
            let dump_dir_path: PathBuf = dump_dir.path().to_path_buf();
            let basename_seed = DUMP_BASENAME_SEED.to_string();
            let basename = tokio::task::spawn_blocking(move || {
                hnsw.file_dump(&dump_dir_path, &basename_seed)
                    .map_err(|e| SnapshotError::Io(format!("hnsw_rs file_dump failed: {e}")))
            })
            .await
            .map_err(|e| SnapshotError::Io(format!("spawn_blocking join error: {e}")))??;

            // ---- 2. read both dump files, slice into chunks, compute crcs --
            let graph_path = dump_dir.path().join(format!("{basename}.hnsw.graph"));
            let data_path = dump_dir.path().join(format!("{basename}.hnsw.data"));
            let graph_path_b = graph_path.clone();
            let data_path_b = data_path.clone();
            let (g, d) = tokio::task::spawn_blocking(move || {
                let g = std::fs::read(&graph_path_b)?;
                let d = std::fs::read(&data_path_b)?;
                Ok::<_, SnapshotError>((g, d))
            })
            .await
            .map_err(|e| SnapshotError::Io(format!("spawn_blocking join error: {e}")))??;
            (g, d, basename)
        } else {
            // #418 — fitted quantized adapter: f32 graph dropped. Emit empty
            // graph/data sections (basename is a placeholder; no files read).
            // The load path keys off the quantized sidecar + `qgraph_chunks
            // > 0` and skips the f32 load entirely.
            (Vec::new(), Vec::new(), DUMP_BASENAME_SEED.to_string())
        };

    // Build the KvOp batch. We accumulate every chunk + the sidecar + the
    // manifest and apply them under ONE `transact` so the dump is observable
    // as a single all-or-nothing batch on backends that override `transact`.
    let mut ops: Vec<KvOp> = Vec::new();

    let (graph_chunks, data_chunks) = (
        slice_into_chunk_ops(&graph_bytes, keyspace, gen, "graph", &mut ops),
        slice_into_chunk_ops(&data_bytes, keyspace, gen, "data", &mut ops),
    );

    // Cross-section crc32 over the f32 sections (graph + data). For a
    // quantized dump the u8 sections get their OWN cross-section crc carried
    // in `QuantMeta`-adjacent fields (we extend the sidecar crc to cover all
    // sections — see below).
    let mut hasher = crc32fast::Hasher::new();
    hasher.update(&graph_bytes);
    hasher.update(&data_bytes);

    // ---- 2b. V5.3 (#412): dump the u8 graph for a fitted adapter ---------
    //
    // For a quantized adapter we file_dump the u8 graph into a SECOND pair
    // of sections (`qgraph`/`qdata`) under a distinct basename. The sidecar
    // carries the quantizer params so load can reconstruct ShamirDistU8 and
    // the u8 graph together.
    //
    // `quant_dump` carries the manifest-side fields (chunk counts + basename)
    // out of the branch so the manifest writer below can populate them.
    let quant_dump: Option<QuantDumpInfo> = if adapter.is_quantized() {
        let q = adapter
            .quantizer()
            .ok_or_else(|| SnapshotError::Backend("fitted adapter has no quantizer".into()))?;
        // Build a QuantMeta from the frozen quantizer.
        let meta = super::QuantMeta::from_quantizer(q);
        // Dump the u8 graph into its OWN TempDir (#418 — the f32 dump_dir is
        // no longer always created: a fitted quantized adapter skips the f32
        // dump, so we cannot reuse its TempDir). The TempDir is dropped after
        // the files are read into memory below.
        let hnsw_u8 = adapter
            .hnsw_u8_handle()
            .ok_or_else(|| SnapshotError::Backend("fitted adapter has no u8 graph".into()))?;
        let dump_dir_q = tempfile::tempdir()?;
        let dump_dir_path_q: PathBuf = dump_dir_q.path().to_path_buf();
        let basename_seed_q = format!("{}q", DUMP_BASENAME_SEED);
        let hnsw_u8_for_dump = Arc::clone(&hnsw_u8);
        let qbasename = tokio::task::spawn_blocking(move || {
            hnsw_u8_for_dump
                .file_dump(&dump_dir_path_q, &basename_seed_q)
                .map_err(|e| SnapshotError::Io(format!("hnsw_rs file_dump (u8) failed: {e}")))
        })
        .await
        .map_err(|e| SnapshotError::Io(format!("spawn_blocking join error: {e}")))??;

        let qgraph_path = dump_dir_q.path().join(format!("{qbasename}.hnsw.graph"));
        let qdata_path = dump_dir_q.path().join(format!("{qbasename}.hnsw.data"));
        let qgraph_path_b = qgraph_path.clone();
        let qdata_path_b = qdata_path.clone();
        let (qgraph_bytes, qdata_bytes) = tokio::task::spawn_blocking(move || {
            let g = std::fs::read(&qgraph_path_b)?;
            let d = std::fs::read(&qdata_path_b)?;
            Ok::<_, SnapshotError>((g, d))
        })
        .await
        .map_err(|e| SnapshotError::Io(format!("spawn_blocking join error: {e}")))??;

        // Slice + crc the u8 sections.
        let (qgraph_chunks, qdata_chunks) = (
            slice_into_chunk_ops(&qgraph_bytes, keyspace, gen, "qgraph", &mut ops),
            slice_into_chunk_ops(&qdata_bytes, keyspace, gen, "qdata", &mut ops),
        );

        // Extend the cross-section crc32 over the u8 sections too, so a
        // section-key-swap (qgraph chunk mis-attributed to graph) is caught.
        hasher.update(&qgraph_bytes);
        hasher.update(&qdata_bytes);

        Some(QuantDumpInfo {
            meta,
            qbasename,
            qgraph_chunks,
            qdata_chunks,
        })
    } else {
        None
    };

    // ---- 3. build the sidecar --------------------------------------------
    //
    // Read adapter state via the pub(crate) accessors + cursor scans. The
    // cursors run synchronously here — they touch a snapshot of the live
    // scc HashMaps; concurrent upserts can race, but P2 dumps are an
    // explicit, infrequent, caller-driven action (the engine will gate them
    // behind a quiesce in #402), not a hot-path concern.
    let mut rid_map: Vec<(usize, RecordId)> = Vec::new();
    adapter.for_each_rid_map(|internal, rid| rid_map.push((internal, rid)));

    let mut rid_to_internal: Vec<(RecordId, usize)> = Vec::new();
    adapter.for_each_rid_to_internal(|rid, internal| rid_to_internal.push((rid, internal)));

    let mut tombstones: Vec<usize> = Vec::new();
    adapter.for_each_deleted(|internal| tombstones.push(internal));

    let mut vectors: Vec<(usize, Vec<f32>)> = Vec::new();
    adapter.for_each_vector(|internal, v| vectors.push((internal, v.to_vec())));

    // V5.3 (#412): for a fitted adapter, also serialise the u8 codes
    // (`vectors_u8`) into the sidecar. The u8 graph dump carries its own
    // copy of the codes inside `Point<T>` DataPoints, but the graph does
    // NOT expose a `get_vector(id)` accessor — so the sidecar's `vectors_u8`
    // is the authoritative source for rescore + brute-force on load.
    let mut vectors_u8: Vec<(usize, Vec<u8>)> = Vec::new();
    if adapter.is_quantized() {
        adapter.for_each_vector_u8(|internal, codes| vectors_u8.push((internal, codes.to_vec())));
    }

    let sections_crc32 = hasher.finalize();

    // Encode the quantization sidecar blob (if any).
    let quantization_blob: Option<Vec<u8>> = match &quant_dump {
        Some(info) => Some(bincode::serialize(&info.meta)?),
        None => None,
    };

    // #418 — build params come from whichever graph is resident: the f32
    // graph (non-quant / pre-fit) or, when it has been dropped post-fit, the
    // u8 graph (m/max_layer/ef_construction are identical across the two —
    // both built from the same HnswConfig). `None` only if BOTH are absent,
    // an invariant violation surfaced as a snapshot error.
    let (m, max_layer, ef_construction) = adapter
        .build_params()
        .ok_or_else(|| SnapshotError::Backend("no graph resident (neither f32 nor u8)".into()))?;
    let sidecar = SnapshotSidecar {
        format_version: SNAPSHOT_FORMAT_VERSION,
        dim: adapter.dim_field(),
        metric: adapter.metric_field(),
        ef_search: adapter.ef_search_field(),
        m,
        max_layer,
        ef_construction,
        next_id: adapter.next_id_value(),
        rid_map,
        rid_to_internal,
        tombstones,
        vectors,
        hnsw_rs_version: HNSW_RS_VERSION.to_string(),
        quantization: quantization_blob,
        sections_crc32,
        graph_len: graph_bytes.len() as u64,
        data_len: data_bytes.len() as u64,
        // V5.3 (#412): carry the u8 codes for the quantized path.
        vectors_u8,
    };
    let sidecar_env = MetaEnvelope::new(sidecar);
    let sidecar_bytes = sidecar_env
        .encode()
        .map_err(|e| SnapshotError::Serde(e.to_string()))?;
    ops.push(KvOp::Set(
        sidecar_key(keyspace, gen),
        Bytes::from(sidecar_bytes.clone()),
    ));

    // ---- 4. build the manifest -------------------------------------------
    //
    // V5.3 (#412): populate `qgraph_chunks`/`qdata_chunks`/`qbasename` for a
    // quantized dump (extracted from the `QuantDumpInfo` stashed by the dump
    // branch).
    let (qgraph_chunks, qdata_chunks, qbasename) = match &quant_dump {
        Some(info) => (
            info.qgraph_chunks,
            info.qdata_chunks,
            info.qbasename.clone(),
        ),
        None => (0u32, 0u32, String::new()),
    };
    let manifest = SnapshotManifest {
        format_version: SNAPSHOT_FORMAT_VERSION,
        gen,
        graph_chunks,
        data_chunks,
        qgraph_chunks,
        qdata_chunks,
        basename: basename.clone(),
        qbasename,
        // V2.1 dumps (gen 0, no delta log in play) carry `delta_applied_upto
        // = 0`. The generation-flip path (`flip_generation`) overrides this
        // with the index of the last delta chunk it just absorbed + pruned.
        delta_applied_upto: 0,
    };
    let manifest_env = MetaEnvelope::new(manifest);
    let manifest_bytes = manifest_env
        .encode()
        .map_err(|e| SnapshotError::Serde(e.to_string()))?;

    // Wipe any PRIOR generation's chunks first so a load can never mix
    // generations. For V2.1 there is only gen 0; the generation flip in
    // #402 will keep the old gen alive until the new one is verified.
    // We do this as a separate pass (not part of the main `transact`)
    // because we don't know the prior gen's chunk counts here without
    // reading its manifest; leaving stale chunks is harmless for V2.1
    // since the manifest points unambiguously at the new gen.
    ops.push(KvOp::Set(
        integrity_key(keyspace),
        integrity_bytes(&manifest_bytes, &sidecar_bytes),
    ));
    ops.push(KvOp::Set(
        manifest_key(keyspace),
        Bytes::from(manifest_bytes),
    ));

    // ---- 5. atomic write -------------------------------------------------
    store.transact(ops).await?;
    Ok(())
}

/// Slice `bytes` into ~`CHUNK_SIZE` chunks; push a `Set` op per chunk into
/// `ops` and return the chunk count. Each chunk carries its own crc32 in a
/// [`ChunkHeader`].
fn slice_into_chunk_ops(
    bytes: &[u8],
    keyspace: &str,
    gen: u32,
    section: &str,
    ops: &mut Vec<KvOp>,
) -> u32 {
    if bytes.is_empty() {
        // An empty file still gets a single zero-length chunk so the load
        // path's "read N chunks" loop has exactly N=N_manifest entries to
        // fetch (no off-by-one for the empty case).
        let header = ChunkHeader {
            idx: 0,
            crc32: crc32fast::hash(&[]),
            bytes: Vec::new(),
        };
        let encoded = bincode::serialize(&header).expect("chunk header serializable");
        ops.push(KvOp::Set(
            chunk_key(keyspace, gen, section, 0),
            Bytes::from(encoded),
        ));
        return 1;
    }
    let mut idx = 0u32;
    for chunk in bytes.chunks(CHUNK_SIZE) {
        let crc = crc32fast::hash(chunk);
        let header = ChunkHeader {
            idx,
            crc32: crc,
            bytes: chunk.to_vec(),
        };
        let encoded = bincode::serialize(&header).expect("chunk header serializable");
        ops.push(KvOp::Set(
            chunk_key(keyspace, gen, section, idx as usize),
            Bytes::from(encoded),
        ));
        idx += 1;
    }
    idx
}

/// Map a `MetaEnvelope::open` failure to the right snapshot error. An
/// unsupported format version is a genuine `VersionMismatch`; bad magic or a
/// decode failure means the manifest/sidecar bytes themselves are corrupt
/// (they carry no crc of their own, unlike the chunk bodies).
pub(super) fn map_meta_err(e: crate::meta_envelope::MetaError) -> SnapshotError {
    use crate::meta_envelope::MetaError;
    match e {
        MetaError::UnsupportedVersion(v) => {
            SnapshotError::VersionMismatch(format!("envelope version {v} unsupported"))
        }
        other => SnapshotError::Corrupt(format!("envelope decode: {other}")),
    }
}
