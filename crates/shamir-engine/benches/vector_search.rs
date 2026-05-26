//! Vector search benchmarks: HNSW vs BruteForce.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use shamir_engine::index2::kind::VectorMetric;
use shamir_engine::index2::vector::adapter::VectorAdapter;
use shamir_engine::index2::vector::brute_force::BruteForceAdapter;
use shamir_engine::index2::vector::hnsw_adapter::{HnswAdapter, HnswConfig};
use shamir_types::types::record_id::RecordId;

fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn random_vec(dim: usize, seed: u64) -> Vec<f32> {
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

fn rid_from(i: usize) -> RecordId {
    let mut a = [0u8; 16];
    a[8..16].copy_from_slice(&(i as u64).to_be_bytes());
    RecordId(a)
}

fn bench_vector(c: &mut Criterion) {
    let rt = rt();
    let dim = 128;

    for &n in &[1_000usize, 10_000] {
        let mut group = c.benchmark_group(format!("vector_search_{n}"));
        group.throughput(Throughput::Elements(1));

        // Build BruteForce
        let brute = rt.block_on(async {
            let a = BruteForceAdapter::new(dim as u32, VectorMetric::Cosine);
            for i in 0..n {
                a.upsert(rid_from(i), &random_vec(dim, i as u64))
                    .await
                    .unwrap();
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            a
        });

        // Build HNSW
        let hnsw = rt.block_on(async {
            let a = HnswAdapter::new(
                dim as u32,
                VectorMetric::Cosine,
                HnswConfig {
                    max_elements: n + 1000,
                    m: 16,
                    max_layer: 16,
                    ef_construction: 200,
                    ef_search: 50,
                },
            );
            for i in 0..n {
                a.upsert(rid_from(i), &random_vec(dim, i as u64))
                    .await
                    .unwrap();
            }
            a
        });

        let query = random_vec(dim, 999_999);

        group.bench_with_input(BenchmarkId::new("brute_force", n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let q = query.clone();
                let a = &brute;
                async move { a.search(&q, 10).await.unwrap() }
            });
        });

        group.bench_with_input(BenchmarkId::new("hnsw", n), &n, |b, _| {
            b.to_async(&rt).iter(|| {
                let q = query.clone();
                let a = &hnsw;
                async move { a.search(&q, 10).await.unwrap() }
            });
        });

        group.finish();
    }
}

criterion_group!(benches, bench_vector);
criterion_main!(benches);
