//! Streaming snapshot dump parity and transaction-boundary regressions.

use crate::kind::{VectorMetric, VectorQuantization};
use crate::meta_envelope::MetaEnvelope;
use crate::vector::adapter::VectorAdapter;
use crate::vector::hnsw_adapter::{await_fitted, HnswAdapter, HnswConfig};
use crate::vector::snapshot::codec::{chunk_key, manifest_key, sidecar_key, ChunkHeader};
use crate::vector::snapshot::manifest::{integrity_bytes, integrity_key};
use crate::vector::snapshot::{
    dump_snapshot_with_gen, load_snapshot, SnapshotError, SnapshotManifest,
};
use async_trait::async_trait;
use bytes::Bytes;
use futures::Stream;
use shamir_storage::error::{DbError, DbResult};
use shamir_storage::storage_in_memory::InMemoryStore;
use shamir_storage::types::{KvOp, RecordKey, Store};
use shamir_types::types::record_id::RecordId;
use std::pin::Pin;
use std::sync::Arc;

type TestStream = Pin<Box<dyn Stream<Item = DbResult<Vec<(RecordKey, Bytes)>>> + Send>>;
const KEYSPACE: &str = "snapshot.streaming";

struct FailMetadataStore {
    inner: Arc<dyn Store>,
}

#[async_trait]
impl Store for FailMetadataStore {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> TestStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> TestStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        if ops
            .iter()
            .any(|op| matches!(op, KvOp::Set(key, _) if *key == manifest_key(KEYSPACE)))
        {
            return Err(DbError::Storage(
                "injected before manifest publication".into(),
            ));
        }
        self.inner.transact(ops).await
    }
}

fn rid(i: u16) -> RecordId {
    let mut bytes = [0; 16];
    bytes[..2].copy_from_slice(&i.to_le_bytes());
    RecordId(bytes)
}

async fn adapter() -> HnswAdapter {
    let adapter = HnswAdapter::new_with_quantization(
        2,
        VectorMetric::L2,
        HnswConfig::default(),
        Some(VectorQuantization::Sq8),
    );
    let items: Vec<_> = (0..256)
        .map(|i| (rid(i), vec![i as f32, (i % 13) as f32]))
        .collect();
    adapter.upsert_batch(&items).await.unwrap();
    await_fitted(&adapter).await;
    adapter
}

async fn manifest(store: &Arc<dyn Store>) -> SnapshotManifest {
    let bytes = store.get(manifest_key(KEYSPACE)).await.unwrap();
    MetaEnvelope::open(&bytes).unwrap()
}

#[tokio::test]
async fn streamed_quantized_chunks_preserve_header_and_crc() {
    let adapter = adapter().await;
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    dump_snapshot_with_gen(&adapter, &store, KEYSPACE, 0)
        .await
        .unwrap();
    let manifest = manifest(&store).await;
    for (section, count) in [
        ("graph", manifest.graph_chunks),
        ("data", manifest.data_chunks),
        ("qgraph", manifest.qgraph_chunks),
        ("qdata", manifest.qdata_chunks),
    ] {
        for index in 0..count {
            let key = chunk_key(KEYSPACE, manifest.gen, section, index as usize);
            let encoded = store.get(key).await.unwrap();
            let header: ChunkHeader = bincode::deserialize(&encoded).unwrap();
            assert_eq!(header.idx, index);
            assert_eq!(header.crc32, crc32fast::hash(&header.bytes));
        }
    }
    let metadata = store.get(manifest_key(KEYSPACE)).await.unwrap();
    let sidecar = store
        .get(sidecar_key(KEYSPACE, manifest.gen))
        .await
        .unwrap();
    assert_eq!(
        store.get(integrity_key(KEYSPACE)).await.unwrap(),
        integrity_bytes(&metadata, &sidecar)
    );
}

#[tokio::test]
async fn failed_before_manifest_keeps_previous_generation_loadable() {
    let adapter = adapter().await;
    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    dump_snapshot_with_gen(&adapter, &inner, KEYSPACE, 0)
        .await
        .unwrap();
    let before = manifest(&inner).await;
    let failing: Arc<dyn Store> = Arc::new(FailMetadataStore {
        inner: Arc::clone(&inner),
    });
    assert!(dump_snapshot_with_gen(&adapter, &failing, KEYSPACE, 1)
        .await
        .is_err());
    assert_eq!(manifest(&inner).await.gen, before.gen);
    assert!(load_snapshot(&inner, KEYSPACE).await.is_ok());
}

#[tokio::test]
async fn missing_and_corrupt_chunks_remain_corrupt_errors() {
    let adapter = adapter().await;
    let store: Arc<dyn Store> = Arc::new(InMemoryStore::new());
    dump_snapshot_with_gen(&adapter, &store, KEYSPACE, 0)
        .await
        .unwrap();
    let manifest = manifest(&store).await;
    let key = chunk_key(KEYSPACE, manifest.gen, "qgraph", 0);
    let encoded = store.get(key.clone()).await.unwrap();
    store.remove(key.clone()).await.unwrap();
    assert!(matches!(
        load_snapshot(&store, KEYSPACE).await,
        Err(SnapshotError::Corrupt(_))
    ));
    store.set(key.clone(), encoded.clone()).await.unwrap();
    let mut header: ChunkHeader = bincode::deserialize(&encoded).unwrap();
    header.bytes[0] ^= 1;
    store
        .set(key, bincode::serialize(&header).unwrap().into())
        .await
        .unwrap();
    assert!(matches!(
        load_snapshot(&store, KEYSPACE).await,
        Err(SnapshotError::Corrupt(_))
    ));
}
