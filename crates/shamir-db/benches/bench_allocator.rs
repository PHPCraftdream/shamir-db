// Bench allocator switch — `include!` this from every bench binary.
//
//   cargo bench -p shamir-db --features bench-sefer          # sefer defaults
//   cargo bench -p shamir-db --features bench-sefer-tuned    # sefer + production LargeCacheConfig
//   cargo bench -p shamir-db --features bench-mimalloc       # mimalloc
//   cargo bench -p shamir-db                                 # system default

#[cfg(all(feature = "bench-sefer", not(feature = "bench-sefer-tuned")))]
#[global_allocator]
static GLOBAL: sefer_alloc::SeferAlloc = sefer_alloc::SeferAlloc::new();

#[cfg(feature = "bench-sefer-tuned")]
#[global_allocator]
static GLOBAL: sefer_alloc::SeferAlloc = sefer_alloc::SeferAlloc::with_config({
    sefer_alloc::LargeCacheConfig::new()
        .budget_bytes(2 * 1024 * 1024 * 1024)
        .headroom_bytes(512 * 1024 * 1024)
        .decay_interval_ms(500)
        .decay_rate_percent(25)
        .mode(sefer_alloc::LargeCacheMode::Lazy)
});

#[cfg(feature = "bench-mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;
