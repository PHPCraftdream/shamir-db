use std::future::Future;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use futures_util::future::BoxFuture;

const RUNTIME_RUN_BOUND: Duration = Duration::from_secs(120);
const SHUTDOWN_BOUND: Duration = Duration::from_secs(150);

/// One current-thread Tokio runtime on a roomy (64 MiB stack) std thread.
pub struct TestRuntime {
    job_tx: Option<tokio::sync::mpsc::Sender<BoxFuture<'static, ()>>>,
    stopped_rx: Option<mpsc::Receiver<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl TestRuntime {
    /// Spawn the runtime thread.
    pub fn new() -> Self {
        let (job_tx, mut job_rx) = tokio::sync::mpsc::channel::<BoxFuture<'static, ()>>(64);
        let (stopped_tx, stopped_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("shell-test-runtime".into())
            .stack_size(64 * 1024 * 1024)
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("test runtime");
                runtime.block_on(async move {
                    while let Some(job) = job_rx.recv().await {
                        tokio::spawn(job);
                    }
                });
                drop(runtime);
                let _ = stopped_tx.send(());
            })
            .expect("spawn shell-test runtime thread");
        Self {
            job_tx: Some(job_tx),
            stopped_rx: Some(stopped_rx),
            thread: Some(thread),
        }
    }

    fn sender(&self) -> tokio::sync::mpsc::Sender<BoxFuture<'static, ()>> {
        self.job_tx
            .as_ref()
            .expect("TestRuntime sender retained until drop")
            .clone()
    }

    /// Drive `future` to completion on the runtime, blocking the caller.
    pub fn run<F>(&self, future: F) -> F::Output
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let (done_tx, done_rx) = mpsc::sync_channel::<F::Output>(1);
        match self.sender().try_send(Box::pin(async move {
            let value = future.await;
            let _ = done_tx.send(value);
        })) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                panic!("the shell-test runtime thread is gone")
            }
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                panic!("the shell-test runtime job queue is full")
            }
        }
        match done_rx.recv_timeout(RUNTIME_RUN_BOUND) {
            Ok(value) => value,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                panic!("shell-test runtime job exceeded {RUNTIME_RUN_BOUND:?}")
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => panic!(
                "the runtime job panicked (its own panic is the real failure); \
                 the job sender was dropped without a value"
            ),
        }
    }
}

impl Drop for TestRuntime {
    fn drop(&mut self) {
        drop(self.job_tx.take());
        if let Some(thread) = self.thread.take() {
            match self
                .stopped_rx
                .take()
                .expect("runtime stop receiver retained until drop")
                .recv_timeout(SHUTDOWN_BOUND)
            {
                Ok(()) => {
                    if let Err(panic) = thread.join() {
                        eprintln!("shell-test runtime worker panicked during shutdown: {panic:?}");
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    eprintln!(
                        "shell-test runtime shutdown exceeded {SHUTDOWN_BOUND:?}; \
                         recording teardown failure and detaching runtime thread"
                    );
                    drop(thread);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => match thread.join() {
                    Ok(()) => eprintln!("shell-test runtime exited without its stop notification"),
                    Err(panic) => eprintln!(
                        "shell-test runtime worker panicked before stop notification: {panic:?}"
                    ),
                },
            }
        }
    }
}
