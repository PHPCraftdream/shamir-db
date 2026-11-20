use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use shamir_connect::common::types::{BindingMode, TransportKind};
use shamir_connect::server::audit_chain::{
    AuditAppender, AuditChain, AuditChainWriter, AuditEntry,
};
use shamir_connect::server::conn_services::ConnectionServices;
use shamir_connect::server::session::{Session, SessionPermissions, SessionStore};
use shamir_db::{ShamirDb, SystemStoreConfig};
use shamir_query_builder::{batch::Batch, wire::admin::cancel_request};
use shamir_query_types::admin::activity::{CancelOutcome, RequestExecId};
use shamir_query_types::wire::{DbRequest, DbResponse};
use shamir_test_diag::FlakeGuard;
use shamir_tx::{CancelHandle, CancelState};
use tokio::sync::oneshot;

use super::super::cancel::cancel_entry;
use super::super::requests::{ActivityRequests, RequestEntry};
use crate::audit_appender::FjallAuditAppender;
use crate::db_handler::handler::ShamirDbHandler;
use crate::db_handler::repo_audit::enqueue_cancel_refusal_audit;
use crate::version::CURRENT_QUERY_LANG_VERSION;

const TIMEOUT: Duration = Duration::from_secs(120);

fn session(user: u8, sid: u8, superuser: bool) -> Session {
    let mut session = Session::new(
        [user; 16],
        "same-display-name".to_owned(),
        SessionPermissions::new(superuser, false, Vec::new()),
        TransportKind::Tcp,
        BindingMode::None,
        [0; 32],
        1_000,
    );
    session.session_id = [sid; 32];
    session
}

#[test]
fn state_word_commit_vs_cancel() {
    let diag = FlakeGuard::with_watchdog("state_word_commit_vs_cancel", TIMEOUT);
    let (start, jobs) = mpsc::channel::<Arc<CancelHandle>>();
    let (ready, at_start) = mpsc::channel();
    let (go, release) = mpsc::channel();
    let (send_result, result) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        while let Ok(handle) = jobs.recv_timeout(TIMEOUT) {
            ready.send(()).unwrap();
            release.recv_timeout(TIMEOUT).unwrap();
            send_result.send(handle.try_commit()).unwrap();
        }
    });
    for id in 0..4_096 {
        let entry = RequestEntry::new(
            RequestExecId(id),
            &session(1, 1, false),
            "app".to_owned(),
            "write",
            None,
        );
        start.send(Arc::clone(&entry.cancel)).unwrap();
        at_start.recv_timeout(TIMEOUT).unwrap();
        // Pin both orderings before stressing scheduler-selected races.
        let (outcome, committed) = match id {
            0 => {
                let outcome = cancel_entry(&entry);
                go.send(()).unwrap();
                (outcome, result.recv_timeout(TIMEOUT).unwrap())
            }
            1 => {
                go.send(()).unwrap();
                let committed = result.recv_timeout(TIMEOUT).unwrap();
                (cancel_entry(&entry), committed)
            }
            _ => {
                go.send(()).unwrap();
                let outcome = cancel_entry(&entry);
                (outcome, result.recv_timeout(TIMEOUT).unwrap())
            }
        };
        if id < 2 {
            assert_eq!(committed, id == 1);
        }
        if committed {
            assert_eq!(outcome, CancelOutcome::AlreadyCommitting);
            assert!(entry.cancel.finish());
            assert_eq!(entry.cancel.state(), CancelState::Done);
        } else {
            assert_eq!(outcome, CancelOutcome::Cancelled);
            assert!(!entry.cancel.finish());
            assert_eq!(entry.cancel.state(), CancelState::Cancelled);
        }
        assert_eq!(cancel_entry(&entry), CancelOutcome::AlreadyFinished);
        assert!(!entry.cancel.try_commit());
    }
    drop(start);
    worker.join().unwrap();
    diag.mark("all commit/cancel races checked");
}

#[test]
fn follower_apply_not_cancellable() {
    // Synthetic only: replication apply is not registered in production.
    let table = Arc::new(ActivityRequests::new());
    let mut entry = RequestEntry::new(
        RequestExecId(99),
        &session(1, 1, false),
        "app".to_owned(),
        "replication_apply",
        None,
    );
    entry.cancellable = false;
    let guard = table.register(entry);
    let entry = table.get(99).unwrap();
    assert_eq!(cancel_entry(&entry), CancelOutcome::NotCancellable);
    assert_eq!(entry.cancel.state(), CancelState::Running);
    assert!(guard.finish());
    assert_eq!(entry.cancel.state(), CancelState::Done);
    let still_registered = table.get(99).expect("finished entry is still registered");
    assert!(Arc::ptr_eq(&entry, &still_registered));
    assert_eq!(
        cancel_entry(&still_registered),
        CancelOutcome::AlreadyFinished
    );
}

#[derive(Default)]
struct CapturedAudit(Mutex<Vec<AuditEntry>>);

impl AuditAppender for CapturedAudit {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        self.0.lock().unwrap().push(entry.clone());
        Ok(())
    }

    fn checkpoint(&self, _: u64, _: &[u8; 32]) -> Result<(), std::io::Error> {
        Ok(())
    }
}

#[tokio::test]
async fn cancel_refusal_audit_has_exact_payload() {
    let captured = Arc::new(CapturedAudit::default());
    let writer = Arc::new(AuditChainWriter::new(
        AuditChain::new([7; 32]),
        captured.clone(),
    ));
    let caller = session(2, 3, true);
    let target = RequestExecId(u64::MAX - 1);
    for outcome in [
        CancelOutcome::Cancelled,
        CancelOutcome::AlreadyFinished,
        CancelOutcome::AlreadyCommitting,
        CancelOutcome::NotCancellable,
    ] {
        let job = enqueue_cancel_refusal_audit(&writer, &caller, target, &outcome);
        if matches!(
            outcome,
            CancelOutcome::Cancelled | CancelOutcome::AlreadyFinished
        ) {
            assert!(job.is_none());
        } else {
            tokio::time::timeout(TIMEOUT, job.expect("refusal audit admitted"))
                .await
                .expect("refusal audit timed out")
                .unwrap();
        }
    }
    let entries = captured.0.lock().unwrap();
    assert_eq!(entries.len(), 2);
    for (entry, outcome) in entries
        .iter()
        .zip(["already_committing", "not_cancellable"])
    {
        assert_eq!(entry.event, "cancel_request");
        assert_eq!(entry.result, outcome);
        assert_eq!(entry.user, caller.username);
        assert_eq!(entry.session_id_prefix, [3; 8]);
        let actual: serde_json::Value =
            rmp_serde::from_slice(&entry.details_canonical_msgpack).unwrap();
        assert_eq!(actual.as_object().unwrap().len(), 2);
        assert_eq!(actual["target"].as_u64(), Some(u64::MAX - 1));
        assert_eq!(actual["outcome"].as_str(), Some(outcome));
    }
}

#[tokio::test]
async fn cancel_request_authorizes_by_user_before_refusing_or_cancelling() {
    tokio::time::timeout(
        TIMEOUT,
        Box::pin(async {
            let root = tempfile::tempdir().unwrap();
            let db = ShamirDb::init(SystemStoreConfig::Fjall(root.path().join("system")))
                .await
                .unwrap();
            let handler = ShamirDbHandler::new(Arc::new(db));
            let store = SessionStore::new();
            let owner = store.insert([1; 32], session(1, 1, false));
            let same_user = store.insert([2; 32], session(1, 2, false));
            let outsider = store.insert([3; 32], session(2, 3, false));
            let manager = store.insert([4; 32], session(3, 4, true));
            assert_ne!(owner.activity_handle(), same_user.activity_handle());
            let live = handler
                .try_track_activity_request(&owner, "app", "read", None)
                .unwrap();
            let protected = handler
                .try_track_activity_request_with_cancellability(
                    &owner, "app", "tx_open", None, false,
                )
                .unwrap();
            for id in [live.id(), protected.id()] {
                assert!(matches!(
                    Box::pin(handler.cancel_request(&outsider, id)).await,
                    DbResponse::Error { code, .. } if code == "permission_denied"
                ));
            }
            assert_eq!(live.cancel_handle().state(), CancelState::Running);
            assert!(matches!(
                Box::pin(handler.cancel_request(&manager, protected.id())).await,
                DbResponse::CancelOutcome {
                    outcome: CancelOutcome::NotCancellable
                }
            ));
            assert!(matches!(
                Box::pin(handler.cancel_request(&same_user, live.id())).await,
                DbResponse::CancelOutcome {
                    outcome: CancelOutcome::Cancelled
                }
            ));
            let id = live.id();
            let handle = Arc::clone(live.cancel_handle());
            assert!(!live.finish());
            drop(live);
            assert_eq!(handle.state(), CancelState::Cancelled);
            assert!(matches!(
                Box::pin(handler.cancel_request(&same_user, id)).await,
                DbResponse::CancelOutcome {
                    outcome: CancelOutcome::AlreadyFinished
                }
            ));
        }),
    )
    .await
    .expect("authorization test timed out");
}

struct GatedDurableAudit {
    durable: Arc<FjallAuditAppender>,
    first: Mutex<Option<(oneshot::Sender<()>, mpsc::Receiver<()>)>>,
    refusal_persisted: Mutex<Option<oneshot::Sender<()>>>,
}

impl AuditAppender for GatedDurableAudit {
    fn append_entry(&self, entry: &AuditEntry) -> Result<(), std::io::Error> {
        let first = self.first.lock().unwrap().take();
        if let Some((entered, release)) = first {
            // AuditChainWriter::append owns its durable gate during this callback.
            entered.send(()).unwrap();
            release
                .recv_timeout(TIMEOUT)
                .map_err(std::io::Error::other)?;
        }
        self.durable.append_entry(entry)?;
        if entry.event != "cancel_request" {
            return Ok(());
        }
        if let Some(persisted) = self.refusal_persisted.lock().unwrap().take() {
            persisted.send(()).unwrap();
        }
        Ok(())
    }

    fn checkpoint(&self, seq: u64, hmac: &[u8; 32]) -> Result<(), std::io::Error> {
        self.durable.checkpoint(seq, hmac)
    }
}

#[tokio::test]
async fn cancel_refusal_replies_while_durable_audit_gate_is_held() {
    let root = tempfile::tempdir().unwrap();
    let diag = FlakeGuard::with_watchdog(
        "cancel_refusal_replies_while_durable_audit_gate_is_held",
        TIMEOUT,
    );
    let db = tokio::time::timeout(
        TIMEOUT,
        Box::pin(ShamirDb::init(SystemStoreConfig::Fjall(
            root.path().join("system"),
        ))),
    )
    .await
    .expect("Fjall init timed out")
    .unwrap();
    let audit_dir = root.path().join("audit");
    let (entered, at_gate) = oneshot::channel();
    let (release_gate, release) = mpsc::sync_channel(1);
    let (persisted, on_disk) = oneshot::channel();
    let appender = Arc::new(GatedDurableAudit {
        durable: FjallAuditAppender::open_strict(&audit_dir).unwrap(),
        first: Mutex::new(Some((entered, release))),
        refusal_persisted: Mutex::new(Some(persisted)),
    });
    let key = [7; 32];
    let writer = Arc::new(AuditChainWriter::new(AuditChain::new(key), appender));
    let handler = ShamirDbHandler::new(Arc::new(db)).with_audit_writer(Arc::clone(&writer));
    let store = SessionStore::new();
    let owner = store.insert([1; 32], session(1, 1, false));
    let control = store.insert([2; 32], session(1, 2, false));
    let live = handler
        .try_track_activity_request(&owner, "app", "write", None)
        .unwrap();
    assert!(live.cancel_handle().try_commit());
    let first = tokio::task::spawn_blocking(move || {
        writer.append("test_first", "tcp", "", "", [0; 8], "ok", Vec::new(), 1)
    });
    tokio::time::timeout(TIMEOUT, at_gate)
        .await
        .expect("first append did not reach the durable gate")
        .unwrap();
    let mut batch = Batch::new();
    batch.op("cancel_request", cancel_request(live.id()));
    let request = DbRequest::Execute {
        query_version: CURRENT_QUERY_LANG_VERSION,
        db: "app".into(),
        batch: batch.build(),
    };
    let conn = ConnectionServices::without_push(0);
    let bytes = tokio::time::timeout(
        TIMEOUT,
        Box::pin(handler.handle_decoded(&control, request, &conn)),
    )
    .await
    .expect("cancel reply waited for the audit writer")
    .unwrap();
    let reply: DbResponse = rmp_serde::from_slice(&bytes).unwrap();
    assert!(matches!(
        reply,
        DbResponse::CancelOutcome {
            outcome: CancelOutcome::AlreadyCommitting
        }
    ));
    // A synchronous regression can stall this runtime's timer, but then the
    // appender's independent deadline drops its receiver and this send fails.
    release_gate
        .try_send(())
        .expect("first append stopped waiting before the reply");
    diag.mark("typed refusal received before releasing durable gate");
    tokio::time::timeout(TIMEOUT, first)
        .await
        .expect("first append timed out")
        .unwrap()
        .unwrap();
    tokio::time::timeout(TIMEOUT, on_disk)
        .await
        .expect("refusal was not persisted")
        .unwrap();
    let entries = FjallAuditAppender::read_log_for_verify(&audit_dir).unwrap();
    AuditChain::verify_chain(&key, &entries).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].event, "test_first");
    let refusal = &entries[1];
    assert_eq!(refusal.event, "cancel_request");
    assert_eq!(refusal.result, "already_committing");
    assert_eq!(refusal.session_id_prefix, [2; 8]);
    let details: serde_json::Value =
        rmp_serde::from_slice(&refusal.details_canonical_msgpack).unwrap();
    assert_eq!(details.as_object().unwrap().len(), 2);
    assert_eq!(details["target"].as_u64(), Some(live.id().0));
    assert_eq!(details["outcome"].as_str(), Some("already_committing"));
    assert!(live.finish());
}
