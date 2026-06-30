//! Benchmark for the Shomer authorization gate (P4).
//!
//! Measures two paths:
//!   1. System fast-path (admin bypass) — the common live path.
//!   2. Non-System path with meta resolution + ancestor traversal
//!      (a Table 3 levels deep under a Database).
//!
//! Run:
//!   cargo bench -p shamir-db -- 'authorize'

use criterion::{criterion_group, criterion_main, Criterion};

#[global_allocator]
static GLOBAL: sefer_alloc::SeferAlloc = sefer_alloc::SeferAlloc::new();
use tokio::runtime::Runtime;

use shamir_db::access::{Action, Actor, ResourcePath};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::shamir_db::ShamirDb;

async fn setup_shamir() -> ShamirDb {
    let shamir = ShamirDb::init_memory().await.unwrap();
    shamir.create_db("benchdb").await;
    let config =
        RepoConfig::new("data", BoxRepoFactory::in_memory()).add_table(TableConfig::new("records"));
    shamir.add_repo("benchdb", config).await.unwrap();
    shamir
}

fn bench_authorize(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let shamir = rt.block_on(setup_shamir());

    let mut group = c.benchmark_group("authorize_gate");

    // 1. System fast-path (admin bypass).
    group.bench_function("system_bypass", |b| {
        b.to_async(&rt).iter(|| {
            let shamir = shamir.clone();
            async move {
                shamir
                    .authorize_access(
                        &Actor::System,
                        &ResourcePath::table("benchdb", "data", "records"),
                        Action::Read,
                    )
                    .await
                    .unwrap();
            }
        })
    });

    // 2. Non-System path with traversal (3 ancestors + target).
    group.bench_function("user_traverse_table", |b| {
        b.to_async(&rt).iter(|| {
            let shamir = shamir.clone();
            async move {
                shamir
                    .authorize_access(
                        &Actor::User(42),
                        &ResourcePath::table("benchdb", "data", "records"),
                        Action::Read,
                    )
                    .await
                    .unwrap();
            }
        })
    });

    // 3. Non-System path on a deep resource (Record — 4 ancestors).
    group.bench_function("user_traverse_record", |b| {
        b.to_async(&rt).iter(|| {
            let shamir = shamir.clone();
            async move {
                shamir
                    .authorize_access(
                        &Actor::User(42),
                        &ResourcePath::record("benchdb", "data", "records", "key1"),
                        Action::Read,
                    )
                    .await
                    .unwrap();
            }
        })
    });

    group.finish();
}

criterion_group!(benches, bench_authorize);
criterion_main!(benches);
