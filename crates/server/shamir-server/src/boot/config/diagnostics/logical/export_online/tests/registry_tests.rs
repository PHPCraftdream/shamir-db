//! Registry-state unit tests (R3 step 2b): single-slot admission,
//! session/database binding, terminal outcomes, publication-vs-cancellation
//! arbitration and the Drop completion guard. Pure state — no I/O, no
//! runtime, no sleeps.

use shamir_query_types::admin::{ExportFailure, ExportPhase};
use tokio_util::sync::CancellationToken;

use super::super::{AlreadyRunning, ExportRegistry};

const OWNER: u64 = 42;
const OTHER: u64 = 77;
const DB: &str = "ops";

fn started(registry: &ExportRegistry) -> (CancellationToken, super::super::ExportRun) {
    let cancel = CancellationToken::new();
    let run = registry
        .start(OWNER, DB, "ex-1".to_string(), cancel.clone())
        .expect("first start admits");
    (cancel, run)
}

#[test]
fn start_admits_and_status_binds_owner_and_db() {
    let registry = ExportRegistry::default();
    let (_cancel, _run) = started(&registry);

    let status = registry.status(OWNER, DB).expect("owner sees own job");
    assert_eq!(status.export_id, "ex-1");
    assert_eq!(status.phase, ExportPhase::WaitingForGate);

    assert!(registry.status(OWNER, "other-db").is_none(), "db-bound");
    assert!(registry.status(OTHER, DB).is_none(), "owner-bound");
    assert!(registry.cancel(OTHER, DB).is_none(), "cancel owner-bound");
}

#[test]
fn unstamped_session_is_refused() {
    let registry = ExportRegistry::default();
    assert!(
        matches!(
            registry.start(0, DB, "ex-0".to_string(), CancellationToken::new()),
            Err(AlreadyRunning)
        ),
        "unstamped refused"
    );
    assert!(registry.status(0, DB).is_none());
}

#[test]
fn live_job_blocks_every_second_start_and_is_never_erased() {
    let registry = ExportRegistry::default();
    let (_cancel, _run) = started(&registry);

    for owner in [OWNER, OTHER] {
        assert!(
            matches!(
                registry.start(owner, DB, "ex-2".to_string(), CancellationToken::new()),
                Err(AlreadyRunning)
            ),
            "second start refused while a job is live"
        );
    }
    // The live job's record is untouched.
    assert_eq!(
        registry.status(OWNER, DB).expect("survives").export_id,
        "ex-1"
    );
}

#[test]
fn terminal_outcomes_are_reported_and_slot_is_replaceable() {
    let registry = ExportRegistry::default();
    let (_cancel, run) = started(&registry);
    run.set_phase(ExportPhase::Exporting);
    run.note_progress(3, 30, 3000);
    run.finish_failed(ExportFailure::Io);

    let status = registry.status(OWNER, DB).expect("last job survives");
    assert_eq!(status.phase, ExportPhase::Failed);
    assert_eq!(status.failure, Some(ExportFailure::Io));
    assert_eq!((status.sections, status.items, status.bytes), (3, 30, 3000));
    assert_eq!(status.artifact_name, None);

    // Cancel after a terminal state: owned, nothing to cancel.
    let receipt = registry.cancel(OWNER, DB).expect("owned");
    assert!(!receipt.cancelled);

    // A finished slot is replaceable — by any owner.
    let other = registry
        .start(OTHER, DB, "ex-3".to_string(), CancellationToken::new())
        .expect("finished slot replaceable");
    assert!(registry.status(OWNER, DB).is_none(), "old owner unbound");
    other.finish_published("dump-3.lx".to_string());
    let done = registry.status(OTHER, DB).expect("new record");
    assert_eq!(done.phase, ExportPhase::Done);
    assert_eq!(done.artifact_name.as_deref(), Some("dump-3.lx"));
    assert_eq!(done.failure, None);
}

#[test]
fn publication_arbitration_blocks_late_cancellation() {
    let registry = ExportRegistry::default();
    let (cancel, run) = started(&registry);

    assert!(!run.try_begin_publication(true), "deadline elapsed loses");
    assert!(run.try_begin_publication(false), "first begin wins");
    assert!(!run.try_begin_publication(false), "second begin refused");

    // A cancel signal racing after the window opened cannot flip the job.
    cancel.cancel();
    run.finish_cancelled();
    let status = registry.status(OWNER, DB).expect("owned");
    assert_eq!(status.phase, ExportPhase::Verifying, "not cancelled yet");
    run.finish_published("dump-1.lx".to_string());
    assert_eq!(
        registry.status(OWNER, DB).expect("owned").phase,
        ExportPhase::Done,
    );
}

#[test]
fn cancellation_before_publication_wins() {
    let registry = ExportRegistry::default();
    let (cancel, run) = started(&registry);
    assert!(registry.cancel(OWNER, DB).expect("owned").cancelled);
    assert!(cancel.is_cancelled());
    assert!(!run.try_begin_publication(false), "cancelled loses");
    run.finish_cancelled();
    assert_eq!(
        registry.status(OWNER, DB).expect("owned").phase,
        ExportPhase::Cancelled,
    );
}

#[test]
fn registry_cancel_loses_once_publication_started() {
    let registry = ExportRegistry::default();
    let (cancel, run) = started(&registry);

    assert!(run.try_begin_publication(false), "publication begins");
    assert!(
        !cancel.is_cancelled(),
        "token untouched before the cancel request"
    );

    // A cancel request arriving inside the publication window is
    // refused: no signal is delivered and the receipt reports false.
    let receipt = registry.cancel(OWNER, DB).expect("owned");
    assert!(!receipt.cancelled, "publication is noncancellable");
    assert!(!cancel.is_cancelled(), "no signal delivered");

    run.finish_published("dump-2.lx".to_string());
    assert_eq!(
        registry.status(OWNER, DB).expect("owned").phase,
        ExportPhase::Done,
    );
    // After the terminal state a cancel request still resolves as owned
    // but reports that there was nothing to cancel.
    let late = registry.cancel(OWNER, DB).expect("owned");
    assert!(!late.cancelled);
}

#[test]
fn registry_cancel_wins_when_already_requested() {
    let registry = ExportRegistry::default();
    let (cancel, run) = started(&registry);

    // The token was already cancelled (e.g. by the deadline path or a
    // direct runner-side signal); the registry still answers owned and
    // confirms the live job as cancellable, and publication is refused.
    cancel.cancel();
    let receipt = registry.cancel(OWNER, DB).expect("owned");
    assert!(receipt.cancelled, "live job reports cancellable");
    assert!(
        !run.try_begin_publication(false),
        "cancelled cannot publish"
    );
    run.finish_cancelled();
    assert_eq!(
        registry.status(OWNER, DB).expect("owned").phase,
        ExportPhase::Cancelled,
    );
}

#[test]
fn dropped_run_marks_failed_internal_unless_completed() {
    let registry = ExportRegistry::default();
    {
        let (_cancel, run) = started(&registry);
        drop(run); // runner vanished without recording an outcome
    }
    let failed = registry.status(OWNER, DB).expect("record kept");
    assert_eq!(failed.phase, ExportPhase::Failed);
    assert_eq!(failed.failure, Some(ExportFailure::Internal));

    // A cancelled-but-unrecorded run is marked Cancelled, not Failed.
    let registry = ExportRegistry::default();
    {
        let (cancel, run) = started(&registry);
        cancel.cancel();
        drop(run);
    }
    assert_eq!(
        registry.status(OWNER, DB).expect("record kept").phase,
        ExportPhase::Cancelled,
    );

    // A completed run keeps its explicit outcome on drop.
    let registry = ExportRegistry::default();
    {
        let (_cancel, run) = started(&registry);
        run.finish_published("dump-9.lx".to_string());
    }
    let done = registry.status(OWNER, DB).expect("record kept");
    assert_eq!(done.phase, ExportPhase::Done);
    assert_eq!(done.failure, None);
}
