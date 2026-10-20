//! Shared fixtures for the `storage_mirrored_tests` split.

use crate::error::{DbError, DbResult};
use crate::types::{KvOp, RecordKey, RecordStream, Store};
use async_trait::async_trait;
use bytes::Bytes;
use shamir_types::types::record_id::RecordId;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};

/// A representative classified key — a `MetaKey::BaseIndexIndexes`-shaped tag
/// (`"indexes"`), matching the exact `RecordId::system` encoding.
pub(crate) fn classified_key() -> RecordKey {
    RecordKey::from_slice(RecordId::system("indexes").as_bytes())
}

/// A representative unclassified key — an ordinary (non-system) 16-byte
/// key, shaped like a real row id (`RecordId::new()`, non-zero
/// timestamp prefix) so it does NOT accidentally match the system-record
/// shape.
pub(crate) fn unclassified_key() -> RecordKey {
    RecordKey::from_slice(RecordId::new().as_bytes())
}

/// A second classified key — `MetaKey::Tables`-shaped tag (`"_m.tbl"`),
/// distinct from [`classified_key`]'s `"indexes"` tag so two durable ops
/// in one batch touch two different keys.
pub(crate) fn classified_key_2() -> RecordKey {
    RecordKey::from_slice(RecordId::system("_m.tbl").as_bytes())
}

/// Test-only mirror wrapper whose `transact` can be configured to fail
/// atomically — when `fail_transact` is set, it returns `Err` WITHOUT
/// applying ANY ops to the inner store, simulating what a genuinely
/// atomic backend (like `FjallStore`'s `OwnedWriteBatch`) does on a
/// batch-commit failure. All other `Store` methods delegate to `inner`
/// unchanged.
///
/// Proves that `MirroredStore::transact` delegates the ENTIRE durable
/// subset to ONE `mirror.transact` call (not per-op): when the mirror's
/// transact fails, NO durable op is partially committed.
pub(crate) struct FailingTransactMirror {
    pub(crate) inner: Arc<dyn Store>,
    /// When `true`, `transact` returns `Err` without delegating to inner.
    pub(crate) fail_transact: AtomicBool,
}

#[async_trait]
impl Store for FailingTransactMirror {
    async fn insert(&self, value: Bytes) -> DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> DbResult<bool> {
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> DbResult<bool> {
        self.inner.remove(key).await
    }
    async fn transact(&self, ops: Vec<KvOp>) -> DbResult<()> {
        if self.fail_transact.load(Ordering::Acquire) {
            return Err(DbError::Internal(
                "injected transact failure (FailingTransactMirror)".into(),
            ));
        }
        self.inner.transact(ops).await
    }
    fn iter_stream(&self, batch_size: usize) -> RecordStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> RecordStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

// ---------------------------------------------------------------------------
// Test-only log capture for the hydration re-filter diagnostic assertion.
// ---------------------------------------------------------------------------

/// Process-global capture buffer for every record the test-only
/// [`CapturingLogger`] emits. nextest runs each test in its OWN process,
/// so this one-shot global is safe from cross-test interference.
static CAPTURED_LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static CAPTURING_LOGGER: CapturingLogger = CapturingLogger;

/// Minimal `log::Log` impl that records every emitted record's
/// `[LEVEL] args` into [`CAPTURED_LOGS`]. Lets the F-41 hydration test
/// ASSERT the diagnostic `warn!` actually fires, rather than only
/// asserting the behavioural consequence of the classify branch.
struct CapturingLogger;

impl log::Log for CapturingLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        if let Some(cell) = CAPTURED_LOGS.get() {
            cell.lock()
                .unwrap()
                .push(format!("[{}] {}", record.level(), record.args()));
        }
    }
    fn flush(&self) {}
}

/// Install the capturing logger (idempotent within a process) and return
/// a handle to the captured-messages cell. Callers `.lock().unwrap()`
/// `.clear()` before the code under test so only its emissions are kept.
pub(crate) fn install_capturing_logger() -> &'static Mutex<Vec<String>> {
    let cell = CAPTURED_LOGS.get_or_init(|| Mutex::new(Vec::new()));
    // `set_logger` is process-global and one-shot. nextest's
    // process-per-test means the first call in this test's own process
    // wins; a repeat call in the same process is a no-op we ignore.
    let _ = log::set_logger(&CAPTURING_LOGGER);
    log::set_max_level(log::LevelFilter::Warn);
    cell
}
