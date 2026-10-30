#[path = "../request_state.rs"]
mod request_state;
#[path = "../secrets.rs"]
mod secrets;

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use tokio::sync::{watch, RwLock};
use zeroize::Zeroize;

struct Probe(Arc<AtomicUsize>);

impl Zeroize for Probe {
    fn zeroize(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

struct Noop;

impl Wake for Noop {
    fn wake(self: Arc<Self>) {}
}

#[test]
fn snapshots_clear_and_drop_zeroize_each_owned_value_once() {
    let session = Arc::new(AtomicUsize::new(0));
    let ticket = Arc::new(AtomicUsize::new(0));
    let mut snapshots = secrets::Snapshots::new(
        Probe(Arc::clone(&session)),
        Some(Probe(Arc::clone(&ticket))),
    );
    assert!(snapshots.session().is_some());
    assert!(snapshots.ticket().is_some());
    snapshots.clear();
    assert!(snapshots.session().is_none());
    assert!(snapshots.ticket().is_none());
    assert_eq!(session.load(Ordering::SeqCst), 1);
    assert_eq!(ticket.load(Ordering::SeqCst), 1);
    drop(snapshots);
    assert_eq!(session.load(Ordering::SeqCst), 1);
    assert_eq!(ticket.load(Ordering::SeqCst), 1);

    let snapshots = secrets::Snapshots::new(
        Probe(Arc::clone(&session)),
        Some(Probe(Arc::clone(&ticket))),
    );
    drop(snapshots);
    assert_eq!(session.load(Ordering::SeqCst), 2);
    assert_eq!(ticket.load(Ordering::SeqCst), 2);
}

#[test]
fn absent_ticket_stays_absent_after_clear() {
    let session = Arc::new(AtomicUsize::new(0));
    let mut snapshots = secrets::Snapshots::new(Probe(Arc::clone(&session)), None);
    assert!(snapshots.ticket().is_none());
    snapshots.clear();
    assert!(snapshots.session().is_none());
    assert!(snapshots.ticket().is_none());
    assert_eq!(session.load(Ordering::SeqCst), 1);
}

#[test]
fn invalid_connect_validation_drops_protected_password() {
    let wiped = Arc::new(AtomicUsize::new(0));
    let result = secrets::protect_before(Probe(Arc::clone(&wiped)), || Err::<(), _>("invalid"));
    assert!(result.is_err());
    assert_eq!(wiped.load(Ordering::SeqCst), 1);
}

#[test]
fn cancelling_request_before_admission_drops_protected_password() {
    let wiped = Arc::new(AtomicUsize::new(0));
    let password = secrets::protect(Probe(Arc::clone(&wiped)));
    let state = Arc::new(RwLock::new(Some(())));
    let guard = state.try_write().expect("exclusive admission lock");
    let (_sender, mut shutdown) = watch::channel(false);
    let mut pending = Box::pin(request_state::request(
        &state,
        &mut shutdown,
        move |_client| {
            Box::pin(async move {
                drop(password);
                Ok::<(), ()>(())
            })
        },
    ));
    let waker = Waker::from(Arc::new(Noop));
    let mut context = Context::from_waker(&waker);
    assert!(matches!(pending.as_mut().poll(&mut context), Poll::Pending));
    assert_eq!(wiped.load(Ordering::SeqCst), 0);
    drop(pending);
    assert_eq!(wiped.load(Ordering::SeqCst), 1);
    drop(guard);
}
