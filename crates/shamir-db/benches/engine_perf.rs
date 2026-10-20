use std::hint::black_box;
use std::sync::Arc;
include!("bench_allocator.rs");
#[path = "engine_perf/common.rs"]
mod common;
use bench_scale_tool::Harness;
use common::*;
use shamir_collections::new_map;
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_db::engine::table::TableConfig;
use shamir_db::ShamirDb;
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl;
use shamir_query_builder::query::Query;
use shamir_query_builder::select;
use shamir_query_builder::write;
use shamir_types::mpack;
use shamir_types::types::value::QueryValue;
use tokio::runtime::Runtime;

fn main() {
    let mut h = Harness::new("engine_perf", env!("CARGO_MANIFEST_DIR"));
    let rt = Runtime::new().unwrap();

    // ── bulk_insert — pure write throughput, fresh empty table per iter ──
    for count in bulk_counts() {
        let id = format!("bulk_insert/{count}");
        h.bench_batched_async(
            &id,
            || async move { fresh_db().await },
            move |shamir| async move {
                let req = req_bulk_insert(0, count);
                shamir.execute("bench", &req).await.unwrap();
            },
        );
    }

    // ── set_existing_no_index / with_index — shared seeded fixture ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let target = format!("u{:08}", n - 1);
        let id = format!("set_existing_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_set_one(&target, 42);
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, true));
        let target = format!("u{:08}", n - 1);
        let id = format!("set_existing_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_set_one(&target, 42);
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── read_by_id_no_index / with_index ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("read_by_id_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_by_id(&format!("u{:08}", n - 1));
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, true));
        let id = format!("read_by_id_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_by_id(&format!("u{:08}", n - 1));
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── read_by_city_no_index / with_index ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("read_by_city_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_by_city("NYC");
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_city", "city", false).await;
            s
        });
        let id = format!("read_by_city_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_by_city("NYC");
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── update_by_id_no_index / with_index ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("update_by_id_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_update_by_id(&format!("u{:08}", n - 1));
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, true));
        let id = format!("update_by_id_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_update_by_id(&format!("u{:08}", n - 1));
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── delete_by_id_no_index / with_index — table shrinks, fresh per iter ──
    for &n in SIZES {
        let id = format!("delete_by_id_no_index/{n}");
        h.bench_batched_async(
            &id,
            move || async move { seeded(n, false).await },
            move |shamir| {
                let req = req_delete_by_id(&format!("u{:08}", n - 1));
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            },
        );
    }
    for &n in SIZES {
        let id = format!("delete_by_id_with_index/{n}");
        h.bench_batched_async(
            &id,
            move || async move { seeded(n, true).await },
            move |shamir| {
                let req = req_delete_by_id(&format!("u{:08}", n - 1));
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                }
            },
        );
    }

    // ── complex_filter ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("complex_filter/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_complex_filter();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── order_limit_top10 (full scan + sort, no index) ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("order_limit_top10/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_with_order_limit();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── count_all_no_filter ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("count_all_no_filter/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_count_all();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── count_with_filter_no_index / with_index ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("count_with_filter_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_count_with_filter("NYC");
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_city", "city", false).await;
            s
        });
        let id = format!("count_with_filter_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_count_with_filter("NYC");
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── min_max_no_index / with_index ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("min_max_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_min_max_score();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_score", "score", false).await;
            s
        });
        let id = format!("min_max_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_min_max_score();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── min_only_no_index / with_index (sorted) ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("min_only_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_min_score();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_sorted_index(&s, "users", "by_score", "score").await;
            s
        });
        let id = format!("min_only_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_min_score();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── order_limit_top10_desc_sorted (sorted index, reverse-iter fast path) ──
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_sorted_index(&s, "users", "by_score", "score").await;
            s
        });
        let id = format!("order_limit_top10_desc_sorted/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_with_order_limit();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── order_limit_top10_asc_sorted (sorted index, forward-iter fast path) ──
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_sorted_index(&s, "users", "by_score", "score").await;
            s
        });
        let id = format!("order_limit_top10_asc_sorted/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_with_order_limit_asc();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── order_limit_top10_with_index (regular index) ──
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            create_index(&s, "users", "by_score", "score", false).await;
            s
        });
        let id = format!("order_limit_top10_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_read_with_order_limit();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ── range_query_no_index / with_index (sorted) ──
    for &n in SIZES {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("range_query_no_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_range_age();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }
    for &n in SIZES {
        let shamir = rt.block_on(async {
            let s = seeded(n, false).await;
            // Sorted index for range queries — equality (hash) index
            // wouldn't help here.
            create_sorted_index(&s, "users", "by_age", "age").await;
            s
        });
        let id = format!("range_query_with_index/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_range_age();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // --------------------------------------------------------------------
    // Sled/disk-backed range bench — exercises the native
    // `iter_range_stream` path on a real disk backend. Fresh tempdir per
    // iter, so `bench_batched_async`.
    //
    // I/O-bound exception: fjall is a persistent LSM-tree — every commit
    // does real disk I/O, so even the /100 cell costs well over the
    // ≲10ms-per-call target (calibrated N=1). Reduced to the smallest
    // cell only (BULK_COUNT); the disk cost is the point of the bench,
    // not something tunable by shrinking N further.
    // --------------------------------------------------------------------

    {
        let count = BULK_COUNT;
        let id = format!("bulk_insert_fjall/{count}");
        h.bench_batched_async(
            &id,
            || async move {
                let tempdir = tempfile::TempDir::new().expect("tempdir");
                let shamir = fresh_db_fjall(tempdir.path()).await;
                (shamir, tempdir)
            },
            move |(shamir, tempdir)| {
                let req = req_bulk_insert(0, count);
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                    drop(shamir);
                    drop(tempdir);
                }
            },
        );
    }

    // ── bulk_insert_membuffer_in_memory — fresh DB per iter (pure wrapper overhead) ──
    for count in bulk_counts() {
        let id = format!("bulk_insert_membuffer_in_memory/{count}");
        h.bench_batched_async(
            &id,
            || async move { fresh_db_membuffer_in_memory().await },
            move |shamir| async move {
                let req = req_bulk_insert(0, count);
                shamir.execute("bench", &req).await.unwrap();
            },
        );
    }

    // ── bulk_insert_membuffer_fjall — fresh tempdir + DB per iter ──
    // I/O-bound exception: same fjall persistent-backend disk cost as
    // `bulk_insert_fjall` above — every commit hits disk, so calibrated N=1
    // is expected and accepted. Kept at the smallest cell only (BULK_COUNT).
    {
        let count = BULK_COUNT;
        let id = format!("bulk_insert_membuffer_fjall/{count}");
        h.bench_batched_async(
            &id,
            || async move {
                let tempdir = tempfile::TempDir::new().expect("tempdir");
                let shamir = fresh_db_membuffer_fjall(tempdir.path()).await;
                (shamir, tempdir)
            },
            move |(shamir, tempdir)| {
                let req = req_bulk_insert(0, count);
                async move {
                    shamir.execute("bench", &req).await.unwrap();
                    drop(shamir);
                    drop(tempdir);
                }
            },
        );
    }

    // ── steady_state_insert — inserts into a fresh MemBuffer DB ──
    h.bench_batched_async(
        "steady_state_insert_500/membuffer_in_memory",
        || async move { fresh_db_membuffer_in_memory().await },
        |shamir| async move {
            let req = req_bulk_insert(0, 500);
            shamir.execute("bench", &req).await.unwrap();
            drop(shamir);
        },
    );

    // ── ttl_sweep — no_ttl vs short-TTL 50k-seed cost ──
    {
        use shamir_db::storage::storage_in_memory::InMemoryStore;
        use shamir_db::storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
        use shamir_db::storage::types::{RecordKey, Store};
        use shamir_types::types::record_id::RecordId;

        h.bench_batched_async(
            "ttl_sweep/no_ttl_seed_2k",
            || async move {
                let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
                let cfg = MemBufferConfig {
                    max_bytes: 100 * 1024 * 1024,
                    max_entries: 100_000,
                    ttl_ms: None,
                    flush_interval_ms: 60_000,
                    flush_batch_size: 256,
                };
                let store: Arc<dyn Store> = Arc::new(MemBufferStore::new(Arc::clone(&inner), cfg));
                let v = bytes::Bytes::copy_from_slice(&[0xAAu8; 80]);
                (store, v)
            },
            |(store, v)| async move {
                for _ in 0..2_000 {
                    let id = RecordId::new();
                    let k = RecordKey::from_slice(id.as_bytes());
                    store.set(k, v.clone()).await.unwrap();
                }
                drop(store);
            },
        );

        h.bench_batched_async(
            "ttl_sweep/ttl_50ms_seed_2k_flush_300ms",
            || async move {
                let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
                let cfg = MemBufferConfig {
                    max_bytes: 100 * 1024 * 1024,
                    max_entries: 100_000,
                    ttl_ms: Some(50),
                    flush_interval_ms: 300,
                    flush_batch_size: 256,
                };
                let store: Arc<dyn Store> = Arc::new(MemBufferStore::new(Arc::clone(&inner), cfg));
                let v = bytes::Bytes::copy_from_slice(&[0xAAu8; 80]);
                (store, v)
            },
            |(store, v)| async move {
                for _ in 0..2_000 {
                    let id = RecordId::new();
                    let k = RecordKey::from_slice(id.as_bytes());
                    store.set(k, v.clone()).await.unwrap();
                }
                drop(store);
            },
        );
    }

    // ── eviction_byte_pressure — seed 8k near cap, time next 1k writes ──
    {
        use shamir_db::storage::storage_in_memory::InMemoryStore;
        use shamir_db::storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
        use shamir_db::storage::types::{RecordKey, Store};
        use shamir_types::types::record_id::RecordId;

        h.bench_batched_async(
            "eviction_byte_pressure/seed_1800_then_insert_200",
            || async move {
                let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
                // ~100 bytes/value × 1800 = ~180k cache footprint.
                // max_bytes 200_000 leaves ~20k headroom (~200 entries) —
                // the first new inserts put us over; eviction kicks in
                // immediately. Scaled down from seed_8k/insert_1k (was
                // ~25ms per single call) so the timed routine stays
                // ≲10ms while still exercising the eviction path on
                // every insert.
                let cfg = MemBufferConfig {
                    max_bytes: 200_000,
                    max_entries: 100_000,
                    ttl_ms: None,
                    flush_interval_ms: 60_000, // effectively no auto-flush
                    flush_batch_size: 256,
                };
                let store: Arc<dyn Store> = Arc::new(MemBufferStore::new(Arc::clone(&inner), cfg));
                let v = bytes::Bytes::copy_from_slice(&[0xAAu8; 80]);
                for _ in 0..1_800 {
                    let id = RecordId::new();
                    let k = RecordKey::from_slice(id.as_bytes());
                    store.set(k, v.clone()).await.unwrap();
                }
                (store, v)
            },
            |(store, v)| async move {
                // Now seeded near cap. Time the next 200 writes — each
                // pushes past the byte cap and forces an eviction.
                for _ in 0..200 {
                    let id = RecordId::new();
                    let k = RecordKey::from_slice(id.as_bytes());
                    store.set(k, v.clone()).await.unwrap();
                }
                drop(store);
            },
        );
    }

    // ── wal_high_qps — 200 single-record batches into fresh MemBuffer DB ──
    h.bench_batched_async(
        "wal_high_qps/200_single_record_batches",
        || async move { fresh_db_membuffer_in_memory().await },
        |shamir| async move {
            let req = req_bulk_insert(0, 1);
            for _ in 0..200 {
                shamir.execute("bench", &req).await.unwrap();
            }
            drop(shamir);
        },
    );

    // ── cache_hit_get — warm MemBuffer cache, 100% hit, random key ──
    {
        use rand::seq::SliceRandom;
        use shamir_db::storage::storage_in_memory::InMemoryStore;
        use shamir_db::storage::storage_membuffer::{MemBufferConfig, MemBufferStore};
        use shamir_db::storage::types::Store;
        use shamir_types::types::record_id::RecordId;

        // Collapsed to the smallest cell (was &[1_000, 2_000]): both
        // calibrated cheaply (~30k iters), but the harness owns repetition
        // count now, so a single scaled axis keeps only its smallest unit.
        let n: usize = 1_000;
        {
            // Build a warmed MemBuffer cache holding `n` keys, all
            // resident (cache size = n, large max_bytes).
            let (store, keys): (Arc<dyn Store>, Vec<shamir_db::storage::types::RecordKey>) = rt
                .block_on(async {
                    let inner: Arc<dyn Store> = Arc::new(InMemoryStore::new());
                    let cfg = MemBufferConfig {
                        max_bytes: 64 * 1024 * 1024,
                        max_entries: n * 2,
                        ttl_ms: None,
                        flush_interval_ms: 500,
                        flush_batch_size: 256,
                    };
                    let store: Arc<dyn Store> =
                        Arc::new(MemBufferStore::new(Arc::clone(&inner), cfg));
                    let mut keys = Vec::with_capacity(n);
                    for i in 0..n {
                        let id = RecordId::new();
                        let key = shamir_db::storage::types::RecordKey::from_slice(id.as_bytes());
                        let value = bytes::Bytes::from(format!("v{i}"));
                        store.set(key.clone(), value).await.unwrap();
                        keys.push(key);
                    }
                    (store, keys)
                });

            // Shuffle keys for uniform-random access pattern.
            let mut rng = rand::rng();
            let mut shuffled = keys.clone();
            shuffled.shuffle(&mut rng);
            let cursor = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

            let id = format!("cache_hit_get/{n}");
            h.bench_async(&id, move || {
                let store = Arc::clone(&store);
                let cursor = Arc::clone(&cursor);
                let i = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let key = shuffled[i % shuffled.len()].clone();
                async move {
                    let _ = store.get(key).await.unwrap();
                }
            });
        }
    }

    // ── batch_multi_read_8 — 8 independent reads in one batch ──
    // Collapsed + tuned down (was &[1_000, 2_000]): each batch fires 8
    // full-table scans, so a single call at n=1_000 cost ~12ms and n=2_000
    // ~25ms — both over the ≲10ms-per-call target. n=500 keeps one full
    // batch (all 8 cities) under budget while preserving the multi-read
    // shape that's the point of this workload.
    let n: usize = 500;
    {
        let shamir = block_on_setup(&rt, seeded(n, false));
        let id = format!("batch_multi_read_8/{n}");
        h.bench_async(&id, move || {
            let shamir = Arc::clone(&shamir);
            let req = req_batch_independent_reads();
            async move {
                shamir.execute("bench", &req).await.unwrap();
            }
        });
    }

    // ═══════════════════════════════════════════════════════════════════
    // Concurrent write contention
    // ═══════════════════════════════════════════════════════════════════

    {
        let shamir = rt.block_on(async {
            let s = Arc::new(ShamirDb::init_memory().await.unwrap());
            s.create_db("bench").await;
            let cfg = RepoConfig::new("main", BoxRepoFactory::in_memory())
                .add_table(TableConfig::new("users"));
            s.add_repo("bench", cfg).await.unwrap();
            s
        });
        for n_writers in [1usize, 2, 4, 8] {
            let shamir = Arc::clone(&shamir);
            let counter = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
            let id = format!("concurrent_inserts/writers_{n_writers}");
            h.bench_async(&id, move || {
                let shamir = Arc::clone(&shamir);
                let counter = Arc::clone(&counter);
                async move {
                    let mut handles = Vec::new();
                    for _w in 0..n_writers {
                        let s = Arc::clone(&shamir);
                        let id = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        handles.push(tokio::spawn(async move {
                            let mut b = Batch::new();
                            b.id(id).try_upsert(
                                "ups",
                                write::upsert("users").key(mpack!({ "id": @(QueryValue::from(id)) })).value(
                                    mpack!({ "id": @(QueryValue::from(id)), "name": @(QueryValue::from(format!("w{id}"))), "score": @(QueryValue::from(id)) }),
                                ),
                            ).unwrap();
                            let req = b.build();
                            s.execute("bench", &req).await.unwrap();
                        }));
                    }
                    for h in handles {
                        h.await.unwrap();
                    }
                }
            });
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // DDL: create_index on a seeded table (rebuild cost) — fresh table per iter
    // ═══════════════════════════════════════════════════════════════════

    for n_records in [50usize, 300] {
        let id = format!("ddl_create_index/records_{n_records}");
        h.bench_batched_async(
            &id,
            move || async move {
                let s = Arc::new(ShamirDb::init_memory().await.unwrap());
                s.create_db("bench").await;
                let cfg = RepoConfig::new("main", BoxRepoFactory::in_memory())
                    .add_table(TableConfig::new("items"));
                s.add_repo("bench", cfg).await.unwrap();
                seed_users_inner(&s, n_records, "items").await;
                s
            },
            move |shamir| async move {
                let idx = ddl::create_index("by_score", "items")
                    .field("score")
                    .build();
                let mut b = Batch::new();
                b.id(1).create_index("idx", idx);
                let req = b.build();
                shamir.execute("bench", &req).await.unwrap();
            },
        );
    }

    // ═══════════════════════════════════════════════════════════════════
    // GROUP BY + aggregation e2e
    // ═══════════════════════════════════════════════════════════════════

    {
        let shamir = block_on_setup(&rt, seeded(1000, false));

        let mut b_sum = Batch::new();
        b_sum.id(1).query(
            "g",
            Query::from("users")
                .select([
                    select::field("city"),
                    select::sum("score", "total_score"),
                    select::count_all("n"),
                ])
                .group_by_many(["city"]),
        );
        let req_group_sum = b_sum.build();

        let mut b_avg = Batch::new();
        b_avg.id(2).query(
            "g",
            Query::from("users")
                .select([select::field("city"), select::avg("score", "avg_score")])
                .group_by_many(["city"]),
        );
        let req_group_avg = b_avg.build();

        {
            let shamir = Arc::clone(&shamir);
            let req = req_group_sum.clone();
            h.bench_async("group_by_e2e/sum_count_by_city_1000", move || {
                let s = Arc::clone(&shamir);
                let req = req.clone();
                async move {
                    black_box(s.execute("bench", &req).await.unwrap());
                }
            });
        }
        {
            let shamir = Arc::clone(&shamir);
            let req = req_group_avg.clone();
            h.bench_async("group_by_e2e/avg_by_city_1000", move || {
                let s = Arc::clone(&shamir);
                let req = req.clone();
                async move {
                    black_box(s.execute("bench", &req).await.unwrap());
                }
            });
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // B1a — changefeed emission overhead
    //
    // Regression guard: overhead of `emit_nontx_changefeed` on a single
    // insert when (a) no subscriber is attached, and (b) one subscriber
    // is holding a `broadcast::Receiver`.
    // ═══════════════════════════════════════════════════════════════════

    {
        let mut b_ins = Batch::new();
        b_ins
            .id("b1")
            .return_flagged()
            .insert("ins", write::insert("users").row(gen_user(999)));
        let req_insert = b_ins.build();

        // (a) no_subscribers.
        {
            let shamir = block_on_setup(&rt, seeded(100, false));
            let req = req_insert.clone();
            h.bench_async("changefeed_overhead/no_subscribers", move || {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        }
        // (b) with_subscriber — one live broadcast::Receiver held for the
        //     duration; never polled (measures send-side cost only).
        {
            let shamir = block_on_setup(&rt, seeded(100, false));
            let _subscriber = rt
                .block_on(shamir.subscribe_changelog("bench", "main"))
                .expect("subscribe succeeds")
                .expect("benchmark repo exists");
            let req = req_insert.clone();
            h.bench_async("changefeed_overhead/with_subscriber", move || {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        }
    }
    // ═══════════════════════════════════════════════════════════════════
    // B1b — validator dispatch overhead

    {
        use shamir_db::engine::validator::WriteOp;

        let mut b_ins = Batch::new();
        b_ins
            .id("b1")
            .return_flagged()
            .insert("ins", write::insert("users").row(gen_user(999)));
        let req_insert = b_ins.build();

        // (a) no_validators.
        {
            let shamir = block_on_setup(&rt, seeded(100, false));
            let req = req_insert.clone();
            h.bench_async("validator_overhead/no_validators", move || {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        }

        // (b) one_validator — minimal WASM module (no exported functions,
        //     vacuous accept) bound to `users` for Insert ops.
        {
            const NOOP_VALIDATOR_WAT: &str = r#"
(module
  (memory (export "memory") 2)
  (global $bump (mut i32) (i32.const 1024))
  (data (i32.const 512) "\c0")
  (func (export "shamir_alloc") (param $len i32) (result i32)
    (local $ptr i32)
    (local.set $ptr (global.get $bump))
    (global.set $bump (i32.add (global.get $bump) (local.get $len)))
    (local.get $ptr)
  )
  (func (export "shamir_call") (param $ptr i32) (param $len i32) (result i64)
    (i64.or
      (i64.shl (i64.const 512) (i64.const 32))
      (i64.const 1)
    )
  )
)
"#;
            let shamir = rt.block_on(async {
                let s = seeded(100, false).await;
                let noop_wasm = wat::parse_str(NOOP_VALIDATOR_WAT).unwrap();
                s.create_validator_from_wasm("v_bench_noop", &noop_wasm, false)
                    .await
                    .expect("create validator");
                s.bind_validator(
                    "bench",
                    "main",
                    "users",
                    "v_bench_noop",
                    vec![WriteOp::Insert],
                    1000,
                )
                .await
                .expect("bind validator");
                s
            });
            let req = req_insert.clone();
            h.bench_async("validator_overhead/one_validator", move || {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        }
    }

    // ═══════════════════════════════════════════════════════════════════
    // P7 — nested-batch overhead
    //
    // Measures the overhead introduced by `BatchOp::Batch` (sub_batch) —
    // bind-resolution, recursive execution, and result-wrapping — versus
    // the same logical work expressed as sibling ops in a flat batch.
    // ═══════════════════════════════════════════════════════════════════

    {
        const N: usize = 1_000;
        const TARGET: &str = "u00000999";
        const SCORE: i64 = 42;

        let shamir = block_on_setup(&rt, seeded(N, false));

        // ── flat request ───────────────────────────────────────────────
        let req_flat = {
            let mut b = Batch::new();
            b.id("flat_nb").return_flagged();
            b.query("user", Query::from("users").where_eq("id", TARGET));
            b.try_upsert(
                "write",
                write::upsert("users")
                    .key(mpack!({ "id": @(QueryValue::from(TARGET)) }))
                    .value(mpack!({ "id": @(QueryValue::from(TARGET)), "score": @(QueryValue::from(SCORE)), "name": "Bench", "active": true })),
            ).unwrap();
            b.build()
        };

        // ── nested request ─────────────────────────────────────────────
        let req_nested = {
            let inner = {
                let mut ib = Batch::new();
                ib.id("inner_nb").return_flagged();
                ib.try_upsert(
                    "write",
                    write::upsert("users").key(mpack!({ "id": @(QueryValue::from(TARGET)) })).value(
                        mpack!({ "id": @(QueryValue::from(TARGET)), "score": @(QueryValue::from(SCORE)), "name": "Bench", "active": true }),
                    ),
                ).unwrap();
                ib.build()
            };

            let mut ob = Batch::new();
            ob.id("nested_nb").return_flagged();
            let user_handle = ob.query("user", Query::from("users").where_eq("id", TARGET));
            let uid_ref = user_handle.first().field("id");
            let mut bind = new_map();
            bind.insert("uid".to_string(), uid_ref);
            ob.sub_batch("proc", inner, bind);
            ob.build()
        };

        {
            let shamir = Arc::clone(&shamir);
            let req = req_flat.clone();
            h.bench_async("nested_batch/flat", move || {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        }

        {
            let shamir = Arc::clone(&shamir);
            let req = req_nested.clone();
            h.bench_async("nested_batch/nested", move || {
                let s = Arc::clone(&shamir);
                let r = req.clone();
                async move {
                    s.execute("bench", &r).await.unwrap();
                }
            });
        }
    }

    h.run();
}
