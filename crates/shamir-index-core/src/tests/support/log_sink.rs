use std::sync::{Arc, Mutex, Once, OnceLock};
use std::thread::{self, ThreadId};

/// Captures `Warn`-and-above records together with the emitting thread, so a
/// test only ever sees the lines its own thread produced (tests run on a
/// current-thread runtime; the captured warn sites never hop threads).
pub struct TestLogSink {
    logs: Arc<Mutex<Vec<(ThreadId, String)>>>,
}

impl TestLogSink {
    fn new() -> Self {
        Self {
            logs: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Drops the calling thread's captured lines.
    pub fn clear(&self) {
        let me = thread::current().id();
        self.logs.lock().unwrap().retain(|(id, _)| *id != me);
    }

    /// True when the calling thread emitted a line containing `needle`.
    pub fn contains(&self, needle: &str) -> bool {
        let me = thread::current().id();
        self.logs
            .lock()
            .unwrap()
            .iter()
            .any(|(id, line)| *id == me && line.contains(needle))
    }
}

impl log::Log for TestLogSink {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            self.logs
                .lock()
                .unwrap()
                .push((thread::current().id(), format!("{}", record.args())));
        }
    }

    fn flush(&self) {}
}

static TEST_LOG_SINK: OnceLock<TestLogSink> = OnceLock::new();
static CAPTURE_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
static INSTALL_LOGGER: Once = Once::new();

/// Installs the shared sink as the process logger exactly once; a failure
/// means another logger was installed first and every capture test would be
/// vacuous, so it aborts loudly instead of being ignored.
fn install_sink() -> &'static TestLogSink {
    let sink = TEST_LOG_SINK.get_or_init(TestLogSink::new);
    INSTALL_LOGGER.call_once(|| {
        log::set_logger(sink).expect(
            "shared log sink must be the installed logger — a pre-installed logger would make capture tests vacuous",
        );
    });
    log::set_max_level(log::LevelFilter::Warn);
    sink
}

/// Begin an exclusive log-capture session: the returned guard must be held
/// for the whole capture interval (acquire, clear, run, assert, drop). For
/// async tests; the tokio guard is safe to hold across `.await` points.
pub async fn capture_session() -> (tokio::sync::MutexGuard<'static, ()>, &'static TestLogSink) {
    let guard = CAPTURE_LOCK.lock().await;
    (guard, install_sink())
}

/// Synchronous variant of [`capture_session`] for plain `#[test]` fns where
/// `.await` is impossible. Panics if called from an async runtime context.
pub fn capture_session_blocking() -> (tokio::sync::MutexGuard<'static, ()>, &'static TestLogSink) {
    let guard = CAPTURE_LOCK.blocking_lock();
    (guard, install_sink())
}
