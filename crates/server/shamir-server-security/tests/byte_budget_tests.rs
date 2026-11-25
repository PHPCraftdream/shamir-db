use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use shamir_server_security::byte_budget::ByteBudget;
use shamir_tx::{CancelHandle, WaitState};

#[derive(Default)]
struct WakeFlag(AtomicBool);

impl Wake for WakeFlag {
    fn wake(self: Arc<Self>) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[test]
fn cancellation_wakes_parked_budget_waiter_without_releasing_holder() {
    let budget = ByteBudget::new(Some(8));
    let handle = CancelHandle::default();
    let wake = Arc::new(WakeFlag::default());
    let waker = Waker::from(Arc::clone(&wake));
    let mut cx = Context::from_waker(&waker);
    let Poll::Ready(held) = Box::pin(budget.acquire(8)).as_mut().poll(&mut cx) else {
        panic!("uncontended acquire parked");
    };
    let mut waiter = Box::pin(budget.acquire_cancellable(1, &handle));
    assert!(waiter.as_mut().poll(&mut cx).is_pending());
    assert_eq!(handle.waiting(), WaitState::Budget);
    assert!(!wake.0.load(Ordering::Relaxed));

    assert_eq!(handle.try_cancel(), Ok(()));
    assert!(wake.0.load(Ordering::Relaxed));
    assert!(matches!(waiter.as_mut().poll(&mut cx), Poll::Ready(None)));
    assert_eq!(handle.waiting(), WaitState::None);
    assert_eq!(budget.used(), 8);
    drop(held);
    assert_eq!(budget.used(), 0);
}

#[test]
fn cancellation_takes_precedence_when_released_budget_is_already_ready() {
    let budget = ByteBudget::new(Some(8));
    let handle = CancelHandle::default();
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(held) = Box::pin(budget.acquire(8)).as_mut().poll(&mut cx) else {
        panic!("uncontended acquire parked");
    };
    let mut waiter = Box::pin(budget.acquire_cancellable(8, &handle));
    assert!(waiter.as_mut().poll(&mut cx).is_pending());
    drop(held);
    assert_eq!(handle.try_cancel(), Ok(()));
    assert!(matches!(waiter.as_mut().poll(&mut cx), Poll::Ready(None)));
    drop(waiter);
    assert_eq!(handle.waiting(), WaitState::None);
    assert_eq!(budget.used(), 0);

    for budget in [budget, ByteBudget::unbounded()] {
        let mut cancelled = Box::pin(budget.acquire_cancellable(1, &handle));
        assert!(matches!(
            cancelled.as_mut().poll(&mut cx),
            Poll::Ready(None)
        ));
        assert_eq!(handle.waiting(), WaitState::None);
        assert_eq!(budget.used(), 0);
    }
}

#[test]
fn budget_release_and_future_drop_both_clear_the_park() {
    let budget = ByteBudget::new(Some(8));
    let handle = CancelHandle::default();
    let mut cx = Context::from_waker(Waker::noop());
    let Poll::Ready(Some(held)) = Box::pin(budget.acquire_cancellable(8, &handle))
        .as_mut()
        .poll(&mut cx)
    else {
        panic!("uncontended cancellable acquire failed");
    };
    assert_eq!(handle.waiting(), WaitState::None);

    let mut abandoned = Box::pin(budget.acquire_cancellable(8, &handle));
    assert!(abandoned.as_mut().poll(&mut cx).is_pending());
    assert_eq!(handle.waiting(), WaitState::Budget);
    drop(abandoned);
    assert_eq!(handle.waiting(), WaitState::None);
    assert_eq!(budget.used(), 8);

    let mut waiter = Box::pin(budget.acquire_cancellable(8, &handle));
    assert!(waiter.as_mut().poll(&mut cx).is_pending());
    drop(held);
    let Poll::Ready(Some(acquired)) = waiter.as_mut().poll(&mut cx) else {
        panic!("released bytes were not acquired");
    };
    assert_eq!(handle.waiting(), WaitState::None);
    assert_eq!(acquired.bytes_reserved(), 8);
    assert_eq!(budget.used(), 8);
    drop(acquired);
    assert_eq!(budget.used(), 0);
}
