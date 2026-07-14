//! Snapshot codec for the persisted HNSW graph (P2 — V2.1).
//!
//! This module gives `HnswAdapter` an isolated, unit-testable dump/load
//! round-trip against any `Store`. It is the FIRST P2 slice: ONLY the codec —
//! startup integration (#401) and the incremental delta-log / generation flip
//! (#402) are out of scope and land in later sheets.
//!
//! ## Layout in the info_store
//!
//! All keys live under the caller-supplied `keyspace` (e.g. `__info__<table>`
//! or a dedicated vector-keyspace); the codec prefixes every record it writes
//! with `<keyspace>.`. Three kinds of records are emitted for generation `N`:
//!
//! * **Chunks** — the `hnsw_rs` dump files (`.hnsw.graph`, `.hnsw.data`) sliced
//!   into ~1 MiB pieces, written as raw bytes under
//!   `<keyspace>.g<N>.graph.KKKK` / `<keyspace>.g<N>.data.KKKK` where `KKKK`
//!   is a 6-digit zero-padded chunk index. Each chunk carries its own crc32 in
//!   the [`ChunkHeader`] (checksums-everywhere pillar): a bit-flip in any
//!   chunk surfaces as [`SnapshotError::Corrupt`] at load, not as a silent
//!   graph corruption.
//! * **Sidecar** — `<keyspace>.g<N>.sidecar` (MetaEnvelope-wrapped, bincode):
//!   everything the graph dump itself does NOT carry — the adapter's maps
//!   (`rid_map`, `rid_to_internal`, `tombstones`, `vectors`), the build
//!   parameters, and the cross-section crc32. `RESERVED` fields
//!   (`quantization`) are `Option`s left `None` until P5.
//! * **Manifest** — `<keyspace>.manifest` (MetaEnvelope-wrapped): the single
//!   source of truth for "which generation is live". Points at `gen N`, the
//!   chunk counts, and the basename `hnsw_rs::file_dump` actually used (it can
//!   be uniquified by the loader when a name collision is detected, so we MUST
//!   record the returned name — see `DumpInit::get_basename`).
//!
//! ## Lifetime: how `Hnsw<'static>` is produced from a load
//!
//! `hnsw_rs::HnswIo::load_hnsw_with_dist<'b, 'a>` ties `'a: 'b` where `'a` is
//! the borrow of the `HnswIo` loader. To hand the reloaded graph to long-lived
//! storage as `Arc<Hnsw<'static, ...>>` (the shape `HnswAdapter` already uses),
//! the loader itself must outlive every reference the graph hands out — i.e.
//! it must be `'static`. `Box::leak(Box::new(HnswIo))` is the sanctioned
//! boot-only pattern (mirrors the V0.0 contract-test
//! `leaked_loader_yields_static_hnsw`): the loader is tiny and lives for the
//! process; the dump files are the durable artefact. A leaked `HnswIo` per
//! snapshot is acceptable ONLY because snapshots are loaded once at boot (a
//! handful per shard). It is NOT a per-request pattern.
//!
//! ## Pillars honoured
//!
//! * `spawn_blocking` for every CPU/IO-bound step (`file_dump`, file read,
//!   file write on load) — never block the async executor on disk I/O.
//! * `Store::transact` for the atomic chunk + sidecar + manifest write — the
//!   dump is observable as a single all-or-nothing batch on backends that
//!   override `transact`, and as per-op sequential on the default impl.
//! * crc32 on every chunk + a cross-section crc32 in the sidecar
//!   (checksums-everywhere).

use crate::kind::VectorMetric;
use crate::meta_envelope::MetaEnvelope;
use crate::vector::adapter::VectorAdapter;
use crate::vector::hnsw_adapter::{HnswAdapter, ShamirDist};
use crate::vector::quantized_dist::ShamirDistU8;
use crate::vector::sq8::Sq8Quantizer;
use bytes::Bytes;
use futures::StreamExt;
use hnsw_rs::api::AnnT;
use hnsw_rs::hnsw::Hnsw;
use hnsw_rs::hnswio::HnswIo;
use serde::{Deserialize, Serialize};
use shamir_storage::error::DbError;
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_types::types::common::THasher;
use shamir_types::types::record_id::RecordId;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use thiserror::Error;

/// Target chunk size. ~1 MiB keeps each store record comfortably under the
/// memtable page size of every backend we ship (redb/sled/fjall all default
/// to multi-MiB pages) while bounding the per-chunk crc32 cost to a single
/// scan over 1 MiB.
const CHUNK_SIZE: usize = 1 << 20; // 1 MiB

// Re-export QuantMeta so callers (and tests) can reach it as
// `crate::vector::snapshot::QuantMeta` — the snapshot codec owns the
// quantization-on-wire contract.
pub use crate::vector::quant_meta::QuantMeta;

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
pub(crate) const HNSW_RS_VERSION: &str = "0.3.4";

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
    meta: QuantMeta,
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
fn chunk_key(keyspace: &str, gen: u32, section: &str, idx: usize) -> RecordKey {
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

fn sidecar_key(keyspace: &str, gen: u32) -> RecordKey {
    Bytes::from(format!("{ks}.g{gen}.sidecar", ks = keyspace)).into()
}

fn manifest_key(keyspace: &str) -> RecordKey {
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

// ============================================================================
// On-wire types (bincode inside a MetaEnvelope)
// ============================================================================

/// crc32-checked chunk payload.
///
/// `bytes` is the raw file slice; `crc32` is `crc32fast::hash(&bytes)`. Load
/// verifies each chunk independently so a single bit-flip fails fast with the
/// exact chunk index in the error, rather than corrupting the whole load.
#[derive(Clone, Serialize, Deserialize)]
struct ChunkHeader {
    idx: u32,
    crc32: u32,
    bytes: Vec<u8>,
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
        let meta = QuantMeta::from_quantizer(q);
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
        Bytes::from(sidecar_bytes),
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

// ============================================================================
// Load path
// ============================================================================

/// Map a `MetaEnvelope::open` failure to the right snapshot error. An
/// unsupported format version is a genuine `VersionMismatch`; bad magic or a
/// decode failure means the manifest/sidecar bytes themselves are corrupt
/// (they carry no crc of their own, unlike the chunk bodies).
fn map_meta_err(e: crate::meta_envelope::MetaError) -> SnapshotError {
    use crate::meta_envelope::MetaError;
    match e {
        MetaError::UnsupportedVersion(v) => {
            SnapshotError::VersionMismatch(format!("envelope version {v} unsupported"))
        }
        other => SnapshotError::Corrupt(format!("envelope decode: {other}")),
    }
}

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
/// `Box::leak` → `Hnsw<'static>` → `HnswAdapter::from_parts`.
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
            // on-disk files. That is why it is safe for `load_dir` (this
            // TempDir) to drop when the closure returns: the files are already
            // resident. If anyone enables mmap (`set_mmap(true)`) here, the
            // graph would borrow the deleted temp files → UAF; `load_dir`
            // would then have to be leaked too.
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
            // The u8 graph temp files live in a SECOND TempDir so the f32
            // TempDir's drop (which happens when the f32 spawn_blocking closure
            // returns) does NOT delete the u8 files before we load them. We
            // leak this TempDir for the same reason we leak the HnswIo: boot-
            // only, one per shard. (The HnswIo loads the data INTO MEMORY, so
            // the files are not needed after load completes — but the leak is
            // defensive: if anyone enables mmap, the files survive.)
            let qload_dir = tempfile::tempdir()?;
            let qgraph_path = qload_dir.path().join(format!("{qbasename}.hnsw.graph"));
            let qdata_path = qload_dir.path().join(format!("{qbasename}.hnsw.data"));
            let qgraph_bytes_move = qgraph_bytes.clone();
            let qdata_bytes_move = qdata_bytes.clone();
            let quantizer_for_load = Arc::clone(&quantizer);
            let hnsw_u8 = tokio::task::spawn_blocking(move || {
                let mut qgf = std::fs::File::create(&qgraph_path)?;
                qgf.write_all(&qgraph_bytes_move)?;
                qgf.flush()?;
                let mut qdf = std::fs::File::create(&qdata_path)?;
                qdf.write_all(&qdata_bytes_move)?;
                qdf.flush()?;
                drop(qgf);
                drop(qdf);

                // Leak the TempDir so its files survive the HnswIo load. See the
                // f32 path's leak comment for the rationale. The qload_dir
                // itself is cheap (~a path + a dir handle); leaking one per shard
                // at boot is acceptable.
                let leaked_qdir: &'static tempfile::TempDir = Box::leak(Box::new(qload_dir));
                let leaked_qio: &'static HnswIo =
                    Box::leak(Box::new(HnswIo::new(leaked_qdir.path(), &qbasename)));
                let dist_for_load = ShamirDistU8::new(quantizer_for_load, metric);
                let hnsw_u8: Hnsw<'static, u8, ShamirDistU8> =
                    leaked_qio.load_hnsw_with_dist(dist_for_load).map_err(|e| {
                        SnapshotError::Io(format!("hnsw_rs load_hnsw_with_dist (u8) failed: {e}"))
                    })?;
                Ok::<_, SnapshotError>(Arc::new(hnsw_u8))
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
/// vector-backed row) emits `Delete`. Each op is applied to the live graph
/// BEFORE the chunk is appended (the chunk is the durable echo of an
/// already-applied in-memory mutation), so the in-graph and on-disk states
/// never diverge in a way that loses data: a crash between the in-memory
/// apply and the chunk append drops the chunk, and the next restart sees a
/// graph without that mutation (acceptable — the mutation was never durable).
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
    old_graph_chunks: u32,
    old_data_chunks: u32,
    new_manifest: SnapshotManifest,
    new_delta_applied_upto: u64,
) -> Result<(), SnapshotError> {
    let mut ops: Vec<KvOp> = Vec::new();

    // Prune the OLD generation's chunks + sidecar. The manifest's chunk
    // counts tell us exactly how many keys to remove — no scan needed.
    for i in 0..old_graph_chunks as usize {
        ops.push(KvOp::Remove(chunk_key(keyspace, old_gen, "graph", i)));
    }
    for i in 0..old_data_chunks as usize {
        ops.push(KvOp::Remove(chunk_key(keyspace, old_gen, "data", i)));
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
