use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::sync::Notify;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug)]
pub struct ConnTasks {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    drain: CancellationToken,
    abort: CancellationToken,
    live: AtomicUsize,
    idle: Notify,
}

#[derive(Clone)]
pub(crate) struct ConnScope {
    inner: Arc<Inner>,
    drain: CancellationToken,
    abort: CancellationToken,
}

struct LiveTask(Arc<Inner>);

impl LiveTask {
    fn acquire(inner: &Arc<Inner>) -> Self {
        inner.live.fetch_add(1, Ordering::AcqRel);
        Self(Arc::clone(inner))
    }
}

impl Drop for LiveTask {
    fn drop(&mut self) {
        if self.0.live.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.idle.notify_waiters();
        }
    }
}

impl ConnTasks {
    pub fn new(drain: CancellationToken) -> Self {
        Self {
            inner: Arc::new(Inner {
                drain,
                abort: CancellationToken::new(),
                live: AtomicUsize::new(0),
                idle: Notify::new(),
            }),
        }
    }

    pub(crate) fn spawn<F>(&self, future: F) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(self.scope().track(future))
    }

    pub(crate) fn scope(&self) -> ConnScope {
        ConnScope {
            inner: Arc::clone(&self.inner),
            drain: self.inner.drain.child_token(),
            abort: self.inner.abort.child_token(),
        }
    }

    pub(crate) fn live(&self) -> usize {
        self.inner.live.load(Ordering::Acquire)
    }

    pub(crate) async fn wait_idle(&self) {
        loop {
            let notified = self.inner.idle.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.live() == 0 {
                return;
            }
            notified.await;
        }
    }

    pub(crate) fn abort_all(&self) {
        self.inner.abort.cancel();
    }
}

impl ConnScope {
    pub(crate) fn drain(&self) -> &CancellationToken {
        &self.drain
    }

    pub(crate) fn track<F>(&self, future: F) -> impl Future<Output = ()> + Send + 'static
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let capture = TrackedFuture {
            future: Box::pin(future),
            _live: LiveTask::acquire(&self.inner),
            abort: self.abort.clone(),
        };
        async move {
            let mut capture = capture;
            tokio::select! {
                biased;
                _ = capture.abort.cancelled() => {}
                _ = &mut capture.future => {}
            }
            drop(capture);
        }
    }

    pub(crate) fn spawn<F>(&self, future: F) -> JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        tokio::spawn(self.track(future))
    }
}

struct TrackedFuture<F> {
    future: Pin<Box<F>>,
    _live: LiveTask,
    abort: CancellationToken,
}
