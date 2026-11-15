//! BOOT-COST-2 measurement: where does the time of one system-store open go?
//!
//! Prints one `BOOT-COST2` line per phase (wall ms and the value of
//! `FJALL_PERSIST_SYNC_ALL_COUNT` after the phase). Asserts nothing about
//! timing; the numbers are read from the CI log. Design and hypotheses:
//! `docs/dev-artifacts/perf/boot-cost.md` section 8.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::engine::db_instance::db_instance::DbInstance;
use crate::engine::function::{EnvPolicy, FunctionRegistry, GlobalVars, WasmEngine};
use crate::engine::repo::{BoxRepoFactory, DurabilityPolicy, RepoConfig, RepoFactory};
use crate::engine::table::TableConfig;
use crate::engine::validator::ValidatorRegistry;
use crate::shamir_db::system_store::SystemStore;
use crate::shamir_db::{ShamirDb, SystemStoreConfig};
use crate::storage::storage_fjall::{reset_persist_sync_all_count, FJALL_PERSIST_SYNC_ALL_COUNT};
use crate::storage::types::Repo;

/// The 13 catalogue tables the system repo is configured with
/// (`system_store/store.rs` lines 171-184).
const SYSTEM_TABLES: [&str; 13] = [
    "databases",
    "repositories",
    "tables",
    "settings",
    "users",
    "roles",
    "functions",
    "groups",
    "validators",
    "function_folders",
    "replication_profiles",
    "publications",
    "subscriptions",
];

/// Tables read at boot, in the real order: `groups` (end of
/// `SystemStore::init_with_durability`), then `databases`, `repositories`,
/// `tables` (`ShamirDb::init_inner`).
const BOOT_READ_TABLES: [&str; 4] = ["groups", "databases", "repositories", "tables"];

fn report(phase: &str, started: Instant, extra: &str) -> Duration {
    let elapsed = started.elapsed();
    println!(
        "BOOT-COST2 phase={} ms={} persist_sync_all={} {}",
        phase,
        elapsed.as_millis(),
        FJALL_PERSIST_SYNC_ALL_COUNT.load(Ordering::Acquire),
        extra
    );
    elapsed
}

fn system_repo_config(path: PathBuf) -> RepoConfig {
    let mut config = RepoConfig::new("system", BoxRepoFactory::fjall(path));
    for table in SYSTEM_TABLES {
        config = config.add_table(TableConfig::new(table));
    }
    config
}

async fn reopen_with_retry(path: PathBuf) -> (ShamirDb, usize) {
    for attempt in 1..=100 {
        if let Ok(shamir) = ShamirDb::init(SystemStoreConfig::Fjall(path.clone())).await {
            return (shamir, attempt);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("system store still locked after retries");
}

#[tokio::test]
#[ignore = "measurement: prints timings, asserts nothing (boot-cost.md section 8)"]
async fn boot_cost2_system_store_open_phases() {
    reset_persist_sync_all_count();

    // A. Reference on its own directory: raw Database open + 16 keyspace
    // opens, the section 7 shape, so this run is comparable with it.
    let dir_a = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let repo_a = BoxRepoFactory::fjall(dir_a.path().join("raw"))
        .create()
        .await
        .unwrap();
    report("A1_raw_factory_create", started, "");
    let mut names: Vec<String> = vec![
        "__tx__".to_string(),
        "__changelog__".to_string(),
        "__interner__".to_string(),
        "__seed_schema__".to_string(),
    ];
    for table in BOOT_READ_TABLES {
        for prefix in ["__data__", "__info__", "__history__"] {
            names.push(format!("{}{}", prefix, table));
        }
    }
    let started = Instant::now();
    for name in &names {
        repo_a.store_get(name).await.unwrap();
    }
    report("A2_raw_16_keyspace_opens", started, "");
    drop(repo_a);

    // B. The open of SystemStore::init_with_durability replayed step by step
    // on a fresh directory (store.rs lines 157-212). Inside
    // add_repo_with_durability the engine steps are not reachable from
    // outside shamir-engine (from_factory_unrecovered, repo_identity_admitted
    // and recover_v2_inflight_admitted are crate-private), so the smallest
    // enclosing public call is timed.
    let dir_b = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let db = DbInstance::new();
    let b1 = report("B1_db_instance_new", started, "");
    let started = Instant::now();
    let config = system_repo_config(dir_b.path().join("meta"));
    let b2 = report("B2_repo_config_13_tables", started, "");
    let started = Instant::now();
    db.add_repo_with_durability(config, DurabilityPolicy::default())
        .await
        .unwrap();
    let b3 = report("B3_add_repo_with_durability", started, "");
    let mut b_sum = b1 + b2 + b3;
    for table in BOOT_READ_TABLES {
        let started = Instant::now();
        db.get_table("system", table).await.unwrap();
        b_sum += report(&format!("B4_first_get_table_{}", table), started, "");
    }
    println!("BOOT-COST2 phase=B_sum ms={}", b_sum.as_millis());
    drop(db);

    // C. The real public-in-crate open of the same thing, fresh directory,
    // then each boot reader separately on the warm store.
    let dir_c = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let store = SystemStore::init_with_durability(
        SystemStoreConfig::Fjall(dir_c.path().join("meta")),
        DurabilityPolicy::default(),
    )
    .await
    .unwrap();
    report("C1_system_store_init_whole", started, "");
    let started = Instant::now();
    store.load_groups().await.unwrap();
    report("C2_load_groups_warm", started, "");
    let started = Instant::now();
    store.load_databases().await.unwrap();
    report("C3_load_databases_warm", started, "");
    let started = Instant::now();
    store.load_repositories().await.unwrap();
    report("C4_load_repositories_warm", started, "");
    let started = Instant::now();
    store.load_tables().await.unwrap();
    report("C5_load_tables_warm", started, "");
    let started = Instant::now();
    store.load_functions().await.unwrap();
    report("C6_load_functions", started, "");
    let started = Instant::now();
    store.load_validators().await.unwrap();
    report("C7_load_validators", started, "");
    drop(store);

    // D. Construction `ShamirDb::init_inner` does around the store
    // (shamir_db/core.rs: wasm engine with disk compile cache, registries).
    let dir_d = tempfile::tempdir().unwrap();
    let started = Instant::now();
    let wasm_engine = WasmEngine::with_compile_cache(dir_d.path().to_path_buf()).unwrap();
    report("D1_wasm_engine_with_compile_cache", started, "");
    let started = Instant::now();
    let functions = FunctionRegistry::with_catalogue_builtins();
    report("D2_function_registry_builtins", started, "");
    let started = Instant::now();
    let globals = GlobalVars::new();
    globals.seed_env(&EnvPolicy::default());
    report("D3_global_vars_seed_env", started, "");
    let started = Instant::now();
    let validators = ValidatorRegistry::new();
    report("D4_validator_registry_new", started, "");
    drop((wasm_engine, functions, globals, validators));

    // E. The public entry point as a whole on a fresh directory, then the
    // first DDL, the flush and the close.
    let dir_e = tempfile::tempdir().unwrap();
    let path_e = dir_e.path().join("meta");
    let started = Instant::now();
    let shamir = ShamirDb::init(SystemStoreConfig::Fjall(path_e.clone()))
        .await
        .unwrap();
    report("E1_shamir_db_init_fresh_whole", started, "");
    let started = Instant::now();
    shamir.create_db("boot_cost2_db").await.unwrap();
    report("E2_create_db_first_ddl", started, "");
    let started = Instant::now();
    shamir.flush_all_waiting().await.unwrap();
    report("E3_flush_all_waiting", started, "");
    let started = Instant::now();
    drop(shamir);
    report("E4_drop", started, "");

    // F. Reopen of the same directory (quiet reopen with one catalogue row).
    let started = Instant::now();
    let (shamir, attempts) = reopen_with_retry(path_e).await;
    report(
        "F1_shamir_db_init_reopen_whole",
        started,
        &format!("attempts={}", attempts),
    );
    drop(shamir);

    let started = Instant::now();
    drop(dir_e);
    drop(dir_d);
    drop(dir_c);
    drop(dir_b);
    drop(dir_a);
    report("G_tempdir_removal_all", started, "");
}
