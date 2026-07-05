//! Startup restore integration tests (V2.2 / #401).
//!
//! Proves the three branches of `VectorBackend::restore_on_open`:
//! 1. **valid snapshot → no rebuild** — a warm restart with a usable
//!    snapshot loads in O(load), `rebuild_count == 0`, and search returns
//!    the same top-k as before the restart.
//! 2. **no snapshot → full rebuild** — a fresh open with no snapshot
//!    falls back to a data-store scan, `rebuild_count == 1`.
//! 3. **mismatched config → warn + rebuild** — a snapshot stamped with
//!    a foreign `format_version` is refused, the open warns and falls back
//!    to a full rebuild, `rebuild_count == 1`, no panic.
//!
//! These tests live in `shamir-index` (next to `VectorBackend`) because
//! they exercise the backend's own restore contract, not the engine's
//! open-path wiring (which is covered by `shamir-engine` integration
//! tests).
//!
//! NOTE on the dump path: `snapshot::dump_snapshot` takes a `&HnswAdapter`
//! (the concrete type), while `VectorBackend` stores its adapter as a
//! `dyn VectorAdapter`. These tests therefore build + dump a standalone
//! `HnswAdapter` in phase 1, then construct a fresh `VectorBackend` (with
//! an EMPTY adapter) in phase 2 and call `restore_on_open` — mirroring
//! exactly what the engine does on a real restart (the dump is a separate,
//! caller-driven action that #402 will wire up; only the LOAD happens
//! automatically on open).

use crate::backend::{IndexBackend, IndexQuery, IndexResult};
use crate::descriptor::IndexDescriptor;
use crate::kind::{IndexKind, VectorBackendRef, VectorConfig, VectorMetric};
use crate::vector::adapter::VectorAdapter;
use crate::vector::hnsw_adapter::{HnswAdapter, HnswConfig};
use crate::vector::snapshot;
use crate::vector::vector_backend::VectorBackend;
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::Store;
use shamir_types::core::interner::{Interner, InternerKey, TouchInd};
use shamir_types::types::common::new_map_wc;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::InnerValue;
use smallvec::SmallVec;
use std::sync::Arc;

// ----------------------------------------------------------------------------
// helpers
// ----------------------------------------------------------------------------

/// Vector dim used by the round-trip tests. Small enough to keep the test
/// fast, large enough that the HNSW graph path is exercised (the adapter
/// switches to brute-force only below ~256 elements, not below a dim).
const DIM: u32 = 16;

/// Number of vectors to insert. > 256 so the HNSW graph (not the
/// brute-force fallback) is the active search path — a snapshot that
/// round-trips through the graph path is the meaningful contract.
const N: usize = 300;

fn intern(i: &Interner, s: &str) -> u64 {
    match i.touch_ind(s).unwrap() {
        TouchInd::Exists(k) | TouchInd::New(k) => k.id(),
    }
}

/// Build a record `{embedding: [f64; dim]}` — the shape `VectorBackend`
/// extracts via `extract_vec` at the `embedding` interned path.
fn make_rec(interner: &Interner, embedding: &[f64]) -> InnerValue {
    let mut m = new_map_wc(1);
    m.insert(
        InternerKey::new(intern(interner, "embedding")),
        InnerValue::List(embedding.iter().map(|f| InnerValue::F64(*f)).collect()),
    );
    InnerValue::Map(m)
}

/// Deterministic LCG pseudo-random vector (mirrors `snapshot_tests`).
fn lcg_vec(dim: usize, seed: u64) -> Vec<f32> {
    let mut v = Vec::with_capacity(dim);
    let mut s = seed;
    for _ in 0..dim {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        v.push(((s >> 33) as f32) / (u32::MAX as f32) - 0.5);
    }
    v
}

/// `RecordId` from a 2-byte label (covers both 1- and 2-byte rid encodings).
fn rid(i: usize) -> RecordId {
    let mut a = [0u8; 16];
    a[14] = (i >> 8) as u8;
    a[15] = (i & 0xFF) as u8;
    RecordId(a)
}

/// Build a `VectorBackend` with the given descriptor id + dim + metric.
///
/// The adapter starts EMPTY — `restore_on_open` is what fills it (either
/// from a snapshot or a data-store scan). The `field_path` is fixed to the
/// interned `embedding` key. Mirrors what `build_index2_backend_*`
/// constructs on the engine open path.
fn make_backend(interner: &Interner, id: u32, dim: u32, metric: VectorMetric) -> VectorBackend {
    let desc = IndexDescriptor::new(
        id,
        format!("vec_idx_{id}"),
        intern(interner, &format!("vec_idx_{id}")),
        SmallVec::new(),
        IndexKind::Vector(Box::new(VectorConfig {
            dim,
            metric,
            backend: VectorBackendRef::InProcessHnsw {
                ef_construct: 200,
                m: 16,
            },
        })),
    );
    let adapter: Arc<dyn VectorAdapter> = Arc::new(HnswAdapter::new(
        dim,
        metric,
        HnswConfig {
            max_elements: 100_000,
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            ..Default::default()
        },
    ));
    VectorBackend::new(desc, vec![intern(interner, "embedding")], adapter)
}

/// Build + populate a standalone `HnswAdapter` (NOT wrapped in a backend)
/// with N LCG vectors. Used in phase 1 of each test to produce a snapshot
/// via `dump_snapshot` (which needs the concrete `HnswAdapter`).
///
/// Returns the adapter (still empty — the CALLER upserts the vectors so
/// it controls the async context) and the list of rids that match the
/// canonical item sequence used across these tests.
fn build_adapter(dim: u32, metric: VectorMetric) -> (HnswAdapter, Vec<RecordId>) {
    let adapter = HnswAdapter::new(
        dim,
        metric,
        HnswConfig {
            max_elements: 100_000,
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            ..Default::default()
        },
    );
    let rids: Vec<RecordId> = (0..N).map(rid).collect();
    (adapter, rids)
}

/// Search a backend for `q` returning the top-k `RecordId` set (scores
/// dropped — the round-trip contract is about WHICH rids survive, not
/// exact float-equality of scores, which can drift across HNSW rebuilds).
async fn topk_ids(backend: &VectorBackend, q: &[f32], k: u32) -> Vec<RecordId> {
    let r = backend
        .lookup(IndexQuery::Vector {
            vec: q.to_vec(),
            k,
            opts: crate::vector::SearchOpts::with_ef_search(64),
        })
        .await
        .unwrap();
    match r {
        IndexResult::Ranked(ranked) => ranked.into_iter().map(|(rid, _)| rid).collect(),
        _ => panic!("expected Ranked result"),
    }
}

// ----------------------------------------------------------------------------
// 1. valid snapshot → rebuild_count == 0, top-k preserved
// ----------------------------------------------------------------------------

#[tokio::test]
async fn restore_on_open_valid_snapshot_skips_rebuild() {
    let i = Interner::new();
    // One info_store carries the snapshot; one data_store carries the
    // rows a fallback rebuild would scan. On the snapshot-hit path the
    // data_store is NEVER read.
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // ---- phase 1: build + populate a standalone adapter, dump it -------
    let id: u32 = 1;
    let (adapter, rids) = build_adapter(DIM, VectorMetric::L2);
    let items: Vec<(RecordId, Vec<f32>)> = rids
        .iter()
        .map(|&r| (r, lcg_vec(DIM as usize, (r.0[15] as u64) * 7 + 1)))
        .collect();
    for (r, v) in &items {
        adapter.upsert(*r, v).await.unwrap();
    }

    // Record the pre-dump top-k for a handful of queries.
    let queries: Vec<Vec<f32>> = (0..5u64).map(|s| lcg_vec(DIM as usize, s + 99)).collect();
    let mut before: Vec<Vec<RecordId>> = Vec::with_capacity(queries.len());
    for q in &queries {
        let r = adapter
            .search(q, 10, crate::vector::SearchOpts::with_ef_search(64), None)
            .await
            .unwrap();
        before.push(r.into_iter().map(|(rid, _)| rid).collect());
    }

    // Dump the live adapter into the info_store under the backend's keyspace.
    let keyspace = format!("__vec_snap__{}", id);
    snapshot::dump_snapshot(&adapter, &info_store, &keyspace)
        .await
        .unwrap();

    // ---- phase 2: simulate a restart — fresh backend, empty adapter -----
    let backend = make_backend(&i, id, DIM, VectorMetric::L2);
    // The fresh backend's adapter is empty: a search now returns nothing.
    assert!(
        topk_ids(&backend, &queries[0], 10).await.is_empty(),
        "fresh backend must be empty before restore_on_open"
    );

    // restore_on_open must take the snapshot branch: load succeeds, no
    // data-store scan.
    backend
        .restore_on_open(Arc::clone(&info_store), Arc::clone(&data_store))
        .await
        .unwrap();

    // The instrumentation proof: rebuild_count == 0 means the snapshot
    // was used, NOT a full scan.
    assert_eq!(
        backend.rebuild_count(),
        0,
        "a successful snapshot load must NOT increment the full-rebuild counter"
    );

    // Top-k fidelity: the same rids survive the round-trip.
    for (q, before_set) in queries.iter().zip(before.iter()) {
        let after = topk_ids(&backend, q, 10).await;
        assert_eq!(
            after.len(),
            before_set.len(),
            "top-k size changed across restart for query {:?}",
            q
        );
        let after_set: shamir_collections::TFxSet<RecordId> = after.into_iter().collect();
        for rid in before_set {
            assert!(
                after_set.contains(rid),
                "rid {:?} present before restart but missing after snapshot load",
                rid
            );
        }
    }
}

// ----------------------------------------------------------------------------
// 2. no snapshot → rebuild_count == 1, data from scan
// ----------------------------------------------------------------------------

#[tokio::test]
async fn restore_on_open_no_snapshot_falls_back_to_rebuild() {
    let i = Interner::new();
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    // The data_store carries the rows the fallback rebuild scans. We
    // populate it directly with serialized records so `rebuild`'s
    // `iter_stream` finds them.
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // Insert N records into the data_store as the rebuild scan expects:
    // key = RecordId bytes (16), value = InnerValue::Map{embedding: [...]}.
    let items: Vec<(RecordId, Vec<f32>)> = (0..N)
        .map(|k| (rid(k), lcg_vec(DIM as usize, k as u64 * 7 + 1)))
        .collect();
    for (r, v) in &items {
        let rec = make_rec(&i, &v.iter().map(|f| *f as f64).collect::<Vec<_>>());
        let val_bytes = rec.to_bytes().unwrap();
        data_store
            .set(r.0.to_vec().into(), val_bytes)
            .await
            .unwrap();
    }

    // Fresh backend, NO snapshot in the info_store.
    let backend = make_backend(&i, 2, DIM, VectorMetric::L2);
    backend
        .restore_on_open(Arc::clone(&info_store), Arc::clone(&data_store))
        .await
        .unwrap();

    // Fallback took the full-scan path: counter == 1.
    assert_eq!(
        backend.rebuild_count(),
        1,
        "no snapshot must fall back to exactly one full rebuild"
    );

    // The rebuild scanned the data_store, so all N vectors are queryable.
    let q = lcg_vec(DIM as usize, 42);
    let top = topk_ids(&backend, &q, 5).await;
    assert_eq!(
        top.len(),
        5,
        "rebuild must populate the graph from the data store"
    );
    // Every returned rid must be one we inserted.
    let inserted: shamir_collections::TFxSet<RecordId> = items.iter().map(|(r, _)| *r).collect();
    for r in &top {
        assert!(
            inserted.contains(r),
            "rebuild returned an unknown rid {:?}",
            r
        );
    }
}

// ----------------------------------------------------------------------------
// 3. snapshot with a tampered format_version → warn + rebuild
// ----------------------------------------------------------------------------

#[tokio::test]
async fn restore_on_open_version_mismatch_warns_and_rebuilds() {
    let i = Interner::new();
    let info_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    let data_store: Arc<dyn Store> = Arc::new(InMemoryStore::new());

    // ---- phase 1: dump a snapshot from a standalone adapter ------------
    let id: u32 = 3;
    let (adapter, rids) = build_adapter(DIM, VectorMetric::L2);
    for r in &rids {
        let v = lcg_vec(DIM as usize, (r.0[15] as u64) * 7 + 1);
        adapter.upsert(*r, &v).await.unwrap();
    }
    let keyspace = format!("__vec_snap__{}", id);
    snapshot::dump_snapshot(&adapter, &info_store, &keyspace)
        .await
        .unwrap();

    // Populate the data_store so the fallback rebuild has rows to scan.
    for r in &rids {
        let v = lcg_vec(DIM as usize, (r.0[15] as u64) * 7 + 1);
        let rec = make_rec(&i, &v.iter().map(|f| *f as f64).collect::<Vec<_>>());
        data_store
            .set(r.0.to_vec().into(), rec.to_bytes().unwrap())
            .await
            .unwrap();
    }

    // ---- phase 2: tamper with the sidecar's format_version -------------
    // load_snapshot checks `sidecar.format_version != SNAPSHOT_FORMAT_VERSION`
    // and returns VersionMismatch. We rewrite the sidecar with a version
    // this build will never accept (u16::MAX), mirroring what a real
    // format bump does.
    let manifest_key = format!("{}.manifest", keyspace);
    let manifest: snapshot::SnapshotManifest = crate::meta_envelope::MetaEnvelope::open(
        &info_store.get(manifest_key.into()).await.unwrap(),
    )
    .unwrap();
    let gen = manifest.gen;
    let sidecar_key = format!("{}.g{}.sidecar", keyspace, gen);
    let sidecar: snapshot::SnapshotSidecar = crate::meta_envelope::MetaEnvelope::open(
        &info_store.get(sidecar_key.clone().into()).await.unwrap(),
    )
    .unwrap();
    let mut tampered = sidecar;
    tampered.format_version = u16::MAX;
    let tampered_bytes = crate::meta_envelope::MetaEnvelope::new(tampered)
        .encode()
        .unwrap();
    info_store
        .set(sidecar_key.into(), tampered_bytes.into())
        .await
        .unwrap();

    // ---- phase 3: open — restore_on_open must warn + fall back ---------
    let backend = make_backend(&i, id, DIM, VectorMetric::L2);
    // Must NOT panic — a corrupt snapshot is recoverable.
    backend
        .restore_on_open(Arc::clone(&info_store), Arc::clone(&data_store))
        .await
        .unwrap();

    // The corrupt snapshot triggered a fallback: counter == 1.
    assert_eq!(
        backend.rebuild_count(),
        1,
        "a version-mismatched snapshot must fall back to a full rebuild"
    );

    // The rebuild scanned the data_store, so the graph is populated.
    let q = lcg_vec(DIM as usize, 7);
    let top = topk_ids(&backend, &q, 5).await;
    assert_eq!(top.len(), 5, "fallback rebuild must populate the graph");
    // No panic — the corrupt snapshot did not abort the open.
}
