use shamir_connect::common::crypto::StoredKey;
use shamir_connect::common::kdf_params::KdfParams;
use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::admin::UserDirectory;
use shamir_connect::server::user_record::UserRecord;
use shamir_connect::server::{Session, SessionPermissions};
use shamir_db::access::{principal64, Actor};
use shamir_db::engine::repo::{BoxRepoFactory, RepoConfig};
use shamir_query_builder::ddl;
use shamir_query_types::batch::BatchOp;
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use super::*;

/// The shutdown test drives the PRODUCTION surfaces end to end: the request
/// is admitted through the launcher-installed [`MutationTasks::spawn`] (the
/// real admission fence, not a test-installed tracker), the export is a REAL
/// job on the boot-built [`OnlineExport`] service (superuser inserted into
/// the server's own directory and session store through the
/// `#[cfg(test)]` `ActivityGaugeTestSeam`), and the join stage acknowledges
/// its entry through an INSTANCE-scoped one-shot probe on the very
/// `MutationTasks` instance the shutdown joins — never a process-global
/// static. The export worker is parked PAST its stage budget (it is queued
/// behind a blocking-pool job the shutdown cannot cancel), so the report
/// must name BOTH bounded-drain stages specifically and the repository
/// flush must never have started.
///
/// Mutants this kills:
/// - the mutation-worker join stage removed or moved after the flush: the
///   join-entry probe never fires, or the flush marker fires while the
///   worker is parked;
/// - `MutationTasks::spawn` admitting after `close` (or the refusal branch
///   swapped for admission): the test's pre-shutdown spawn still succeeds,
///   but the drain then cannot settle and the assertion on the flush
///   marker/report fails;
/// - the export-drain arm (stage 3b) dropped, or its wait re-fixed to a
///   private 60 s clock past the early cutoff: the report loses the
///   "online export worker still draining" line;
/// - a flush that runs despite `quiesced = false`: the flush marker fires;
/// - `MutationTasks::spawn` admitting AFTER `close` (post-close admission):
///   the late-admission assertion below fails.
///
/// What stays MODELLED, and why: (1) the parked "mutation worker" is an
/// admitted no-op future parked on a channel, not a real handler batch —
/// parking a REAL mid-mutation handler worker deterministically would
/// need a park seam inside the engine's commit path (a cross-crate test
/// hook that does not exist); the admission fence, the join and the
/// budget expiry it exercises are the production surfaces. (2) The
/// export worker's QUEUED-never-started state is PROVEN, not only
/// intended: the pool is parked BEFORE `service.start`, and
/// `BlockingPoolOccupant::await_started` proves the runtime's single
/// blocking thread is occupied before the start runs — so the worker
/// closure, which executes on the blocking pool, cannot have begun
/// before shutdown cancelled its token. The start's
/// authorization/warm-up completes on the async path (asserted: start
/// returns a receipt and the registry row is visible); what is NOT
/// exercised is a worker parked MID-FREEZE, which would need a hook
/// inside the writer, and none exists. (3) The flush marker is the
/// process-global `arm_repo_flush_probe`, which fires on THIS shutdown
/// owner's flush path — immediately BEFORE the real `flush_all_report`
/// call in `server_handle/core.rs`, so a skipped flush leaves it
/// unfired (the property under test). An INSTANCE-LOCAL marker is
/// possible in principle — the probe fires inside `shutdown_with_budget`,
/// which can reach per-instance state — but would need a `#[cfg(test)]`
/// field threaded through `ServerHandle` → `take_shutdown_owner` →
/// `ShutdownOwner` AND rewiring the other global-probe users, i.e.
/// production-surface churn disproportionate to the additional oracle;
/// the global slot is the established seam every shutdown test already
/// arms (one server instance per test at a time).
#[test]
fn shutdown_drains_a_production_mutation_worker_and_a_real_export_before_the_flush() {
    on_single_blocking_thread(|| async {
        timeout(BOUND, run_production_shutdown_order())
            .await
            .expect("production shutdown-order test bounded");
    });
}

const DB: &str = "app";

async fn run_production_shutdown_order() {
    let guard = FlakeGuard::with_watchdog(
        "shutdown_drains_a_production_mutation_worker_and_a_real_export_before_the_flush",
        Duration::from_secs(300),
    );
    try_install_global_tracing_ring();
    let temp = tempfile::tempdir().expect("test root");
    let data_dir = temp.path().join("data");
    create_private_dir(&data_dir).expect("private data directory");

    // A configured export root is what makes the launcher build the REAL
    // `OnlineExport` service and its shared maintenance gate.
    let mut config = bind_config(
        data_dir.clone(),
        "127.0.0.1:0".to_owned(),
        "127.0.0.1:0".to_owned(),
    );
    config.security.export_root = Some(temp.path().join("exports"));

    let handle = ServerLauncher::new(config)
        .with_bootstrap(BootstrapMode::Skip)
        .launch()
        .await
        .expect("launch real server");
    guard.mark("launched");

    let seam = handle
        .activity_test_seam
        .as_ref()
        .expect("activity test seam");
    let sessions = Arc::clone(&seam.session_store);
    let user_dir = Arc::clone(&seam.user_dir);
    let tracker = Arc::clone(
        handle
            .owned_mutation_tasks
            .as_ref()
            .expect("the launcher-installed production mutation tracker"),
    );
    let service = Arc::clone(
        handle
            .online_export
            .as_ref()
            .expect("the launcher-built online export service"),
    );
    let db = Arc::clone(&handle.shamir);

    // A REAL superuser, in the SAME directory the export authorization
    // reads, with a REAL session in the server's own store.
    let user = user_dir
        .insert(
            "export-owner".into(),
            UserRecord {
                salt: [0x61; 16],
                stored_key: StoredKey([0x72; 32]),
                server_key: Zeroizing::new([0x73; 32]),
                kdf_params: KdfParams::DEFAULT,
                tickets_invalid_before_ns: 0,
            },
        )
        .expect("insert the export owner");
    user_dir
        .set_superuser("export-owner", true, 0)
        .expect("make the export owner a superuser");
    let session = sessions.insert(
        [0xA1; 32],
        Session::new(
            user,
            "export-owner".into(),
            SessionPermissions::new(true, false, vec![]),
            TransportKind::Tcp,
            BindingMode::TlsExporter,
            [0; 32],
            0,
        ),
    );
    guard.mark("superuser session in the server's own store");

    // Real scope content so the export start authorizes and warms.
    let actor = Actor::User(principal64(user));
    db.create_db_as(DB, actor.clone())
        .await
        .expect("create the export database");
    db.add_repo_as(
        DB,
        RepoConfig::new("main", BoxRepoFactory::fjall_raw(data_dir.join("main"))),
        actor,
    )
    .await
    .expect("create the export repository");
    guard.mark("export scope materialized");

    // A PRODUCTION-ENROLLED mutation worker: admitted through the real
    // fence BEFORE shutdown closes it, parked on a channel the test
    // releases only after the report is in hand (never a sleep).
    let (park_tx, park_rx) = oneshot::channel::<()>();
    let worker = tracker
        .spawn(async move {
            let _ = park_rx.await;
        })
        .expect("the production admission fence must admit before shutdown");

    // The instance-scoped join-entry acknowledgment: proves the shutdown
    // really entered THIS tracker's join stage.
    let (join_tx, join_rx) = oneshot::channel::<()>();
    tracker.arm_join_entry_probe(move || {
        let _ = join_tx.send(());
    });

    // The flush marker: fired only if the REAL flush call is entered. A
    // shutdown with unquiesced workers must never reach it.
    let flush_entered = Arc::new(AtomicBool::new(false));
    let _flush_probe = {
        let flush_entered = Arc::clone(&flush_entered);
        arm_repo_flush_probe(move || flush_entered.store(true, Ordering::SeqCst))
    };

    // Park the runtime's ONLY blocking thread: the export worker is spawned
    // onto the blocking pool and therefore stays QUEUED — past the export
    // drain stage budget — no matter that shutdown already cancelled its
    // token (a queued job cannot observe cancellation).
    let occupant = BlockingPoolOccupant::park();
    occupant.await_started();
    guard.mark("blocking pool occupied");

    // Start a REAL export job on the boot-built service.
    let BatchOp::StartExport(options) = ddl::start_export(DB).repo("main").build() else {
        unreachable!("start_export builder returned another op");
    };
    let receipt = timeout(
        BOUND,
        service.start(&session, options, CancellationToken::new()),
    )
    .await
    .expect("export start bounded")
    .expect("the real export job is admitted");
    assert!(
        service
            .status_for(&session, DB)
            .is_some_and(|status| status.export_id == receipt.export_id),
        "the admitted export job must be visible in the registry before shutdown"
    );
    guard.mark("real export admitted, worker queued behind the parked pool");

    let mut shutdown = tokio::spawn(crate::runtime::drain_server(
        handle,
        REMAINING_BUDGET_DEADLINE,
    ));
    timeout(BOUND, join_rx)
        .await
        .expect("the shutdown must acknowledge entry into the mutation-join stage")
        .expect("join-entry probe channel");
    guard.mark("mutation-join stage entered");
    // Post-close admission is refused: `close()` has run (the join stage
    // is entered), so `MutationTasks::spawn` must return `None` — the
    // latch IS an admission fence, unlike a bare `TaskTracker::close`.
    // Needs no request: the refusal happens before any work is admitted.
    assert!(
        tracker.spawn(async {}).is_none(),
        "admission after shutdown closed the mutation fence must be refused"
    );
    guard.mark("post-close admission refused");
    let report = timeout(BOUND, &mut shutdown)
        .await
        .expect("the bounded drain must return while both workers are parked")
        .expect("drain task");
    guard.mark("drain returned with the workers still parked");

    let errors = report.errors();
    assert!(
        errors.iter().any(|error| error.contains(
            "owned mutation workers still running after the shutdown bound; \
                 repository flush skipped"
        )),
        "the mutation-join stage must report its own budget expiry: {report:?}"
    );
    assert!(
        errors
            .iter()
            .any(|error| error
                .contains("online export worker still draining after the shutdown bound")),
        "the export-drain stage must report its own budget expiry: {report:?}"
    );
    assert!(
        !flush_entered.load(Ordering::SeqCst),
        "the repository flush must never start while the mutation worker and the export \
         worker are unquiesced (mutant: the flush runs before or despite the drains): {report:?}"
    );
    assert!(
        !report.repository_failures().is_empty(),
        "a SKIPPED flush leaves every repository outcome unrecorded; reporting the flush as \
         complete would hide the skip: {:?}",
        report.repository_failures()
    );
    assert!(
        !errors
            .iter()
            .any(|error| error.contains("graceful shutdown exceeded deadline")),
        "the stage verdicts must come from the bounded stages, not the generic outer expiry: \
         {report:?}"
    );
    assert!(!report.is_clean(), "unquiesced shutdown must be unclean");
    assert_eq!(report.exit_code(), 70, "unclean report: {report:?}");

    // Unconditional release, then the completion handshakes.
    let _ = park_tx.send(());
    timeout(BOUND, worker)
        .await
        .expect("mutation worker join bounded")
        .expect("mutation worker task");
    drop(occupant);
    assert!(
        timeout(BOUND, service.wait_idle())
            .await
            .expect("wait_idle bounded"),
        "the queued export worker must exit once the pool is free (shutdown already \
         cancelled its run token)"
    );
    guard.mark("both workers released and joined");
}
