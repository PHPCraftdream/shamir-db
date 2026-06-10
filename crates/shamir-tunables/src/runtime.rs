//! Instance-level runtime-overridable tunables.
//!
//! Reads are a single atomic load (instant, cached, lock-free, non-blocking).
//! Overrides are rare and just store a new atomic value, taking effect on the
//! next read. Initialized from the compiled [`instance_defaults`] consts —
//! so an untouched instance behaves exactly as the consts (§ simple-by-default).

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use crate::instance_defaults;

/// Instance-level runtime-overridable tunables. Reads are a single atomic load
/// (instant, cached, lock-free, non-blocking); overrides store a new value.
/// Initialized from the compiled `instance_defaults` consts — so an untouched
/// instance behaves exactly as the consts (§ simple-by-default).
#[derive(Debug)]
pub struct RuntimeTunables {
    io_frame_buffer_cap: AtomicUsize,
    server_poll_interval_ms: AtomicU64,
    conn_max_in_flight: AtomicUsize,
}

impl Default for RuntimeTunables {
    fn default() -> Self {
        Self {
            io_frame_buffer_cap: AtomicUsize::new(instance_defaults::IO_FRAME_BUFFER_CAP),
            server_poll_interval_ms: AtomicU64::new(
                instance_defaults::SERVER_POLL_INTERVAL.as_millis() as u64,
            ),
            conn_max_in_flight: AtomicUsize::new(instance_defaults::CONN_MAX_IN_FLIGHT),
        }
    }
}

impl RuntimeTunables {
    /// Create a new `RuntimeTunables` initialized from the compiled
    /// `instance_defaults` consts.
    pub fn new() -> Self {
        Self::default()
    }

    /// Zero-overhead read: single relaxed atomic load.
    #[inline]
    pub fn io_frame_buffer_cap(&self) -> usize {
        self.io_frame_buffer_cap.load(Ordering::Relaxed)
    }

    /// Zero-overhead read: single relaxed atomic load.
    #[inline]
    pub fn server_poll_interval(&self) -> Duration {
        Duration::from_millis(self.server_poll_interval_ms.load(Ordering::Relaxed))
    }

    /// Override (rare): store a new value (takes effect on the next read).
    pub fn set_io_frame_buffer_cap(&self, v: usize) {
        self.io_frame_buffer_cap.store(v, Ordering::Relaxed);
    }

    /// Override (rare): store a new value (takes effect on the next read).
    pub fn set_server_poll_interval(&self, v: Duration) {
        self.server_poll_interval_ms
            .store(v.as_millis() as u64, Ordering::Relaxed);
    }

    /// Zero-overhead read: single relaxed atomic load.
    #[inline]
    pub fn conn_max_in_flight(&self) -> usize {
        self.conn_max_in_flight.load(Ordering::Relaxed)
    }

    /// Override (rare): store a new value (takes effect on the next read).
    pub fn set_conn_max_in_flight(&self, v: usize) {
        self.conn_max_in_flight.store(v, Ordering::Relaxed);
    }
}
