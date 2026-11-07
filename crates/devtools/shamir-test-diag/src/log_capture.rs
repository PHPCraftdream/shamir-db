//! Installs the process-wide `log::Log` sink that feeds the shared ring, on
//! the first guard creation; degrades gracefully when another logger is
//! already installed (design requirement: the report header says so).

use crate::ring::shared_ring;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Once, OnceLock};

static INSTALL: Once = Once::new();
static RING_LOGGER: OnceLock<RingLogger> = OnceLock::new();
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Whether the process-wide `log` ring capture is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogCaptureStatus {
    /// This crate's ring logger owns the process logger.
    Installed,
    /// Another logger was already installed (`log::set_logger` refused);
    /// the guard degrades to timeline and notes only.
    Degraded,
}

/// Installs the ring logger exactly once per process and returns the status.
/// Repeated calls report the same status without re-attempting.
pub fn install_log_ring() -> LogCaptureStatus {
    INSTALL.call_once(|| {
        let logger = RING_LOGGER.get_or_init(RingLogger::new);
        // `set_logger` fails only when another logger already owns the
        // process logger — that is the designed graceful degradation, not an
        // error, and the existing logger is left untouched.
        if log::set_logger(logger).is_ok() {
            // Capture contract (design): every level up to `Debug`.
            log::set_max_level(log::LevelFilter::Debug);
            INSTALLED.store(true, Ordering::Relaxed);
        }
    });
    if INSTALLED.load(Ordering::Relaxed) {
        LogCaptureStatus::Installed
    } else {
        LogCaptureStatus::Degraded
    }
}

struct RingLogger;

impl RingLogger {
    fn new() -> Self {
        Self
    }
}

impl log::Log for RingLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Debug
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        shared_ring().push(
            record.level().to_string(),
            record.target().to_string(),
            format!("{}", record.args()),
        );
    }

    fn flush(&self) {}
}
