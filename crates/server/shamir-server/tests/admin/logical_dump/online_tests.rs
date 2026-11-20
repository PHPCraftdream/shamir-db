//! CI-only integration tests for the ONLINE logical-export job (R3 step 2b,
//! brief `56-r3-2b-online-export-job.md` item 6): a REAL launched server
//! (`ServerLauncher`, SCRAM superuser login through the typed `Client`),
//! `Config.security.export_root` set to an absolute temp sibling directory,
//! and the typed `start_export` → `export_status` → `cancel_export` client
//! methods. No mocks, no raw wire frames — every operation goes through the
//! builder and the typed client.
//!
//! Proven here:
//! * live roundtrip: start → same-session status polls (paced, bounded) →
//!   `ExportPhase::Done` with an opaque `artifact_name`; the artifact
//!   verifies with the existing offline verifier, imports into a fresh
//!   target through the offline import driver, and the imported data reads
//!   back the EXACT expected marker list (not a cumulative count); the
//!   whole raw artifact and its typed content carry NO fixture secret
//!   sentinel (the SCRAM password bytes).
//! * session-only: a second, independent session never observes the job —
//!   it gets the typed `ClientError::Db` with the stable
//!   `online_export_not_found` code, asserted WITHOUT any raw peer-response
//!   debug output (this fixture carries a secret sentinel).
//! * unreadable scope: the export is denied, no job or output is visible,
//!   and a follow-up write still commits.
//! * restart: the same data directory and export root reopen; one stale
//!   temp directory matching the job's own stage pattern is removed at
//!   boot while a nonmatching operator sentinel file is left untouched.
//! * every shutdown's typed `ShutdownReport` is checked clean before any
//!   directory is reopened.
//!
//! Fixture discipline: `argon2_concurrent_max` and
//! `auth_init_rate_per_second` set to 64 (KDF admission), EVERY await
//! bounded (outer budget ≥ 120 s — the Windows CI runner is up to 10x
//! slower), status polls paced at 50 ms (never a tight loop), no fixed
//! sleeps for synchronization, every `TempDir` kept alive for the whole
//! test, and the server handle shut down before any directory is reopened.
//! Everything runs directly on the `#[tokio::test]` runtime — never a
//! nested `Runtime::block_on`, never a dropped `Runtime`.

use std::fs::{self, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::Duration;

use shamir_client::{Client, ClientError, ConnectOptions};
use shamir_db::{ShamirDb, ShutdownReport, SystemStoreConfig};
use shamir_query_builder::batch::Batch;
use shamir_query_builder::ddl::{self, RepoEngine};
use shamir_query_types::admin::ExportPhase;
use shamir_query_types::batch::BatchResponse;
use shamir_query_types::hmac::{canonical_batch_op, compute_tag_hex, derive_session_hmac_key};
use shamir_server::config::{
    Config, LoggingConfig, ObservabilityConfig, ProfileKind, SecurityConfig, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerHandle, ServerLauncher};
use shamir_sync::replication_seed::{verify_dump_file, DumpReader, LogicalValue, PortableItem};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::logical_dump::fixture::import_args;

/// The bootstrapped superuser's name (the server fixture always boots
/// `admin`).
const ADMIN_USER: &str = "admin";

/// The superuser password — also the fixture SECRET SENTINEL: the whole raw
/// artifact and its typed content must never contain these bytes.
const ADMIN_PASSWORD: &[u8] = b"online-export-admin-secret-passphrase";

/// The fixture application database created AS the SCRAM superuser.
const APP_DB: &str = "app";
const APP_REPO: &str = "main";
const APP_TABLE: &str = "rows";

/// Marker values written into the fixture rows; the imported target must
/// read back EXACTLY this list (sorted — a full-scan order is not pinned),
/// never merely a matching count.
const ROW_MARKERS: &[&str] = &[
    "online-export-live-row-marker-a",
    "online-export-live-row-marker-b",
    "online-export-live-row-marker-c",
];

/// Stable wire code the server answers an UNOWNED `ExportStatus` with (the
/// session-scoped registry has no job behind the other session).
const NOT_FOUND_CODE: &str = "online_export_not_found";
const ROOT_LOCK: &str = ".shamir-export-root.lock";

/// End-to-end hang detector, including launch, offline import and teardown.
const FIXTURE_BUDGET: Duration = Duration::from_secs(600);

/// Outer bound for the whole start → Done phase (brief: at least 120 s —
/// the Windows CI runner is up to 10x slower).
const EXPORT_BUDGET: Duration = Duration::from_secs(120);

/// Outer bound for ONE request round trip (status poll, batch execute,
/// start/cancel). Brief floor: 120 s.
const REQUEST_BUDGET: Duration = Duration::from_secs(120);

/// Status poll pacing (brief: pace at 20 ms or more).
const POLL_PACE: Duration = Duration::from_millis(50);

/// Outer bound for one offline import, data reopen, catalogue walk or read.
const DATA_BUDGET: Duration = Duration::from_secs(120);

/// Generous-but-bounded connect/handshake/request timeouts for the typed
/// client's production options.
const CONNECT_BUDGET: Duration = Duration::from_secs(120);

/// Outer bound for one server launch.
const LAUNCH_BUDGET: Duration = Duration::from_secs(120);

/// Outer bound for one graceful server shutdown (the report barrier).
const SHUTDOWN_BUDGET: Duration = Duration::from_secs(120);

/// Spec-floor Argon2id parameters — same shape as `tests/common`.
fn fast_kdf() -> shamir_server::config::KdfConfig {
    shamir_server::config::KdfConfig {
        memory_kb: 19_456,
        time: 2,
        parallelism: 1,
        argon2_version: 0x13,
    }
}

/// Server config for one fixture boot: single TLS-exporter TCP listener on
/// an OS-assigned port, `data_dir` = `data` under the kept-alive temp root,
/// and the online-export service configured with an ABSOLUTE SIBLING export
/// root (`export` under the same temp root — never inside `data_dir` and
/// never an ancestor of it). KDF admission is 64 on both knobs.
fn make_config(temp: &TempDir, export_root: PathBuf) -> Config {
    let data_dir = temp.path().join("data");
    Config {
        data_dir,
        durability: Default::default(),
        transactions: Default::default(),
        tokens: Default::default(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        kdf_defaults: fast_kdf(),
        argon2_concurrent_max: 64,
        listeners: vec![shamir_server::config::ListenerConfig {
            kind: shamir_server::config::ListenerKind::Tcp,
            addr: "127.0.0.1:0".into(),
            profile: Some(ProfileKind::TlsExporter),
            path: None,
            kdf_override: None,
            browser_origin_allowlist: vec![],
            http: Default::default(),
        }],
        tls: TlsConfig {
            cert_path: temp.path().join("data").join("cert.pem"),
            key_path: temp.path().join("data").join("key.pem"),
        },
        security: SecurityConfig {
            auth_init_rate_per_second: 64,
            export_root: Some(export_root),
            ..SecurityConfig::default()
        },
        audit: Default::default(),
        observability: ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
    }
}

/// Launch a server with [`make_config`], bootstrapping (or re-bootstrapping
/// on an existing data directory — the launcher accepts this, see
/// `persistence_e2e`) the `admin` superuser to [`ADMIN_PASSWORD`]. Bounded.
async fn launch(temp: &TempDir, export_root: PathBuf) -> ServerHandle {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    tokio::time::timeout(
        LAUNCH_BUDGET,
        Box::pin(
            ServerLauncher {
                config: make_config(temp, export_root),
                bootstrap: BootstrapMode::Password {
                    username: ADMIN_USER.into(),
                    password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
                },
                node_mode: Default::default(),
            }
            .launch(),
        ),
    )
    .await
    .expect("server launch outer bound")
    .expect("server launcher boot")
}

/// Graceful shutdown, bounded, and the typed `ShutdownReport` checked
/// CLEAN — a repository checkpoint failure fails the test.
async fn shutdown_clean(handle: ServerHandle) {
    let report: ShutdownReport = tokio::time::timeout(SHUTDOWN_BUDGET, handle.shutdown())
        .await
        .expect("shutdown outer bound");
    assert!(
        report.is_clean(),
        "shutdown must complete cleanly: {:?}",
        report.repository_failures()
    );
}

/// Typed SCRAM client login as the superuser, with the production client
/// options bounded at the brief floor.
async fn connect_admin(addr: std::net::SocketAddr) -> Client {
    tokio::time::timeout(
        CONNECT_BUDGET,
        Client::connect(ConnectOptions {
            addr,
            server_name: "localhost".to_string(),
            username: ADMIN_USER.to_string(),
            password: Zeroizing::new(ADMIN_PASSWORD.to_vec()),
            accept_new_host: true,
            trusted_pin: None,
            connect_timeout: Some(CONNECT_BUDGET),
            handshake_timeout: Some(CONNECT_BUDGET),
            request_timeout: Some(REQUEST_BUDGET),
        }),
    )
    .await
    .expect("typed client connect outer bound")
    .expect("typed client connect")
}

async fn close_client(client: Client) {
    tokio::time::timeout(REQUEST_BUDGET, client.close())
        .await
        .expect("typed client close outer bound");
}

async fn execute(client: &Client, db: &str, batch: Batch, aliases: &[&str]) -> BatchResponse {
    let response = tokio::time::timeout(REQUEST_BUDGET, client.execute(db, batch.build()))
        .await
        .expect("fixture batch outer bound")
        .unwrap_or_else(|_| panic!("fixture batch failed: {aliases:?}"));
    assert_eq!(
        response.results.len(),
        aliases.len(),
        "unexpected result set"
    );
    for &alias in aliases {
        let result = response.results.get(alias).expect("expected result alias");
        assert!(!result.skipped, "{alias} must execute");
    }
    response
}

/// Create `app`/`main`/`rows` AS the connected SCRAM superuser (never a
/// default System-owned database: the superuser issues the DDL itself over
/// the wire) and insert the marker rows of [`ROW_MARKERS`].
async fn create_app_schema(client: &Client) {
    let mut mk_db = Batch::new();
    mk_db.id("mk-db");
    // CreateDb is not HMAC-gated. The existing `default` is only the envelope.
    mk_db.create_db("mk", ddl::create_db(APP_DB));
    execute(client, "default", mk_db, &["mk"]).await;

    let mut mk_repo = Batch::new();
    mk_repo.id("mk-repo");
    mk_repo.create_repo("mr", ddl::create_repo(APP_REPO).engine(RepoEngine::Fjall));
    execute(client, APP_DB, mk_repo, &["mr"]).await;

    // Independent DDL aliases can run concurrently in a single batch.
    let mut mk_table = Batch::new();
    mk_table.id("mk-table");
    mk_table.create_table("tb", ddl::create_table(APP_TABLE).repo(APP_REPO));
    execute(client, APP_DB, mk_table, &["tb"]).await;

    let mut row = Batch::new();
    for (i, marker) in ROW_MARKERS.iter().enumerate() {
        let id = format!("live-{i}");
        // INSERT lets the engine mint the storage RecordId; `id` below is
        // only an application field, not a hand-written storage key.
        row.insert(
            format!("ins-{i}"),
            shamir_query_builder::write::insert(APP_TABLE).row(shamir_query_builder::doc! {
                "id" => id,
                "marker" => *marker,
            }),
        );
    }
    let aliases: Vec<_> = (0..ROW_MARKERS.len()).map(|i| format!("ins-{i}")).collect();
    let aliases: Vec<_> = aliases.iter().map(String::as_str).collect();
    execute(client, APP_DB, row, &aliases).await;
}

/// Full-scan the fixture table and return the sorted `marker` string values
/// of all records.
async fn read_markers_sorted(home: &ShamirDb) -> Vec<String> {
    let mut batch = Batch::new();
    batch.id("rd");
    batch.query("rd", shamir_query_builder::Query::from(APP_TABLE));
    let response = tokio::time::timeout(DATA_BUDGET, home.execute(APP_DB, &batch.build()))
        .await
        .expect("read rows outer bound")
        .expect("read rows");
    let mut markers: Vec<String> = response.results["rd"]
        .records
        .iter()
        .filter_map(|record| record.get_value_str("marker").map(str::to_string))
        .collect();
    markers.sort();
    markers
}

/// The sorted expected marker list.
fn expected_markers_sorted() -> Vec<String> {
    let mut markers: Vec<String> = ROW_MARKERS.iter().map(|m| (*m).to_string()).collect();
    markers.sort();
    markers
}

/// Every entry name in `dir`.
fn entry_names(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

/// `haystack` contains `needle` as a byte subsequence.
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Collect every string carried in a typed dump value (recursively).
fn collect_strings(value: &LogicalValue, out: &mut Vec<String>) {
    match value {
        LogicalValue::Null
        | LogicalValue::Bool(_)
        | LogicalValue::Int(_)
        | LogicalValue::F64(_) => {}
        LogicalValue::Decimal { .. } => {}
        LogicalValue::Big(text) | LogicalValue::Str(text) => out.push(text.clone()),
        LogicalValue::Bin(bytes) => {
            if contains_bytes(bytes, ADMIN_PASSWORD) {
                out.push("<<binary secret sentinel>>".to_string());
            }
        }
        LogicalValue::List(items) | LogicalValue::Set(items) => {
            for item in items {
                collect_strings(item, out);
            }
        }
        LogicalValue::Map(entries) => {
            for (key, item) in entries {
                out.push(key.clone());
                collect_strings(item, out);
            }
        }
    }
}

/// (1) The live roundtrip: a real launched server with the export root
/// configured, the application schema created AS the SCRAM superuser,
/// `start_export` → same-session paced `export_status` polls →
/// `ExportPhase::Done`; the opaque `artifact_name` resolves to exactly one
/// final artifact in the export root with no stage directory left; the
/// artifact verifies with the existing offline verifier; a second session
/// observes NO job (session-only, typed unowned `Db` code, no raw response
/// debug); the whole raw artifact and its typed content carry no secret
/// sentinel and every expected marker is present in the typed content; the
/// offline import reproduces the data in a fresh target, which — reopened
/// through the proven system-store catalogue path — reads back the EXACT
/// expected marker list.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn online_export_roundtrip_verifies_imports_and_leaks_no_secrets() {
    tokio::time::timeout(FIXTURE_BUDGET, Box::pin(roundtrip()))
        .await
        .expect("roundtrip fixture outer bound");
}

async fn roundtrip() {
    let temp = TempDir::new().expect("tempdir");
    let export_root = temp.path().join("export");
    let handle = launch(&temp, export_root.clone()).await;
    let addr = handle.first_tls_exporter_addr().expect("bound addr");

    let client = connect_admin(addr).await;
    create_app_schema(&client).await;

    // Start the job. The `db` argument is the node-local envelope
    // placeholder (the operations are node-local on the server).
    let start = tokio::time::timeout(
        REQUEST_BUDGET,
        client.start_export(APP_DB, ddl::start_export(APP_DB)),
    )
    .await
    .expect("start_export outer bound")
    .expect("start_export accepted");
    assert!(!start.export_id.is_empty(), "export id is minted");

    // Same-session paced polling until Done — never a tight loop. Every
    // poll is individually bounded; a Failed/Cancelled phase aborts with
    // only the TYPED failure reason (never a raw peer-response debug dump —
    // this fixture carries a secret sentinel).
    let status = tokio::time::timeout(EXPORT_BUDGET, async {
        loop {
            let poll = tokio::time::timeout(REQUEST_BUDGET, client.export_status(APP_DB))
                .await
                .expect("status poll outer bound")
                .expect("status on the starting session");
            match poll.phase {
                ExportPhase::Done => break poll,
                ExportPhase::Failed => panic!("export job failed: {:?}", poll.failure),
                ExportPhase::Cancelled => panic!("export job was unexpectedly cancelled"),
                _ => {}
            }
            tokio::time::sleep(POLL_PACE).await;
        }
    })
    .await
    .expect("export did not finish in time");
    assert!(status.sections > 0, "progress reports sections");
    assert!(status.items > 0, "progress reports items");
    assert!(status.bytes > 0, "progress reports bytes");
    assert_eq!(status.export_id, start.export_id);
    assert!(status.failure.is_none(), "Done has no failure reason");

    // The artifact name is OPAQUE: no path components, and it resolves to a
    // real file inside the export root.
    let artifact_name = status.artifact_name.expect("Done carries artifact_name");
    assert!(!artifact_name.is_empty());
    assert!(!artifact_name.contains('/'), "opaque name, not a path");
    assert!(!artifact_name.contains('\\'), "opaque name, not a path");
    let artifact = export_root.join(&artifact_name);
    assert!(artifact.is_file(), "artifact exists under the export root");

    // Session-only: a second, independent session observes NO job — never
    // this one's receipt. The refusal is the typed, UNOWNED
    // `ClientError::Db` with the stable `online_export_not_found` code.
    // NEVER the raw peer response (this test carries a secret sentinel).
    let second = connect_admin(addr).await;
    match tokio::time::timeout(REQUEST_BUDGET, second.export_status(APP_DB))
        .await
        .expect("second session status outer bound")
    {
        Err(ClientError::Db { code, .. }) if code == NOT_FOUND_CODE => {}
        Err(ClientError::Db { .. }) => {
            panic!("expected Db[{NOT_FOUND_CODE}], got a Db error with a different code")
        }
        Err(_) => panic!("expected the typed Db[{NOT_FOUND_CODE}] refusal"),
        Ok(_) => panic!("another session must not observe the job"),
    }

    // Exactly one final artifact and NO stage directory left behind: the
    // export root holds only the artifact and the root lock file.
    let names = entry_names(&export_root);
    let artifacts: Vec<&String> = names.iter().filter(|n| **n == artifact_name).collect();
    assert_eq!(artifacts.len(), 1, "the artifact is published exactly once");
    assert!(
        names
            .iter()
            .all(|n| !n.starts_with(".shamir-export-") || *n == ROOT_LOCK),
        "no stage directory survives a Done export: {names:?}"
    );
    let mut names = names;
    names.sort();
    assert_eq!(names, [ROOT_LOCK.to_string(), artifact_name]);

    // The existing offline verifier accepts the artifact.
    verify_dump_file(&artifact).expect("artifact verifies");

    // Secret sentinels: the SCRAM password bytes must appear NOWHERE —
    // neither in the whole raw artifact nor in any typed value of the
    // decoded content.
    let raw = fs::read(&artifact).expect("read raw artifact");
    assert!(
        !contains_bytes(&raw, ADMIN_PASSWORD),
        "the raw artifact must not carry the SCRAM password bytes"
    );
    // `DumpReader::next_item` returns `Result<Option<PortableItem>>` — ONE
    // expect surfaces the seed/IO error, the `while let` peels the EOF.
    let mut reader = DumpReader::new(fs::File::open(&artifact).unwrap()).unwrap();
    let mut typed_strings = Vec::new();
    let mut markers_in_dump = Vec::new();
    while let Some(item) = reader.next_item().expect("dump reader stream") {
        match item {
            PortableItem::Record(record) => {
                let mut record_strings = Vec::new();
                collect_strings(&record.value, &mut record_strings);
                for marker in ROW_MARKERS {
                    if record_strings.iter().any(|s| s == *marker) {
                        markers_in_dump.push((*marker).to_string());
                    }
                }
                typed_strings.append(&mut record_strings);
            }
            PortableItem::Identity(identity) => typed_strings.push(identity.logical_name),
            PortableItem::Acl(acl) => {
                if let Some(owner) = &acl.owner {
                    typed_strings.push(owner.clone());
                }
                if let Some(group) = &acl.group {
                    typed_strings.push(group.clone());
                }
            }
            _ => {}
        }
    }
    drop(reader);
    assert!(
        !typed_strings.iter().any(|s| s.as_bytes() == ADMIN_PASSWORD),
        "the typed dump content must not carry the SCRAM password"
    );
    for marker in ROW_MARKERS {
        assert!(
            markers_in_dump.iter().any(|s| s == marker),
            "marker {marker} must be present in the typed content"
        );
    }

    // Offline import into a fresh target (the driver creates it) — direct
    // async on this test's runtime, bounded; NO nested Runtime::block_on.
    let target = temp.path().join("import-target");
    assert!(!target.exists(), "the import target starts absent");
    let report = tokio::time::timeout(
        DATA_BUDGET,
        Box::pin(shamir_server::logical_import::import(
            import_args(&artifact, &target),
            CancellationToken::new(),
        )),
    )
    .await
    .expect("import outer bound")
    .expect("import succeeds");
    assert!(report.items > 0, "the import reports imported items");

    // Bounded graceful shutdown, report checked CLEAN, before any directory
    // is reopened.
    close_client(client).await;
    close_client(second).await;
    shutdown_clean(handle).await;

    // Reopen through the PROVEN system-store catalogue path (same as the
    // offline fixtures: Fjall store + `check_seed_catalogue` before any
    // facade read), then read back the EXACT expected marker list.
    let imported = tokio::time::timeout(
        DATA_BUDGET,
        ShamirDb::init(SystemStoreConfig::Fjall(target.join("shamir_db_meta.redb"))),
    )
    .await
    .expect("reopen outer bound")
    .expect("imported target opens");
    tokio::time::timeout(DATA_BUDGET, imported.check_seed_catalogue())
        .await
        .expect("catalogue walk outer bound")
        .expect("seed catalogue");
    let markers = read_markers_sorted(&imported).await;
    assert_eq!(
        markers,
        expected_markers_sorted(),
        "the imported target reads back exactly the fixture markers"
    );
    drop(imported);
    wait_for_store_release(&target.join("shamir_db_meta.redb")).await;
    wait_for_store_release(&target.join(format!("shamir_db_{APP_DB}_{APP_REPO}.redb"))).await;
    temp.close()
        .expect("roundtrip fixture directories released");
}

/// The offline reader has no server shutdown barrier. Probe Fjall's lock
/// after dropping the reader; back off only on lock contention (Windows can
/// still have detached store tasks), never on a different I/O failure.
async fn wait_for_store_release(store: &Path) {
    tokio::time::timeout(DATA_BUDGET, async {
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(store.join("lock"))
            .expect("open Fjall lock for release probe");
        let mut delay = Duration::from_millis(1);
        loop {
            match lock.try_lock() {
                Ok(()) => break,
                Err(TryLockError::WouldBlock) => {
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(POLL_PACE);
                }
                Err(error) => panic!("Fjall lock probe failed: {error}"),
            }
        }
    })
    .await
    .expect("offline store release outer bound");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn online_export_denies_unreadable_scope_without_job_or_output() {
    tokio::time::timeout(FIXTURE_BUDGET, Box::pin(unreadable_scope()))
        .await
        .expect("unreadable-scope fixture outer bound");
}

async fn unreadable_scope() {
    let temp = TempDir::new().expect("tempdir");
    let export_root = temp.path().join("export");
    let handle = launch(&temp, export_root.clone()).await;
    let client = connect_admin(handle.first_tls_exporter_addr().expect("bound addr")).await;
    create_app_schema(&client).await;
    assert_eq!(entry_names(&export_root), [ROOT_LOCK.to_string()]);

    // Leave the database and repository alone. Remove read from every class
    // on the only included table, retaining owner write and execute. The
    // export scope checks DAC as the principal, unlike normal admin DML.
    let mode = 0o300;
    let resource = ddl::res::table(APP_DB, APP_REPO, APP_TABLE);
    // The builder requires a tag; the canonical encoder excludes that field.
    let template = ddl::chmod(resource.clone(), mode).hmac("").build().unwrap();
    let canonical = canonical_batch_op(APP_DB, &template).expect("canonical table chmod");
    let key = derive_session_hmac_key(&client.session_id());
    let signed = ddl::chmod(resource, mode)
        .hmac(compute_tag_hex(&key, &canonical))
        .build()
        .expect("signed table chmod");
    let mut chmod = Batch::new();
    chmod.chmod("chmod", signed);
    let response = execute(&client, APP_DB, chmod, &["chmod"]).await;
    assert_eq!(response.results["chmod"].records.len(), 1);
    assert_eq!(
        response.results["chmod"].records[0].get_value_i64("mode"),
        Some(i64::from(mode)),
        "chmod applied the write-only mode"
    );

    let start = tokio::time::timeout(
        REQUEST_BUDGET,
        client.start_export(
            APP_DB,
            ddl::start_export(APP_DB).repo(APP_REPO).table(APP_TABLE),
        ),
    )
    .await
    .expect("denied start outer bound");
    assert!(
        matches!(start, Err(ClientError::Db { code, .. }) if code == "export_scope_denied"),
        "StartExport must deny an unreadable included table"
    );
    let status = tokio::time::timeout(REQUEST_BUDGET, client.export_status(APP_DB))
        .await
        .expect("same-session status outer bound");
    assert!(
        matches!(status, Err(ClientError::Db { code, .. }) if code == NOT_FOUND_CODE),
        "the denied start must leave no job visible to its own session"
    );
    assert_eq!(entry_names(&export_root), [ROOT_LOCK.to_string()]);

    // This fixture has no acquisition observer. A committed follow-up proves
    // writes are admitted after denial, not that there was no transient freeze.
    const FOLLOW_UP: &str = "after-denied-export";
    let mut write = Batch::new();
    write.insert(
        "write",
        shamir_query_builder::write::insert(APP_TABLE)
            .row(shamir_query_builder::doc! { "id" => FOLLOW_UP, "marker" => FOLLOW_UP }),
    );
    execute(&client, APP_DB, write, &["write"]).await;
    let mut read = Batch::new();
    read.query("read", shamir_query_builder::Query::from(APP_TABLE));
    let response = execute(&client, APP_DB, read, &["read"]).await;
    let mut markers: Vec<_> = response.results["read"]
        .records
        .iter()
        .map(|record| record.get_value_str("marker").expect("stored marker"))
        .collect();
    markers.sort_unstable();
    let mut expected = ROW_MARKERS.to_vec();
    expected.push(FOLLOW_UP);
    expected.sort_unstable();
    assert_eq!(markers, expected);

    close_client(client).await;
    shutdown_clean(handle).await;
    assert_eq!(entry_names(&export_root), [ROOT_LOCK.to_string()]);
    temp.close().expect("denial fixture directories released");
}

/// (2) Restart with the same data directory and the same export root: one
/// stale temp directory matching the job's own stage pattern
/// (`.shamir-export-` + exactly 6 ASCII alphanumerics) is removed at boot,
/// while a nonmatching operator sentinel file is left untouched; the data
/// written before the restart still reads back as the exact marker list.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn online_export_restart_cleans_stale_stage_and_spares_sentinel() {
    tokio::time::timeout(FIXTURE_BUDGET, Box::pin(restart()))
        .await
        .expect("restart fixture outer bound");
}

async fn restart() {
    let temp = TempDir::new().expect("tempdir");
    let export_root = temp.path().join("export");
    let handle = launch(&temp, export_root.clone()).await;
    let addr = handle.first_tls_exporter_addr().expect("bound addr");

    let client = connect_admin(addr).await;
    create_app_schema(&client).await;
    close_client(client).await;
    // Shutdown drains accepted sessions and includes a Fjall lock-release
    // barrier. Relaunch on the same directories checks that it succeeded.
    shutdown_clean(handle).await;

    // A crashed job's leftovers: a stale temp DIRECTORY matching the exact
    // stage pattern, and a nonmatching operator sentinel FILE.
    let stale_stage = export_root.join(".shamir-export-ab12cd");
    fs::create_dir(&stale_stage).expect("stale stage dir");
    fs::write(stale_stage.join("partial.bin"), b"crashed job bytes").expect("stale payload");
    let sentinel = export_root.join("operator-sentinel.txt");
    fs::write(&sentinel, b"operator artifact - never touched").expect("sentinel file");

    let handle = launch(&temp, export_root.clone()).await;
    let names = entry_names(&export_root);
    assert!(
        !stale_stage.exists(),
        "the stale stage directory must be cleaned at boot"
    );
    assert!(
        sentinel.is_file(),
        "the nonmatching sentinel file must be untouched; root now: {names:?}"
    );
    assert!(
        names.iter().any(|n| n == ROOT_LOCK),
        "the export root lock file is present"
    );

    // Same data: the pre-restart marker rows still read back exactly.
    let client = connect_admin(handle.first_tls_exporter_addr().unwrap()).await;
    let mut read = Batch::new();
    read.id("rd");
    read.query("rd", shamir_query_builder::Query::from(APP_TABLE));
    let response = tokio::time::timeout(REQUEST_BUDGET, client.execute(APP_DB, read.build()))
        .await
        .expect("read after restart outer bound")
        .expect("read after restart");
    let mut markers: Vec<String> = response.results["rd"]
        .records
        .iter()
        .filter_map(|record| record.get_value_str("marker").map(str::to_string))
        .collect();
    markers.sort();
    assert_eq!(
        markers,
        expected_markers_sorted(),
        "the same data survives the restart"
    );
    close_client(client).await;
    shutdown_clean(handle).await;
    temp.close().expect("restart fixture directories released");
}
