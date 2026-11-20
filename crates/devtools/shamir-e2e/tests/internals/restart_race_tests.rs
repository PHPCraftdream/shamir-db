//! Real-server RESTART and RACE coverage for application-schema migrations
//! (design `docs/dev-artifacts/design/app-schema-migrations.md` §7.3 e2e /
//! §7.5; brief `55-r1c-4-integration-guides-acceptance.md` item 2).
//!
//! Two scenarios against the REAL server with a PERSISTENT data directory:
//!
//! **Restart** (`migration_real_server_restart_same_directory_...`):
//! apply migration 1, SHUT the server down cleanly (drain + flush, clean
//! report awaited before reopening the directory), relaunch on the SAME
//! `TempDir` (the `Password` bootstrap is a no-op on an already-bootstrapped
//! directory), reconnect, and prove:
//!
//! 1. `status` reports the applied prefix (the exact ordinal-1 entry) and
//!    the fence generation UNCHANGED across the restart (the released
//!    generation 2 observed before shutdown equals the released generation
//!    reported after the restart),
//! 2. a repeat of the applied migration is a typed no-op refusal
//!    (`MigrationError::NothingToApply`) AFTER the restart — no second
//!    applied event,
//! 3. migration 2 applies through the same high-level path with the
//!    STATUS-DERIVED expected generation (2, reported by the post-restart
//!    status; acquired 3 in the apply result),
//! 4. drift is refused after the restart (changed FIRST body ahead of an
//!    identical second body and a pending third:
//!    `PrefixCoverage::Drift` in status, `MigrationError::Drift` in
//!    `apply_next`, `MigrationError::ChecksumMismatch` in `plan`),
//! 5. repeat-and-drift ordering: after the second apply another repeat
//!    refuses and the drifted refusals leave the journal at exactly the
//!    two applied entries with the released generation 4.
//!
//! **Race** (`migration_real_server_concurrent_appliers_one_gate_...`), run
//! in TWO modes over ONE SHARED scenario body (`run_race_scenario`):
//!
//! * **Preflight**
//!   (`..._status_preflight_exactly_one_applied_event`): both connections
//!   use the high-level `Client::apply_next` (status preflight + typed
//!   send).
//! * **Raw** (`..._raw_typed_request_exactly_one_applied_event`): each
//!   connection independently derives the SAME typed
//!   `SchemaMigrationApplyRequest` from ITS OWN `status` read (the fresh
//!   path's status-derived predecessor and fence generation, plus the
//!   identical body and canonical checksum), builds the typed
//!   `apply_application_schema_migration` operation, and sends it through
//!   the plain `Client::execute` transport from its OWN connection — no
//!   `apply_next` preflight anywhere. Both requests are asserted EQUAL
//!   before the gate.
//!
//! Both modes release the two appliers from ONE gate (`tokio::sync::Barrier`)
//! and assert on the SET of outcomes: AT LEAST ONE applier returns an
//! applied result (both refusals are impossible — the raced event IS durably
//! applied exactly once, so the winner's apply already-applied fast path
//! returns a successful response); if BOTH return applied, the two stored
//! responses must be EQUAL (the fast-path loser gets the identical stored
//! response). A loser may instead be refused typed: in preflight mode
//! `NothingToApply` / `InProgress` / `PredecessorMismatch`; in raw mode ONLY
//! the result-record error codes `schema_migration_in_progress` /
//! `schema_migration_predecessor_mismatch` (a raw send can never produce a
//! client-side `NothingToApply` — the ordinal is submitted explicitly — so
//! that code is impossible here and any other record error fails the test).
//! The DURABLE effect is pinned identically in both modes: afterwards
//! `status` reports EXACTLY ONE applied event for the raced ordinal (exact
//! id and canonical checksum, fence released generation 2, no unresolved
//! tail), the physical `events` table answers a typed builder query, and a
//! repeat refuses without a second event.
//!
//! Direct-journal seam note: the persisted journal is asserted through the
//! `status` projection (`MigrationStatus::applied` — the full applied
//! prefix verified entry by entry by the client, plus the durable fence
//! generation) because no journal seam is reachable from this crate: the
//! `shamir-db` journal accessors hang off `SystemStore`
//! (`read_schema_migration_history`), and `ServerHandle` exposes no engine
//! handle (only `first_tls_exporter_addr` / `format_stamp` / IPC path), so
//! a real-server test in `shamir-e2e` cannot read the journal directly.
//!
//! Raw decode note: an applied raw send answers with the applied result
//! record, asserted field by field on its `QueryValue` payload map
//! (`migration_id`, `ordinal`, `status`, `fence_generation` — the wire
//! shape the typed `SchemaMigrationApplyResult` deserializes from); a
//! refusal is an ENVELOPE-level error answered as
//! `ClientError::Db { code }` / `ClientError::Server { code }` and is
//! matched on the stable §5.3 code strings. The typed client decode
//! helpers are `pub(super)` inside `shamir-client`, and `rmp_serde` is not
//! a dependency of this test crate (no new dependencies allowed), so the
//! payload-map assertion IS the typed-record assertion here.
//!
//! Fence-generation vocabulary (as in `rust_migration_tests.rs`): EXPECTED
//! = what the client sends (status-derived), ACQUIRED = the generation the
//! apply RESULT reports, RELEASED = the highest durable fence generation a
//! status reports.
//!
//! Discipline: every await bounded (120 s request deadline, 360 s scenario
//! deadline = hang guard), persistent `TempDir` alive for the whole test,
//! clean shutdown awaited before reopen, bounded retry on `Locked` ONLY at
//! the relaunch (fjall releases directory locks from background handles —
//! no fixed sleeps for synchronization anywhere else), no hand-built wire
//! values (typed builders only), failure-only `FlakeGuard`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tempfile::TempDir;
use zeroize::Zeroizing;

use shamir_client::builder::batch::{Batch, IntoBatchOp};
use shamir_client::builder::ddl::{self, RepoEngine};
use shamir_client::builder::query::Query;
use shamir_client::builder::SchemaMigration;
use shamir_client::schema_migration::{
    ApplicationMigrationSet, MigrationError, PrefixCoverage, SchemaMigrationAppliedEntry,
    SchemaMigrationApplyResult, SchemaMigrationErrorCode, SchemaMigrationTarget,
};
use shamir_client::{Client, ClientError, ConnectOptions};

use shamir_query_builder::apply_application_schema_migration;
use shamir_query_builder::batch::Batch as RawBatch;
use shamir_query_types::admin::types::schema_migration::{
    SchemaMigrationApplyRequest, SchemaMigrationPredecessor,
};
use shamir_query_types::batch::BatchResponse;
use shamir_query_types::read::QueryRecord;

use shamir_server::config::{
    Config, KdfConfig, ListenerConfig, ListenerKind, LoggingConfig, ProfileKind, TlsConfig,
};
use shamir_server::server::{BootstrapMode, ServerHandle, ServerLauncher};
use shamir_test_diag::FlakeGuard;
use shamir_types::types::record_id::RecordId;
use shamir_types::types::value::QueryValue;

use tokio::sync::Barrier;

const DB: &str = "migdb";
const R1_ID: &str = "restart-create-items";
const R2_ID: &str = "restart-create-audit";
const R3_ID: &str = "restart-create-metrics";
const RACE_ID: &str = "race-create-events";
const DRIFT_TABLE: &str = "items_tampered";
const APPLY_ALIAS: &str = "apply_application_schema_migration";
const ADMIN_PASSWORD: &[u8] = b"correct horse battery staple";
const TEST_TIMEOUT: Duration = Duration::from_secs(360);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Which send path the race scenario uses for BOTH appliers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RaceMode {
    /// High-level `Client::apply_next` (status preflight + typed send).
    Preflight,
    /// Raw typed `SchemaMigrationApplyRequest` via plain `Client::execute`
    /// from each independent connection; no preflight.
    Raw,
}

fn migration_config(temp: &TempDir) -> Config {
    let data_dir: PathBuf = temp.path().to_path_buf();
    Config {
        tokens: Default::default(),
        data_dir: data_dir.clone(),
        logging: LoggingConfig {
            level: "warn".into(),
            slow_query_threshold_ms: 0,
            file: None,
            flush_interval_ms: 2000,
        },
        durability: Default::default(),
        transactions: Default::default(),
        kdf_defaults: KdfConfig {
            memory_kb: 19_456,
            time: 2,
            parallelism: 1,
            argon2_version: 0x13,
        },
        // Several concurrent sessions are opened by the race scenario
        // (brief: fixtures that open several sessions set this to 64).
        argon2_concurrent_max: 64,
        listeners: vec![ListenerConfig {
            kind: ListenerKind::Tcp,
            addr: "127.0.0.1:0".into(),
            profile: Some(ProfileKind::TlsExporter),
            path: None,
            kdf_override: None,
            browser_origin_allowlist: vec![],
            http: Default::default(),
        }],
        tls: TlsConfig {
            cert_path: data_dir.join("cert.pem"),
            key_path: data_dir.join("cert.key"),
        },
        security: Default::default(),
        audit: Default::default(),
        observability: shamir_server::config::ObservabilityConfig {
            addr: String::new(),
            allow_public_metrics: false,
        },
        replication: None,
        backup: None,
    }
}

fn migration_body(
    id: &str,
    ordinal: u64,
    name: &str,
    table: &str,
) -> shamir_client::schema_migration::SchemaMigrationBody {
    SchemaMigration::new(id, ordinal, name)
        .op(ddl::create_table(table).repo("main").into_batch_op())
        .expect("create_table is an admitted migration operation")
        .build()
        .expect("migration body validates")
}

fn password() -> Zeroizing<Vec<u8>> {
    Zeroizing::new(ADMIN_PASSWORD.to_vec())
}

/// Launch on `config.data_dir`; retry bounded ONLY on the transient
/// `Locked` error (fjall background handles release the directory lock
/// slightly after the shutdown future resolves; Windows holds the lock
/// until they finish). No other sleeping.
async fn launch_on_same_directory(
    guard: &FlakeGuard,
    config: Config,
    label: &'static str,
) -> ServerHandle {
    for _ in 0..200 {
        guard.mark("launch retry window tick");
        let launched = ServerLauncher {
            node_mode: Default::default(),
            config: config.clone(),
            // Idempotent on an already-bootstrapped directory
            // (`BootstrapOutcome::AlreadyExists` is a no-op).
            bootstrap: BootstrapMode::Password {
                username: "admin".into(),
                password: password(),
            },
        }
        .launch()
        .await;
        match launched {
            Ok(handle) => return handle,
            Err(e) if format!("{e:?}").contains("Locked") => {
                guard.note(label, "transient fjall lock; retrying launch");
            }
            Err(e) => panic!("{label}: launcher boot failed: {e:?}"),
        }
    }
    panic!("{label}: fjall lock was not released within the bounded retry window");
}

async fn connect_client(addr: std::net::SocketAddr) -> Client {
    tokio::time::timeout(
        REQUEST_TIMEOUT,
        Box::pin(Client::connect(ConnectOptions {
            addr,
            server_name: "localhost".into(),
            username: "admin".into(),
            password: password(),
            accept_new_host: true,
            trusted_pin: None,
            connect_timeout: None,
            handshake_timeout: None,
            request_timeout: Some(REQUEST_TIMEOUT),
        })),
    )
    .await
    .expect("client connect deadline")
    .expect("client connect")
}

/// Fixture: db + a PERSISTENT Fjall repo (explicit durable engine).
async fn create_db_and_repo(client: &Client) {
    let mut mk = Batch::new();
    mk.id("mk-db");
    mk.create_db(DB, ddl::create_db(DB));
    tokio::time::timeout(REQUEST_TIMEOUT, client.execute("default", mk.build()))
        .await
        .expect("create db deadline")
        .expect("create db");
    let mut repo = Batch::new();
    repo.id("mk-repo");
    repo.create_repo("mr", ddl::create_repo("main").engine(RepoEngine::Fjall));
    tokio::time::timeout(REQUEST_TIMEOUT, client.execute(DB, repo.build()))
        .await
        .expect("create repo deadline")
        .expect("create repo");
}

/// Identity discovery through the documented mismatch echo (as in
/// `rust_migration_tests.rs`).
async fn discover_identity(
    client: &Client,
    set: &ApplicationMigrationSet,
) -> SchemaMigrationTarget {
    let probe = client
        .status(
            DB,
            SchemaMigrationTarget {
                database_identity: RecordId::from_ts_seq(1, 1),
            },
            set,
        )
        .await
        .expect("status is read-only; a mismatch is a result, not an error");
    assert_eq!(
        probe.server_error,
        Some(SchemaMigrationErrorCode::SchemaMigrationIdentityMismatch),
        "the probe must report the typed identity mismatch"
    );
    let identity = probe.reported_target.database_identity;
    assert_ne!(
        identity,
        RecordId::from_ts_seq(1, 1),
        "the echo must carry the serving identity"
    );
    SchemaMigrationTarget {
        database_identity: identity,
    }
}

async fn shutdown_clean(guard: &FlakeGuard, handle: ServerHandle, label: &'static str) {
    // `shutdown` resolves to the `ShutdownReport` itself (not a `Result`):
    // only the deadline wrapper is fallible.
    let report = tokio::time::timeout(TEST_TIMEOUT, handle.shutdown())
        .await
        .expect("shutdown deadline");
    if !report.is_clean() {
        guard.note(
            label,
            format!("unclean shutdown: {:?}", report.repository_failures()),
        );
    }
    assert!(
        report.is_clean(),
        "{label}: clean shutdown, including the persistent repo: {:?}",
        report.repository_failures()
    );
}

// ---------------------------------------------------------------------------
// Scenario 1: restart on the same persistent data directory
// ---------------------------------------------------------------------------

async fn run_restart_scenario(guard: &FlakeGuard) {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let temp = TempDir::new().expect("tempdir");
    let first = launch_on_same_directory(guard, migration_config(&temp), "first boot").await;
    let addr = first.first_tls_exporter_addr().expect("bound listener");
    let client = connect_client(addr).await;
    create_db_and_repo(&client).await;

    let first_body = migration_body(R1_ID, 1, "create items table", "items");
    let second_body = migration_body(R2_ID, 2, "create audit table", "audit");
    let third_body = migration_body(R3_ID, 3, "create metrics table", "metrics");
    let set_full = ApplicationMigrationSet::new(vec![first_body.clone(), second_body.clone()])
        .expect("the two-migration set validates");
    let set_first_only =
        ApplicationMigrationSet::new(vec![first_body.clone()]).expect("the one-migration set");
    let target = discover_identity(&client, &set_full).await;

    // --- apply migration 1 BEFORE the restart ---------------------------
    let applied = client
        .apply_next(DB, target, &set_full)
        .await
        .expect("first apply");
    assert_eq!(applied.migration_id, R1_ID);
    assert_eq!(applied.status, "applied");
    assert_eq!(
        applied.fence_generation,
        Some(1),
        "ACQUIRED generation: expected 0, acquired 1"
    );

    // Released generation observed BEFORE the restart — this exact value
    // must survive the restart unchanged.
    let before = client.status(DB, target, &set_full).await.expect("status");
    assert_eq!(before.coverage, PrefixCoverage::Verified { ordinal: 1 });
    assert_eq!(
        before.fence_generation, 2,
        "RELEASED generation after the first apply"
    );
    client.close().await;

    // --- clean shutdown, then relaunch on the SAME directory ------------
    guard.mark("shutting down for restart");
    shutdown_clean(guard, first, "restart").await;
    guard.mark("relaunching on the same directory");
    let second = launch_on_same_directory(guard, migration_config(&temp), "restart boot").await;
    let addr2 = second.first_tls_exporter_addr().expect("bound listener");
    let client = connect_client(addr2).await;

    // --- the applied prefix and fence generation survive the restart ----
    let after = client.status(DB, target, &set_full).await.expect("status");
    assert_eq!(
        after.coverage,
        PrefixCoverage::Verified { ordinal: 1 },
        "the applied prefix is reported after the restart"
    );
    assert_eq!(
        after.applied,
        vec![SchemaMigrationAppliedEntry {
            id: R1_ID.to_string(),
            ordinal: 1,
            checksum: set_full.checksums()[0],
        }],
        "the persisted journal prefix carries the exact applied entry"
    );
    assert_eq!(after.last_applied_ordinal, 1);
    assert!(
        after.journal_checksum_verified(),
        "the persisted checksum verifies after the restart"
    );
    assert_eq!(
        after.fence_generation,
        before.fence_generation,
        "the fence generation is UNCHANGED across the restart \
         (released {before} == released {after})",
        before = before.fence_generation,
        after = after.fence_generation,
    );

    // --- repeat of the applied migration AFTER the restart: typed no-op -
    match client.apply_next(DB, target, &set_first_only).await {
        Err(MigrationError::NothingToApply) => {}
        other => panic!("expected NothingToApply after the restart, got {other:?}"),
    }
    let still_one = client.status(DB, target, &set_full).await.expect("status");
    assert_eq!(
        still_one.applied.len(),
        1,
        "the repeat must not have appended a second applied event"
    );
    assert_eq!(still_one.fence_generation, before.fence_generation);

    // --- apply migration 2 with the STATUS-DERIVED expected generation --
    let applied_second = client
        .apply_next(DB, target, &set_full)
        .await
        .expect("second apply after the restart");
    assert_eq!(applied_second.migration_id, R2_ID);
    assert_eq!(applied_second.ordinal, 2);
    assert_eq!(applied_second.status, "applied");
    assert_eq!(
        applied_second.fence_generation,
        Some(3),
        "ACQUIRED generation: the status-derived expected 2, acquired 3"
    );

    // --- drift is refused AFTER the restart ------------------------------
    // Changed FIRST body (different canonical checksum) ahead of an
    // IDENTICAL second body and a pending third, so any refusal below is
    // attributable to the drift alone.
    let drift_first = migration_body(R1_ID, 1, "create items table", DRIFT_TABLE);
    let set_drift = ApplicationMigrationSet::new(vec![drift_first, second_body, third_body])
        .expect("the drifted three-migration set validates");
    let drifted_status = client.status(DB, target, &set_drift).await.expect("status");
    match &drifted_status.coverage {
        PrefixCoverage::Drift { ordinal, .. } => assert_eq!(*ordinal, 1),
        other => panic!("expected Drift at ordinal 1 after the restart, got {other:?}"),
    }
    match client.apply_next(DB, target, &set_drift).await {
        Err(MigrationError::Drift { ordinal: 1, .. }) => {}
        other => panic!("expected typed Drift refusal after the restart, got {other:?}"),
    }
    match client.plan(DB, target, &set_drift).await {
        Err(MigrationError::ChecksumMismatch { .. }) => {}
        other => panic!("expected ChecksumMismatch from plan after the restart, got {other:?}"),
    }

    // --- repeat AND drift ordering: the journal is exactly the two bodies
    let final_status = client.status(DB, target, &set_full).await.expect("status");
    assert_eq!(
        final_status.coverage,
        PrefixCoverage::Verified { ordinal: 2 }
    );
    assert_eq!(
        final_status.applied,
        vec![
            SchemaMigrationAppliedEntry {
                id: R1_ID.to_string(),
                ordinal: 1,
                checksum: set_full.checksums()[0],
            },
            SchemaMigrationAppliedEntry {
                id: R2_ID.to_string(),
                ordinal: 2,
                checksum: set_full.checksums()[1],
            },
        ],
        "the refusals left the journal at exactly the two applied bodies"
    );
    assert_eq!(
        final_status.fence_generation, 4,
        "RELEASED generation after the second apply (expected 2, acquired 3)"
    );

    client.close().await;
    guard.mark("final shutdown");
    shutdown_clean(guard, second, "final").await;
}

// ---------------------------------------------------------------------------
// Scenario 2: two concurrent appliers of the same ordinal, one gate —
// shared by the preflight and the raw send path
// ---------------------------------------------------------------------------

/// The race fixture: a launched server and TWO independent client
/// connections over one persistent directory, plus the raced set and the
/// discovered target identity.
struct RaceFixture {
    temp: TempDir,
    handle: ServerHandle,
    client_a: Client,
    client_b: Client,
    set: ApplicationMigrationSet,
    target: SchemaMigrationTarget,
}

async fn race_fixture(guard: &FlakeGuard) -> RaceFixture {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let temp = TempDir::new().expect("tempdir");
    let handle = launch_on_same_directory(guard, migration_config(&temp), "race boot").await;
    let addr = handle.first_tls_exporter_addr().expect("bound listener");

    // TWO independent client connections (single release gate below).
    let client_a = connect_client(addr).await;
    let client_b = connect_client(addr).await;
    create_db_and_repo(&client_a).await;

    let body = migration_body(RACE_ID, 1, "create events table", "events");
    let set = ApplicationMigrationSet::new(vec![body]).expect("the one-migration set");
    let target = discover_identity(&client_a, &set).await;
    RaceFixture {
        temp,
        handle,
        client_a,
        client_b,
        set,
        target,
    }
}

/// Build the typed RAW apply request from THIS connection's own status
/// read: the fresh path's status-derived predecessor (zero on an empty
/// journal) and expected fence generation, plus the identical body and
/// canonical checksum. Mirrors the fresh arm of the client's `apply_next`
/// preflight (no in-flight entry may exist on a fresh journal).
async fn status_derived_raw_request(
    client: &Client,
    target: SchemaMigrationTarget,
    set: &ApplicationMigrationSet,
) -> SchemaMigrationApplyRequest {
    let st = client.status(DB, target, set).await.expect("status");
    assert!(
        st.in_flight.is_none(),
        "the fresh race journal must hold no unresolved entry: {st:?}"
    );
    assert_eq!(
        st.last_applied_ordinal, 0,
        "the raced ordinal is the FIRST migration of the journal"
    );
    let predecessor = if st.last_applied_ordinal == 0 {
        SchemaMigrationPredecessor {
            migration_id: None,
            ordinal: 0,
            checksum: [0; 32],
        }
    } else {
        SchemaMigrationPredecessor {
            migration_id: st.last_applied_id.clone(),
            ordinal: st.last_applied_ordinal,
            checksum: st
                .last_applied_checksum
                .expect("a nonempty head reports its checksum"),
        }
    };
    SchemaMigrationApplyRequest::from_body(
        target,
        predecessor,
        st.fence_generation,
        &set.bodies()[0],
        set.checksums()[0],
    )
}

/// Send the typed raw request through the plain `execute` transport of
/// this connection (ONE typed admin op in ONE batch — no preflight).
async fn send_raw_apply(
    client: &Client,
    request: SchemaMigrationApplyRequest,
) -> Result<BatchResponse, ClientError> {
    let mut batch = RawBatch::new();
    batch.id("raw-race-apply");
    batch.op(APPLY_ALIAS, apply_application_schema_migration(request));
    let built = batch.try_build().expect("the raw apply batch builds");
    tokio::time::timeout(REQUEST_TIMEOUT, client.execute(DB, built))
        .await
        .expect("raw apply deadline")
}

/// The direct record payload of the raw apply result (the typed
/// `SchemaMigrationApplyResult` wire shape, asserted as a map).
fn raw_apply_payload(response: &BatchResponse) -> &QueryValue {
    let result = response
        .results
        .get(APPLY_ALIAS)
        .expect("the apply result is present under the operation alias");
    let record = result
        .records
        .first()
        .expect("the apply result carries a payload record");
    match record {
        QueryRecord::Direct(payload) => payload,
        other => panic!("the apply payload is a direct record, got {other:?}"),
    }
}

/// A successful race response, kept for the identical-response assertion:
/// the typed result (preflight) or the raw payload map (raw).
#[derive(Debug, PartialEq)]
enum AppliedResponse {
    Typed(SchemaMigrationApplyResult),
    Map(QueryValue),
}

/// One applier's outcome, normalized across the two send paths.
#[derive(Debug)]
enum RaceOutcome {
    /// The stored applied response (typed, or the raw payload map).
    Applied(AppliedResponse),
    /// A typed refusal (allowed refusal kinds are enumerated per mode in
    /// the `classify_*` functions; only the variant is distinguished here).
    Refused,
}

fn classify_preflight(outcome: &Result<SchemaMigrationApplyResult, MigrationError>) -> RaceOutcome {
    match outcome {
        Ok(applied) => {
            assert_eq!(applied.migration_id, RACE_ID);
            assert_eq!(applied.ordinal, 1);
            assert_eq!(applied.status, "applied");
            assert_eq!(
                applied.fence_generation,
                Some(1),
                "ACQUIRED: expected 0, acquired 1"
            );
            RaceOutcome::Applied(AppliedResponse::Typed(applied.clone()))
        }
        Err(MigrationError::NothingToApply)
        | Err(MigrationError::InProgress { .. })
        | Err(MigrationError::PredecessorMismatch { .. }) => {
            // Allowed refusal outcomes: its preflight saw the winner's
            // applied head, or the server fenced the stale expected
            // generation / predecessor.
            RaceOutcome::Refused
        }
        Err(other) => panic!(
            "the loser must refuse typed (NothingToApply / InProgress / \
             PredecessorMismatch); got {other:?}"
        ),
    }
}

fn classify_raw(outcome: &Result<BatchResponse, ClientError>) -> RaceOutcome {
    match outcome {
        Ok(response) => {
            // Applied: the typed `SchemaMigrationApplyResult` fields, on the
            // wire shape the typed struct deserializes from. A successful
            // envelope carries the applied result record (a refusal is an
            // ENVELOPE-level error, never an error field inside the
            // record).
            let payload = raw_apply_payload(response).clone();
            match payload.get("migration_id") {
                Some(QueryValue::Str(id)) => assert_eq!(id, RACE_ID),
                other => panic!("the applied result reports migration_id, got {other:?}"),
            }
            match payload.get("ordinal") {
                Some(QueryValue::Int(1)) => {}
                other => panic!("the applied result reports ordinal 1, got {other:?}"),
            }
            match payload.get("status") {
                Some(QueryValue::Str(status)) => assert_eq!(status, "applied"),
                other => panic!("the applied result status is a string, got {other:?}"),
            }
            match payload.get("fence_generation") {
                Some(QueryValue::Int(1)) => {}
                other => panic!("ACQUIRED: expected 0, acquired 1, got {other:?}"),
            }
            RaceOutcome::Applied(AppliedResponse::Map(payload))
        }
        Err(ClientError::Db { code, .. }) | Err(ClientError::Server { code }) => {
            // Typed refusal codes at the envelope level: ONLY the
            // in-progress / stale-predecessor refusals are allowed. A raw
            // send can never produce a `nothing_to_apply` (the ordinal is
            // submitted explicitly), and any other code is a failure.
            match code.as_str() {
                "schema_migration_in_progress" | "schema_migration_predecessor_mismatch" => {
                    RaceOutcome::Refused
                }
                other => panic!(
                    "the raw loser must refuse with in_progress or \
                     predecessor_mismatch only; got the error code {other:?}"
                ),
            }
        }
        Err(other) => panic!(
            "the raw apply must answer with a result record (an applied result or a \
             typed refusal code), not a transport/shape failure; got {other:?}"
        ),
    }
}

/// The DURABLE effect of the race, pinned identically in both modes:
/// exactly ONE applied event for the raced ordinal, the fence advanced
/// exactly once, no unresolved tail, the physical table present, and a
/// repeat refused without a second event.
async fn assert_race_durable_state(
    guard: &FlakeGuard,
    client: &Client,
    target: SchemaMigrationTarget,
    set: &ApplicationMigrationSet,
) {
    // The persisted journal prefix carries EXACTLY ONE applied event for
    // the raced ordinal, with the exact id and canonical checksum, and the
    // fence advanced exactly once (acquired 1, released 2). Asserted via
    // the status projection (no direct journal seam in this crate).
    let settled = client.status(DB, target, set).await.expect("status");
    assert_eq!(
        settled.coverage,
        PrefixCoverage::Verified { ordinal: 1 },
        "the raced ordinal is applied exactly once"
    );
    assert_eq!(
        settled.applied,
        vec![SchemaMigrationAppliedEntry {
            id: RACE_ID.to_string(),
            ordinal: 1,
            checksum: set.checksums()[0],
        }],
        "exactly one applied journal event for the raced ordinal"
    );
    assert!(settled.journal_checksum_verified());
    assert_eq!(settled.in_flight, None, "no unresolved journal entry");
    assert_eq!(
        settled.fence_generation, 2,
        "the fence advanced exactly once (acquired 1, released 2)"
    );

    // The PHYSICAL effect of the raced migration: the created `events`
    // table answers a typed builder query (a query against a missing
    // table errors) — not only the journal projection.
    let mut read_events = Batch::new();
    read_events.id("read-raced-table");
    read_events.query("sel-events", Query::from("events"));
    let events = tokio::time::timeout(REQUEST_TIMEOUT, client.execute(DB, read_events.build()))
        .await
        .expect("table read deadline")
        .expect("the migrated table must be queryable");
    assert!(
        events.results.contains_key("sel-events"),
        "the raced migration physically created the `events` table"
    );

    // A repeat refuses without a second event.
    match client.apply_next(DB, target, set).await {
        Err(MigrationError::NothingToApply) => {}
        other => panic!("expected NothingToApply for the post-race repeat, got {other:?}"),
    }
    let unchanged = client.status(DB, target, set).await.expect("status");
    assert_eq!(
        unchanged.applied.len(),
        1,
        "still exactly one applied event"
    );
    assert_eq!(
        unchanged.fence_generation, 2,
        "the fence did not move again"
    );
    guard.mark("race durable state verified");
}

async fn run_race_scenario(guard: &FlakeGuard, mode: RaceMode) {
    let fixture = race_fixture(guard).await;
    let RaceFixture {
        temp,
        handle,
        client_a,
        client_b,
        set,
        target,
    } = fixture;
    let _temp = temp; // the persistent directory lives for the whole test

    // Each connection derives ITS OWN request; both MUST arrive at the
    // identical typed request (same status-derived predecessor/generation
    // and body/checksum). In preflight mode the equivalent derivation
    // happens inside `apply_next`'s status preflight.
    let request_a = match mode {
        RaceMode::Raw => Some(status_derived_raw_request(&client_a, target, &set).await),
        RaceMode::Preflight => None,
    };
    let request_b = match mode {
        RaceMode::Raw => Some(status_derived_raw_request(&client_b, target, &set).await),
        RaceMode::Preflight => None,
    };
    if let (Some(a), Some(b)) = (&request_a, &request_b) {
        assert_eq!(
            a, b,
            "both independent connections must derive the SAME typed apply request"
        );
    }

    // The SET of outcomes, WITHOUT pinning the number of successes: each
    // applier either applied or refused typed. TWO `applied` results are a
    // LEGAL outcome (the already-applied fast path returns an identical
    // successful response to the loser). The durable journal (asserted
    // below) is what pins the single application exactly once.
    let gate = Arc::new(Barrier::new(2));
    let (gate_a, gate_b) = (Arc::clone(&gate), Arc::clone(&gate));
    guard.mark("releasing the two-applier race gate");
    let (out_a, out_b) = tokio::join!(
        async {
            gate_a.wait().await;
            match mode {
                RaceMode::Preflight => {
                    let applied = client_a.apply_next(DB, target, &set).await;
                    classify_preflight(&applied)
                }
                RaceMode::Raw => {
                    let response =
                        send_raw_apply(&client_a, request_a.expect("raw request a")).await;
                    classify_raw(&response)
                }
            }
        },
        async {
            gate_b.wait().await;
            match mode {
                RaceMode::Preflight => {
                    let applied = client_b.apply_next(DB, target, &set).await;
                    classify_preflight(&applied)
                }
                RaceMode::Raw => {
                    let response =
                        send_raw_apply(&client_b, request_b.expect("raw request b")).await;
                    classify_raw(&response)
                }
            }
        },
    );
    // AT LEAST ONE success: the raced event is durably applied, so both
    // refusals are impossible (the winner's apply must return `Ok`).
    let mut applied: Vec<&AppliedResponse> = Vec::new();
    for outcome in [&out_a, &out_b] {
        if let RaceOutcome::Applied(response) = outcome {
            applied.push(response);
        }
    }
    assert!(
        !applied.is_empty(),
        "at least one applier must succeed (the event is applied exactly \
         once; both refusals are impossible), got {out_a:?} / {out_b:?}"
    );
    if let [left, right] = applied[..] {
        // Both applied responses come from the SAME stored event (the fast
        // path replays the stored response), so they must be identical.
        assert_eq!(left, right, "both successful responses must be identical");
    }
    guard.note("race outcomes", format!("{out_a:?} / {out_b:?}"));

    assert_race_durable_state(guard, &client_a, target, &set).await;

    client_a.close().await;
    client_b.close().await;
    guard.mark("race shutdown");
    shutdown_clean(guard, handle, "race shutdown").await;
}

/// Big-stack harness (same mechanism as `rust_migration_tests.rs`): the
/// deep server + client async frames overflow a default debug test stack.
/// The guard is created INSIDE the spawned thread: the guard is only moved
/// across a thread boundary if it is `Send`, and instantiating it on the
/// test thread itself keeps the harness unconditionally sound.
macro_rules! big_stack_test {
    // `$mode` is an enum variant expression (`RaceMode::...`); the scenario
    // is invoked INSIDE the spawned thread so the `&guard` borrow never
    // escapes into a moved closure/future.
    ($name:ident, $scenario:ident $(, $mode:expr)?) => {
        #[test]
        fn $name() {
            std::thread::Builder::new()
                .stack_size(64 * 1024 * 1024)
                .spawn(|| {
                    let guard = FlakeGuard::new(stringify!($name));
                    tokio::runtime::Builder::new_multi_thread()
                        .worker_threads(4)
                        .thread_stack_size(16 * 1024 * 1024)
                        .enable_all()
                        .build()
                        .expect("test runtime")
                        .block_on(Box::pin(async {
                            tokio::time::timeout(
                                TEST_TIMEOUT,
                                Box::pin($scenario(&guard $(, $mode)?)),
                            )
                            .await
                            .expect(
                                "the whole scenario must finish inside the hang-guard \
                                 deadline",
                            );
                        }));
                })
                .expect("spawn big-stack test thread")
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        }
    };
}

big_stack_test!(
    migration_real_server_restart_same_directory_prefix_fence_repeat_and_drift,
    run_restart_scenario
);
big_stack_test!(
    migration_real_server_concurrent_appliers_one_gate_status_preflight_exactly_one_applied_event,
    run_race_scenario,
    RaceMode::Preflight
);
big_stack_test!(
    migration_real_server_concurrent_appliers_one_gate_raw_typed_request_exactly_one_applied_event,
    run_race_scenario,
    RaceMode::Raw
);
