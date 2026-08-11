//! Integration tests for the `doctor` CLI subcommand.
//!
//! Tests the full offline path: open `data_dir`, replay wire-created tables,
//! verify/repair, and return a [`DoctorReport`]. Every test observes the
//! actual report data (table count, health) rather than just `is_ok()`.

use std::path::{Path, PathBuf};

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::doctor::DoctorArgs;
use shamir_server::server::{BootstrapMode, ServerLauncher};
use zeroize::Zeroizing;

fn make_test_config(data_dir: &Path) -> Config {
    Config {
        data_dir: data_dir.to_path_buf(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        kdf_defaults: KdfConfig {
            memory_kb: 19_456,
            time: 2,
            parallelism: 1,
            argon2_version: 0x13,
        },
        argon2_concurrent_max: 4,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            addr: "127.0.0.1:0".to_string(),
            profile: ProfileKind::TlsExporter,
            path: None,
            kdf_override: None,
            browser_origin_allowlist: vec![],
        }],
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("key.pem"),
        },
        security: Default::default(),
        audit: Default::default(),
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

/// Boot a server, create a table over the wire, shut down, and return the
/// `data_dir` (as a kept-alive `TempDir` + its path).
///
/// The resulting dir contains:
/// - `shamir_db_meta.redb` — system store (`default` db, `main` repo)
/// - `wire_tables.mpack` — registry listing `default.main → ["test_table"]`
/// - Fjall data files for the table
///
/// The server is fully shut down before returning, so the redb single-writer
/// lock is released and doctor can reopen the dir directly.
async fn setup_data_with_table() -> (tempfile::TempDir, PathBuf) {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    let temp = tempfile::TempDir::new().expect("tempdir");
    let config = make_test_config(temp.path());

    // Boot server and create a table.
    let admin_pw = b"admin-password".to_vec();
    let bootstrap = BootstrapMode::Password {
        username: "admin".into(),
        password: Zeroizing::new(admin_pw),
    };
    let launcher = ServerLauncher {
        config: config.clone(),
        bootstrap,
    };
    let handle = launcher.launch().await.expect("launcher boot");
    let server_addr = handle.first_tls_exporter_addr().expect("bound address");

    let client = shamir_client::Client::connect(shamir_client::ConnectOptions {
        addr: server_addr,
        server_name: "localhost".to_string(),
        username: "admin".to_string(),
        password: Zeroizing::new(b"admin-password".to_vec()),
        accept_new_host: true,
        trusted_pin: None,
        connect_timeout: None,
        request_timeout: None,
    })
    .await
    .expect("admin connect");

    let mut batch = shamir_query_builder::batch::Batch::new();
    batch.id(1).create_table(
        "test_table",
        shamir_query_builder::ddl::create_table("test_table"),
    );
    client.execute("default", batch.build()).await.unwrap();
    drop(client);

    // Shutdown server — releases the redb single-writer lock.
    handle.shutdown().await;

    let data_dir = temp.path().to_path_buf();
    (temp, data_dir)
}

/// Open the data dir, register the wire-created table (mirroring what
/// `doctor::run` does via `TablesRegistry` replay), corrupt its record
/// counter to a bogus value, then drop everything so doctor can reopen.
///
/// Mirrors the `counter().set_to(999)` corruption pattern from
/// `crates/shamir-engine/src/table/tests/doctor_tests.rs::repair_heals_drifted_counter`.
async fn corrupt_table_counter(data_dir: &Path) {
    use shamir_db::shamir_db::SystemStoreConfig;
    use shamir_db::ShamirDb;

    let meta_path = data_dir.join("shamir_db_meta.redb");
    let shamir = ShamirDb::init(SystemStoreConfig::Fjall(meta_path))
        .await
        .expect("open ShamirDb for corruption");

    let db = shamir.get_db("default").expect("default db exists");
    // The table was created over the wire; its config is in-memory-only
    // so we must re-register it before we can open the TableManager.
    if !db.has_table("main", "test_table") {
        db.create_table("main", "test_table")
            .expect("create_table for corruption");
    }
    let repo = db.get_repo("main").expect("main repo exists");
    let table_mgr = repo
        .get_table("test_table")
        .await
        .expect("open table for corruption");

    // Corrupt: set counter to a value that cannot match the actual record
    // count (0 records exist), making verify() report counter_consistent=false.
    table_mgr
        .counter()
        .set_to(999)
        .await
        .expect("corrupt counter");

    // Drop ShamirDb — releases file handles so doctor can reopen.
    drop(table_mgr);
    drop(repo);
    drop(db);
    drop(shamir);
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

/// Scenario 1: Empty data dir (no filter) → succeeds with zero tables.
/// An empty database is not unhealthy — exit 0 is correct here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn doctor_empty_data_dir_succeeds() {
    let temp = tempfile::TempDir::new().expect("tempdir");
    let config = make_test_config(temp.path());

    let args = DoctorArgs::default();
    let report = shamir_server::doctor::run(&config, &args)
        .await
        .expect("doctor should succeed on empty data_dir");
    assert_eq!(report.total_tables, 0);
    assert!(report.healthy);
}

/// ⚠️ Scenario 2: Data dir with a wire-created table → doctor must SEE it.
///
/// This test asserts `total_tables > 0` rather than just `is_ok()` — the
/// old test passed for the wrong reason (empty-tables branch returned
/// `Ok(())` regardless of whether tables existed).
///
/// Note: wire-created tables are persisted in BOTH the system store (via
/// `system_store.save_table`) and `wire_tables.mpack`. The TablesRegistry
/// replay in `doctor::run` is defense-in-depth mirroring the server boot
/// path — even if the system-store path were to fail (it only logs a
/// warning on error), the replay ensures the table is visible.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn doctor_with_table_succeeds() {
    let (_guard, data_dir) = setup_data_with_table().await;

    let config = make_test_config(&data_dir);
    let args = DoctorArgs::default();
    let report = shamir_server::doctor::run(&config, &args)
        .await
        .expect("doctor should succeed with a healthy table");

    assert!(
        report.total_tables > 0,
        "doctor must see the wire-created table — \
         found {} table(s) (did TablesRegistry replay run?)",
        report.total_tables
    );
    assert_eq!(report.tables[0].table, "test_table");
    assert_eq!(report.tables[0].db, "default");
    assert_eq!(report.tables[0].repo, "main");
    assert!(report.healthy, "a freshly created table must be healthy");
    assert!(
        report.tables[0].verify.counter_consistent,
        "counter must be consistent on a fresh table"
    );
}

/// Scenario 3: Filter options match the wire-created table.
///
/// The real production repo is named `"main"` (server_launcher.rs:405),
/// not `"default"` — the old test used the wrong name and could never match.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn doctor_filter_options_work() {
    let (_guard, data_dir) = setup_data_with_table().await;

    let config = make_test_config(&data_dir);
    let args = DoctorArgs {
        db: Some("default".to_string()),
        repo: Some("main".to_string()),
        table: Some("test_table".to_string()),
        ..Default::default()
    };
    let report = shamir_server::doctor::run(&config, &args)
        .await
        .expect("doctor with matching filters should succeed");

    assert_eq!(report.total_tables, 1);
    assert_eq!(report.tables[0].table, "test_table");
    assert_eq!(report.tables[0].repo, "main");
}

/// Scenario 4: JSON output structure is valid.
///
/// Uses `DoctorReport::to_json` (the same code path `run` uses for
/// `--json`/`--pretty`) rather than capturing stdout — no established
/// stdout-capture pattern exists in this crate.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn doctor_json_output_works() {
    let (_guard, data_dir) = setup_data_with_table().await;

    let config = make_test_config(&data_dir);
    let args = DoctorArgs {
        json: true,
        ..Default::default()
    };
    let report = shamir_server::doctor::run(&config, &args)
        .await
        .expect("doctor --json should succeed");

    // Validate the JSON structure that `run` serializes for --json.
    let json = report.to_json(false).expect("json serialize");
    let parsed: serde_json::Value = serde_json::from_str(&json).expect("valid json");

    assert_eq!(parsed["total_tables"], 1, "JSON must report 1 table");
    assert_eq!(parsed["healthy"], true, "JSON must report healthy");

    let table = &parsed["tables"][0];
    assert_eq!(table["table"], "test_table");
    assert_eq!(table["db"], "default");
    assert_eq!(table["repo"], "main");
    assert!(
        table["healthy"].is_boolean(),
        "healthy field must be present"
    );
    assert!(
        table["verify"]["counter_consistent"].is_boolean(),
        "counter_consistent must be present"
    );
    assert_eq!(
        table["verify"]["records_in_data"], 0,
        "no records were inserted"
    );
}

/// ⚠️ Scenario 5: Explicit filter matching nothing → non-zero exit (`Err`).
///
/// This test FAILS on unfixed HEAD because the old `table_reports.is_empty()`
/// branch returned `Ok(())` regardless of whether filters were set.  On fixed
/// HEAD an explicit filter matching zero tables returns `Err`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn doctor_filter_no_match_returns_err() {
    let (_guard, data_dir) = setup_data_with_table().await;

    let config = make_test_config(&data_dir);
    let args = DoctorArgs {
        table: Some("this_table_does_not_exist".to_string()),
        ..Default::default()
    };
    let result = shamir_server::doctor::run(&config, &args).await;
    assert!(
        result.is_err(),
        "an explicit filter matching zero tables must be non-zero, got Ok: {:?}",
        result
    );
}

/// Scenario 6: A corrupted (unhealthy) table produces a non-zero exit
/// without `--apply`.
///
/// Corruption mirrors the `counter().set_to(999)` pattern from
/// `crates/shamir-engine/src/table/tests/doctor_tests.rs::repair_heals_drifted_counter`:
/// the persisted counter is set to a value that cannot match the actual record
/// count, so `verify()` reports `counter_consistent = false` → unhealthy.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn doctor_corrupted_table_returns_err() {
    let (_guard, data_dir) = setup_data_with_table().await;

    // Corrupt the table's record counter (persisted write-through).
    corrupt_table_counter(&data_dir).await;

    let config = make_test_config(&data_dir);
    let args = DoctorArgs::default();
    let result = shamir_server::doctor::run(&config, &args).await;
    assert!(
        result.is_err(),
        "a corrupted table must produce non-zero exit, got Ok: {:?}",
        result
    );
}
