//! The online logical-export job runner (`OnlineExport`, R3 step 2b,
//! brief `56-r3-2b-online-export-job.md`).
//!
//! Thin orchestration over the already-landed pieces: the session-bound
//! [`ExportRegistry`] (admission + status/cancel state), the validated
//! [`ExportRoot`] (private directory + exclusive lock + stale cleanup),
//! the scope module (`authorize_scope` / `warm_scope`), the server-wide
//! [`MaintenanceGate`], and the shared artifact writer
//! (`logical_dump::write_live_export` with live staging and the
//! [`publish_live_stage`] no-clobber commit point, which owns the typed
//! post-link durability behavior — the service never infers a
//! publication outcome from the filesystem).
//!
//! Fail-closed posture: a freshly built service is DISABLED — `start`
//! refuses with [`ServiceError::Disabled`] until the launcher explicitly
//! enables it after the admission audit has enrolled every mutation
//! source in the shared gate (design §0.3: an embedding that has not
//! enrolled every mutation source must not advertise online consistent
//! export). The service itself never opens any network endpoint.
//!
//! Execution model (mirrors the backup registry precedent): `start`
//! runs on the server runtime and performs every cancellable async step
//! (scope authorization, mutation admission, warm opens, registry
//! reservation) inline, so a dropped `start` future leaves no guard and
//! no job behind. The freeze + artifact production then run on a
//! dedicated blocking worker (`runtime.spawn_blocking`) that bridges
//! the synchronous writer I/O through the captured runtime
//! `Handle::block_on`; `start` returns the [`ExportStartReceipt`] right
//! after the worker spawn succeeded, never after a terminal result.
//!
//! Task ownership: the blocking worker's tokio [`JoinHandle`] is owned
//! by a per-job async supervisor task that selects (biased) on the
//! session's `connection_closed` token and the worker completion; a
//! cancel signal cancels the run token, after
//! which the supervisor waits for the worker with a bounded timeout.
//! The worker itself selects (biased) between its cancel token and the
//! freeze future, so expiry during the DRAIN (before the writer even
//! starts) unwinds the freeze future promptly — the freeze guard and
//! every snapshot live only inside that future and are released by its
//! drop. No export is detached after a terminal result: the registry
//! job is reserved synchronously in `start`, the run ticket is owned by
//! the worker, and its `Drop` completion guard records any unrecorded
//! exit as `Failed`/`Cancelled`.
//!
//! Clocks: the engine's 600 s ceiling covers the drain plus the export
//! (`with_freeze`); the writer's own deadline leaves the standard 10 s
//! margin. The synchronous codec/file boundaries are covered by the
//! writer's `CheckedIo` token/deadline checks — a blocking-pool thread
//! cannot be aborted, so no hard cutoff is advertised across a stuck
//! synchronous syscall; shutdown waits only a bounded drain.
//!
//! Wire hygiene: errors are static typed variants with no path data, no
//! secrets and no peer text; artifact and export ids are
//! `timestamp + process-wide sequence` (one shared atomic counter), so
//! no user-controlled string ever names a file or appears in status.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use shamir_connect::server::session::Session;
use shamir_db::engine::db_instance::logical_export::MaintenanceGate;
use shamir_db::engine::table::writer_drain_barrier::WriterDrainGuard;
use shamir_db::ShamirDb;
use shamir_query_types::admin::{
    ExportCancelReceipt, ExportFailure, ExportPhase, ExportStartReceipt, ExportStatusReceipt,
    StartExportOp,
};
use shamir_server_security::user_directory::FjallUserDirectory;
use shamir_sync::replication_seed::DumpReport;
use tokio::runtime::Handle;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::registry::{AlreadyRunning, ExportRegistry, ExportRun};
use super::root::ExportRoot;
use super::scope::{authorize_scope, warm_scope, ScopeError};
use crate::boot::config::diagnostics::logical::logical_dump::{
    classify_export_failure, publish_live_stage, write_live_export, ExportObserver, ExportProgress,
    LiveExport, LiveExportFailure, LogicalExportArgs, SecuritySource,
};

/// Process-wide sequence for export ids and artifact names (brief: names
/// are `timestamp + process-wide sequence`; no user-supplied component).
static NAME_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Millis since the Unix epoch; the sequence disambiguates
/// same-millisecond names inside one process.
fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Opaque export id.
fn fresh_export_id() -> String {
    let seq = NAME_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("ex-{}-{seq:06}", unix_millis())
}

/// Opaque artifact name (shares the process-wide counter; never a
/// filesystem path on the wire).
fn fresh_artifact_name() -> String {
    let seq = NAME_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("export-{}-{seq:06}", unix_millis())
}

/// Writer deadline inside the freeze: leaves margin under the engine's
/// 600-second ceiling (same margin as the offline path's `CheckedIo`).
const WRITER_BUDGET: Duration = Duration::from_secs(590);

/// Bounded wait for the blocking worker after a shutdown/cancel signal:
/// the writer checks its token on every codec I/O boundary, so the
/// unwind is prompt; the bound only caps pathological stuck filesystem
/// I/O (a blocking-pool thread cannot be aborted) and keeps server
/// shutdown bounded. Also bounds [`OnlineExport::wait_idle`].
const THREAD_DRAIN_WAIT: Duration = Duration::from_secs(60);

/// Typed refusal/failure of [`OnlineExport::start`]. Static variants
/// only: no filesystem paths, no peer text, no catalogue strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ServiceError {
    /// The service is disabled (fail-closed default; the launcher
    /// enables it after the admission audit).
    #[error("the online export job is disabled on this server")]
    Disabled,
    /// The engine or runtime is shutting down; no job was admitted (or
    /// the worker could not be spawned; the reserved job is recorded
    /// failed by the run ticket's completion guard).
    #[error("the server is shutting down")]
    Shutdown,
    /// A job is already admitted on this server (any owner). This is
    /// also the typed answer when the maintenance gate refuses the
    /// warm-up admission: only the online export ever RAISES freeze
    /// intent (the online backup takes mutation admission, it never
    /// freezes), so freeze intent observed here means an export job is
    /// already running server-wide — the same in-progress code is
    /// returned to every caller, which also avoids leaking whether a
    /// job runs to a non-owner (registry precedent).
    #[error("an online export job is already running on this server")]
    AlreadyRunning,
    /// Current directory state is not a superuser, or an included scope
    /// failed its read/manage ACL check.
    #[error("the export scope was denied")]
    Denied,
    /// The requested scope is malformed or unresolved.
    #[error("the requested export scope is invalid")]
    InvalidScope,
    /// The scope or options carry content this phase cannot represent.
    #[error("the requested export scope is not supported")]
    Unsupported,
    /// The scope is temporarily unexportable (quarantine, rename in
    /// doubt) or a catalogue/directory read failed during warm-up.
    #[error("the export scope is temporarily unavailable")]
    Unavailable,
}

impl From<ScopeError> for ServiceError {
    fn from(scope: ScopeError) -> Self {
        match scope {
            ScopeError::Denied => Self::Denied,
            ScopeError::InvalidScope => Self::InvalidScope,
            ScopeError::Unsupported => Self::Unsupported,
            ScopeError::Unavailable => Self::Unavailable,
        }
    }
}

impl From<AlreadyRunning> for ServiceError {
    fn from(_: AlreadyRunning) -> Self {
        Self::AlreadyRunning
    }
}

/// anyhow marker wrapping a scope refusal that happened under the
/// freeze (re-authorization race), so the terminal mapping can keep the
/// typed kind instead of `Internal`.
#[derive(Debug, thiserror::Error)]
#[error("export scope refusal under freeze")]
struct ScopeRefusal(ScopeError);

/// Runtime wiring for [`OnlineExport::new`]: the handle the blocking
/// worker and supervisor run on, and the server shutdown token (each
/// job's run token is a child of it, so a shutdown cancels every job).
pub(crate) struct ExportRuntime {
    pub runtime: Handle,
    pub shutdown: CancellationToken,
}

/// The online logical-export job service.
///
/// Wraps the registry, the export root, the shared maintenance gate, the
/// engine handles and the server shutdown token. One instance per
/// server; every method is safe for concurrent sessions.
pub struct OnlineExport {
    registry: Arc<ExportRegistry>,
    root: Arc<ExportRoot>,
    gate: MaintenanceGate,
    db: Arc<ShamirDb>,
    directory: Arc<FjallUserDirectory>,
    /// Source storage epoch carried into the dump header (stamp-derived;
    /// the caller supplies it at construction).
    epoch: Option<u32>,
    runtime: ExportRuntime,
    /// Fail-closed until the launcher enables the service.
    enabled: bool,
    /// Shutdown-quiescence slot: ONE permit, held by the in-flight
    /// blocking worker for the whole run (an owned permit moved into the
    /// job). [`OnlineExport::wait_idle`] acquires it under a bounded
    /// timeout; a start refuses with [`ServiceError::AlreadyRunning`]
    /// while a worker still drains, and the registry's own single
    /// admission slot stays the primary arbitration.
    worker_slot: Arc<Semaphore>,
}

impl OnlineExport {
    /// Build the disabled service. Call [`OnlineExport::enable`] before
    /// wiring it to the admin dispatch.
    pub fn new(
        registry: Arc<ExportRegistry>,
        root: Arc<ExportRoot>,
        gate: MaintenanceGate,
        db: Arc<ShamirDb>,
        directory: Arc<FjallUserDirectory>,
        epoch: Option<u32>,
        runtime: ExportRuntime,
    ) -> Self {
        Self {
            registry,
            root,
            gate,
            db,
            directory,
            epoch,
            runtime,
            enabled: false,
            worker_slot: Arc::new(Semaphore::new(1)),
        }
    }

    /// Enable the job (launcher-only, after the admission audit).
    pub fn enable(mut self) -> Self {
        self.enabled = true;
        self
    }

    /// Shutdown quiescence: `true` when no blocking export worker still
    /// holds the single worker slot. The caller must have cancelled the
    /// shutdown token (or otherwise stopped admitting new jobs) first —
    /// `start` refuses while the token is cancelled, so a permit acquired
    /// here cannot be handed to a new worker. Bounded: the writer checks
    /// its token on every codec I/O boundary, but a blocking-pool thread
    /// cannot be aborted, so a stuck synchronous syscall can only be
    /// waited out, not killed; `false` means the bound elapsed with a
    /// worker still draining. The acquired permit is released on return
    /// (no state mutated).
    pub(crate) async fn wait_idle(&self) -> bool {
        match tokio::time::timeout(THREAD_DRAIN_WAIT, self.worker_slot.acquire()).await {
            Ok(Ok(permit)) => {
                drop(permit);
                true
            }
            _ => false,
        }
    }

    /// Start an export job for `session`, targeting the scope in
    /// `options`.
    ///
    /// Ordering (design §4.1): validate + authorize against the CURRENT
    /// state → warm opens and the bounded catalogue warm-up under ONE
    /// mutation admission → reserve the registry job (the single
    /// server-wide admission slot) → spawn the blocking worker. A
    /// denial, an invalid scope, a busy gate or a lost admission race
    /// refuses with a typed error and never touches the freeze.
    ///
    /// `connection_closed` is the session's connection token: when the
    /// session's transport dies, the supervisor cancels the job (the
    /// gate is released and the stage removed, like any other cancel).
    pub async fn start(
        &self,
        session: &Session,
        options: StartExportOp,
        connection_closed: CancellationToken,
    ) -> Result<ExportStartReceipt, ServiceError> {
        if !self.enabled {
            return Err(ServiceError::Disabled);
        }
        if self.runtime.shutdown.is_cancelled() {
            return Err(ServiceError::Shutdown);
        }
        let user_id = session.user_id;
        let owner = session.activity_handle();

        // ── authorize against the CURRENT directory/ACL state. Every
        // guard below is a plain local of this future, so a dropped or
        // cancelled `start` leaves nothing behind.
        let db_instance = authorize_scope(&self.db, &self.directory, user_id, &options)
            .await
            .map_err(ServiceError::from)?;

        // ── warm phase under ONE mutation admission: cold table opens
        // (store opens, index loads, possible repair) and the cold
        // function-folder catalogue handles must never happen inside the
        // freeze. The function-folder family is warmed with a bounded
        // point query (`load_function_folder` with a fixed empty path
        // key — at most one indexed lookup, no full-catalogue allocation
        // and no live `check_seed_catalogue` flush); the other catalogue
        // families and settings are warmed by boot and the ACL checks
        // above.
        let admission: WriterDrainGuard = self
            .gate
            .enter_mutation()
            // Freeze intent raised ⇒ an export job is running (see the
            // `AlreadyRunning` docs): the same in-progress answer.
            .map_err(|_| ServiceError::AlreadyRunning)?;
        warm_scope(&db_instance, &options)
            .await
            .map_err(ServiceError::from)?;
        self.db
            .system_store()
            .load_function_folder("")
            .await
            .map_err(|_| ServiceError::Unavailable)?;
        drop(admission);

        // ── claim the shutdown-quiescence slot BEFORE the registry
        // reservation, so the permit is held from before the spawn through
        // the worker's whole run. A permit still held by the previous
        // job's draining worker refuses with the same in-progress answer
        // as a busy registry (see `AlreadyRunning`). `try_acquire_owned`
        // cannot block and the permit cannot be dropped on any refusal
        // path below before the job takes ownership.
        let worker_slot = Arc::clone(&self.worker_slot)
            .try_acquire_owned()
            .map_err(|_| ServiceError::AlreadyRunning)?;

        // ── reserve the one registry job. From here on the worker owns
        // the outcome; a panic between reservation and the terminal
        // record is caught by the run ticket's Drop backstop (the permit
        // releases with the job).
        let cancel = self.runtime.shutdown.child_token();
        let export_id = fresh_export_id();
        let run = Arc::new(self.registry.start(
            owner,
            &options.database,
            export_id.clone(),
            cancel.clone(),
        )?);

        // ── spawn the dedicated blocking worker (synchronous codec/file
        // work bridged through the captured runtime handle). The job
        // holds `Arc<ExportRoot>`, so the root's exclusive lock lives
        // for the whole run even if the service is dropped.
        let job = ExportJob {
            gate: self.gate.clone(),
            db: Arc::clone(&self.db),
            directory: Arc::clone(&self.directory),
            root: Arc::clone(&self.root),
            options,
            epoch: self.epoch,
            user_id,
            run: Arc::clone(&run),
            cancel: cancel.clone(),
            runtime: self.runtime.runtime.clone(),
            artifact: fresh_artifact_name(),
            publication_claimed: Arc::new(AtomicBool::new(false)),
            _worker_slot: worker_slot,
        };
        // `Handle::spawn_blocking` is infallible: it always returns a
        // `JoinHandle` (shutdown only prevents NEW polls of this runtime,
        // the task is still spawned and will unwind on its cancelled
        // token).
        let mut join = self.runtime.runtime.spawn_blocking(move || job.run());

        // ── supervisor: owns the worker's JoinHandle. Biased select: a
        // cancel signal is observed before polling the join, so a
        // shutdown racing a finished worker is harmless (the ticket
        // already recorded a terminal state; `finish_*` is idempotent).
        // Shutdown cancels the run token through the child-token link,
        // so only the connection token needs an explicit forward.
        let supervisor_cancel = cancel.clone();
        // Detached by design: the supervisor lives only to forward the
        // connection-close cancel and join the worker; the worker records
        // its own terminal outcome, so an unobserved supervisor exit loses
        // nothing.
        self.runtime.runtime.spawn(async move {
            tokio::select! {
                biased;
                _ = connection_closed.cancelled() => supervisor_cancel.cancel(),
                // Poll by reference: the join is awaited again below, so
                // the select must not consume it on the cancel arm.
                _ = &mut join => return,
            }
            // Bounded drain: the worker unwinds on its token; never wait
            // unboundedly on a stuck filesystem (a blocking-pool thread
            // cannot be aborted, only waited out).
            let _ = tokio::time::timeout(THREAD_DRAIN_WAIT, join).await;
        });

        Ok(ExportStartReceipt { export_id })
    }

    /// Synchronous status of `session`'s job against the database the
    /// launcher resolved (the wire ops carry no database field; the
    /// launcher passes the session's current database): `None` when no
    /// visible record belongs to this session/database (never another
    /// owner's job). No database or filesystem access.
    pub fn status_for(&self, session: &Session, database: &str) -> Option<ExportStatusReceipt> {
        self.registry.status(session.activity_handle(), database)
    }

    /// Synchronous cancellation of `session`'s job against `database`:
    /// delivers the cancel signal onto the run token (the worker unwinds
    /// inside the freeze future; the supervisor joins it). `None` when
    /// no visible job of this session exists.
    pub fn cancel_for(&self, session: &Session, database: &str) -> Option<ExportCancelReceipt> {
        self.registry.cancel(session.activity_handle(), database)
    }
}

/// Terminal outcome of one freeze, decided structurally (token, gate
/// result) — never by matching error text.
enum FreezeOutcome {
    /// The freeze returned; the writer reported its own result (`Err`
    /// carries the driver's typed phase/projection markers, or a scope
    /// refusal, or a gate-level error reboxed).
    Completed(Result<DumpReport, anyhow::Error>),
    /// The run token fired (per-session cancel, connection close, or
    /// server shutdown) before the freeze returned.
    Cancelled,
}

/// One admitted job's runner: owns the run ticket (through the shared
/// `Arc`) and drives authorize → freeze → capture → write → publish.
/// Fields are private; `start` constructs the literal in the same
/// module, so there is no many-argument constructor.
struct ExportJob {
    gate: MaintenanceGate,
    db: Arc<ShamirDb>,
    directory: Arc<FjallUserDirectory>,
    /// Held for the whole run: the root's exclusive lock must outlive
    /// the job even if the service itself is dropped.
    root: Arc<ExportRoot>,
    options: StartExportOp,
    epoch: Option<u32>,
    user_id: [u8; 16],
    run: Arc<ExportRun>,
    cancel: CancellationToken,
    runtime: Handle,
    /// Opaque artifact name, minted once at reservation time.
    artifact: String,
    /// `true` once the publication window was claimed inside the
    /// writer's `on_publish` callback. After the claim, ANY error —
    /// even one arriving while an external cancellation has fired — is
    /// recorded as a typed failure, never as `Cancelled` (a cancel is a
    /// no-op in the publishing window, so recording it there would
    /// strand the job on the ticket's Drop backstop instead of giving
    /// status a real reason).
    publication_claimed: Arc<AtomicBool>,
    /// Shutdown-quiescence permit: held for the whole worker run so
    /// [`OnlineExport::wait_idle`] can observe quiescence; released when
    /// the closure returns (or unwinds).
    _worker_slot: tokio::sync::OwnedSemaphorePermit,
}

impl ExportJob {
    /// Blocking-worker entry point. Blocks on the runtime handle for the
    /// async facade; the freeze guard and every snapshot live only
    /// inside the (cancellable) freeze future, so a cancelled or
    /// panicked job releases the gate through unwinding. The run
    /// ticket's Drop (fired when this worker's locals unwind) is the
    /// last-resort completion guard.
    fn run(self) {
        // The writer deadline the worker hands to `CheckedIo`: the
        // standard margin under the engine ceiling, so the synchronous
        // codec checkpoints honor the same clock the select does.
        let writer_deadline = Instant::now() + WRITER_BUDGET;
        let outcome = self.runtime.block_on(self.drive(writer_deadline));
        self.record_outcome(outcome, writer_deadline);
    }

    /// Drive the whole freeze with cancellation-expiry coverage over the
    /// DRAIN: a biased select between the run token and the freeze
    /// future. Dropping the freeze future releases the gate through the
    /// guard's `Drop`; the engine's 600 s ceiling bounds the drain and
    /// reports through the gate result (a real timeout).
    async fn drive(&self, writer_deadline: Instant) -> FreezeOutcome {
        let freeze = self.freeze(writer_deadline);
        let cancelled = self.cancel.clone();
        tokio::select! {
            biased;
            _ = cancelled.cancelled() => FreezeOutcome::Cancelled,
            r = freeze => r,
        }
    }

    /// The freeze itself. The callback always returns `Ok(report)` in
    /// the outer `DbResult` (the writer's own `Err` stays boxed
    /// inside), so a gate-level `DbError` can only mean drain failure
    /// or the engine ceiling — mapped structurally below.
    async fn freeze(&self, writer_deadline: Instant) -> FreezeOutcome {
        let args = LogicalExportArgs {
            db: self.options.database.clone(),
            output: self.root.path().join(&self.artifact),
            repo: self.options.repo.clone(),
            table: self.options.table.clone(),
            schema_only: self.options.schema_only,
            include_artifacts: false,
        };
        // Publication claim + no-clobber commit. `publish_live_stage`
        // owns the typed post-commit behavior: after the hard link has
        // committed it reports success (the durability tail is a
        // published, retained artifact, never a failure), so the
        // service never infers an outcome from the filesystem.
        let publish_run = Arc::clone(&self.run);
        let claimed = Arc::clone(&self.publication_claimed);
        let output = self.root.path().join(&self.artifact);
        let claim_deadline = writer_deadline;
        let on_publish = move |stage: &Path| -> Result<()> {
            // Claim the noncancellable publication window: the token and
            // the deadline are checked under the registry's state cell,
            // so a racing per-session cancel either wins entirely or
            // has already lost.
            if !publish_run.try_begin_publication(Instant::now() >= claim_deadline) {
                bail!("export publication lost cancellation arbitration");
            }
            claimed.store(true, Ordering::SeqCst);
            publish_live_stage(stage, &output)
        };
        let result = self
            .gate
            .with_freeze(|guard| {
                // Captured exactly once, moved into the freeze future;
                // no re-derivation from `self` inside.
                let db = Arc::clone(&self.db);
                let directory = Arc::clone(&self.directory);
                let options = self.options.clone();
                let user_id = self.user_id;
                let epoch = self.epoch;
                let run = Arc::clone(&self.run);
                let cancel = self.cancel.clone();
                let writer_deadline = writer_deadline;
                let args = args;
                let on_publish = on_publish;
                Box::pin(async move {
                    // Re-authorize under the freeze (closes the
                    // authorize→freeze race), then capture the read
                    // facade pinned to the drained instant.
                    let db_instance =
                        match authorize_scope(&db, &directory, user_id, &options).await {
                            Ok(db) => db,
                            Err(scope) => return Ok(Err(anyhow::Error::new(ScopeRefusal(scope)))),
                        };
                    let reader = match db
                        .system_store()
                        .capture_logical_export(&db_instance, guard)
                        .await
                    {
                        Ok(reader) => reader,
                        Err(error) => return Ok(Err(anyhow::Error::new(error))),
                    };
                    let observer = RunObserver::new(Arc::clone(&run));
                    run.set_phase(ExportPhase::Exporting);
                    let report = write_live_export(LiveExport {
                        reader: &reader,
                        security: SecuritySource::LiveDirectory(Some(Arc::clone(&directory))),
                        // The live job reuses the server's directory
                        // handle above; no offline home is involved.
                        offline_home: None,
                        args: &args,
                        epoch,
                        live: true,
                        cancel: &cancel,
                        deadline: writer_deadline,
                        observer: &observer,
                        on_publish: Some(&on_publish),
                    })
                    .await;
                    drop(reader);
                    Ok(report)
                })
            })
            .await;
        match result {
            Ok(report) => FreezeOutcome::Completed(report),
            Err(gate_error) => {
                // Gate-level only: drain refusal or the engine's 600 s
                // ceiling. Cancellation dominates; everything else is
                // reboxed and classified structurally (the classifier's
                // clock marks an elapsed ceiling as a timeout, so a real
                // timeout stays distinct from a cancel).
                if self.cancel.is_cancelled() {
                    FreezeOutcome::Cancelled
                } else {
                    FreezeOutcome::Completed(Err(anyhow::Error::new(gate_error)))
                }
            }
        }
    }

    /// Map the terminal outcome onto the registry exactly once. The
    /// ticket's Drop is the backstop for a panic between spawn and here.
    fn record_outcome(&self, outcome: FreezeOutcome, writer_deadline: Instant) {
        match outcome {
            FreezeOutcome::Completed(Ok(report)) => {
                // The writer's verified report carries the real section,
                // item and byte totals; record them before `Done` so a
                // status polled at the terminal transition never shows
                // stale counters.
                self.run
                    .note_progress(report.sections, report.items, report.bytes);
                self.run.finish_published(self.artifact.clone());
            }
            FreezeOutcome::Completed(Err(error)) => self.record_failure(error, writer_deadline),
            FreezeOutcome::Cancelled => self.run.finish_cancelled(),
        }
    }

    /// Record a freeze-internal failure with its typed kind.
    fn record_failure(&self, error: anyhow::Error, writer_deadline: Instant) {
        // A scope refusal under the freeze keeps its typed kind; the
        // closed wire set has no `Denied` variant, so only the
        // never-representable-content case stays `UnsupportedContent`.
        if let Some(ScopeRefusal(scope)) = error.downcast_ref::<ScopeRefusal>() {
            let failure = match scope {
                ScopeError::Unsupported => ExportFailure::UnsupportedContent,
                _ => ExportFailure::Internal,
            };
            self.run.finish_failed(failure);
            return;
        }
        let claimed = self.publication_claimed.load(Ordering::SeqCst);
        if claimed {
            // Inside/after the claimed publication window a cancel is a
            // no-op in the registry — record the real typed failure
            // instead, whatever the token says now.
            let failure = match classify_export_failure(error, &self.cancel, writer_deadline) {
                LiveExportFailure::Cancelled | LiveExportFailure::WriterIo => ExportFailure::Io,
                LiveExportFailure::DeadlineExceeded => ExportFailure::TimedOut,
                LiveExportFailure::StageIo => ExportFailure::Io,
                LiveExportFailure::VerifierIo => ExportFailure::Verification,
                LiveExportFailure::UnsupportedProjection => ExportFailure::UnsupportedContent,
            };
            self.run.finish_failed(failure);
            return;
        }
        if self.cancel.is_cancelled() {
            self.run.finish_cancelled();
            return;
        }
        let failure = match classify_export_failure(error, &self.cancel, writer_deadline) {
            LiveExportFailure::Cancelled => {
                self.run.finish_cancelled();
                return;
            }
            LiveExportFailure::DeadlineExceeded => ExportFailure::TimedOut,
            LiveExportFailure::StageIo | LiveExportFailure::WriterIo => ExportFailure::Io,
            LiveExportFailure::VerifierIo => ExportFailure::Verification,
            LiveExportFailure::UnsupportedProjection => ExportFailure::UnsupportedContent,
        };
        self.run.finish_failed(failure);
    }
}

/// Live progress observer: maps writer events onto the registry's
/// counters and phase. All state is atomic; the writer calls this from
/// the runner worker only.
struct RunObserver {
    run: Arc<ExportRun>,
    sections: AtomicU64,
    items: AtomicU64,
    bytes: AtomicU64,
}

impl RunObserver {
    fn new(run: Arc<ExportRun>) -> Self {
        Self {
            run,
            sections: AtomicU64::new(0),
            items: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        }
    }
}

impl ExportObserver for RunObserver {
    fn on_export_progress(&self, event: ExportProgress) {
        match event {
            ExportProgress::ItemWritten { bytes } => {
                self.items.fetch_add(1, Ordering::Relaxed);
                let total = self.bytes.fetch_add(bytes, Ordering::Relaxed) + bytes;
                self.run.note_progress(
                    self.sections.load(Ordering::Relaxed),
                    self.items.load(Ordering::Relaxed),
                    total,
                );
            }
            ExportProgress::SectionFinished(_) => {
                let sections = self.sections.fetch_add(1, Ordering::Relaxed) + 1;
                self.run.note_progress(
                    sections,
                    self.items.load(Ordering::Relaxed),
                    self.bytes.load(Ordering::Relaxed),
                );
            }
            ExportProgress::Verifying => {
                // Pre-publication verification: report the phase at the
                // event (the publication claim would otherwise be the
                // first `Verifying` observer, which is too late for a
                // status polled during verification). Idempotent: the
                // claim's own flip lands on the same value.
                self.run.set_phase(ExportPhase::Verifying);
                self.run.note_progress(
                    self.sections.load(Ordering::Relaxed),
                    self.items.load(Ordering::Relaxed),
                    self.bytes.load(Ordering::Relaxed),
                );
            }
        }
    }
}
