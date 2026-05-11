//! Micro-bench for `RecordId::new()` — the hot ID-generation
//! call on every insert path.

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};

use shamir_types::types::record_id::RecordId;

fn bench_single_thread(c: &mut Criterion) {
    let mut group = c.benchmark_group("record_id_single");
    group.throughput(Throughput::Elements(1));
    group.bench_function("new", |b| {
        b.iter(|| black_box(RecordId::new()));
    });
    group.finish();
}

criterion_group!(benches, bench_single_thread);
criterion_main!(benches);
