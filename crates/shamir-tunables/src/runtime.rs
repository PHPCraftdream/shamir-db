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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::instance_defaults;

    #[test]
    fn defaults_equal_consts() {
        let rt = RuntimeTunables::new();
        assert_eq!(
            rt.io_frame_buffer_cap(),
            instance_defaults::IO_FRAME_BUFFER_CAP,
            "io_frame_buffer_cap default must match instance_defaults const"
        );
        assert_eq!(
            rt.server_poll_interval(),
            instance_defaults::SERVER_POLL_INTERVAL,
            "server_poll_interval default must match instance_defaults const"
        );
        assert_eq!(
            rt.conn_max_in_flight(),
            instance_defaults::CONN_MAX_IN_FLIGHT,
            "conn_max_in_flight default must match instance_defaults const"
        );
    }

    #[test]
    fn set_io_frame_buffer_cap_then_read() {
        let rt = RuntimeTunables::new();
        rt.set_io_frame_buffer_cap(8192);
        assert_eq!(rt.io_frame_buffer_cap(), 8192);
    }

    #[test]
    fn set_server_poll_interval_then_read() {
        let rt = RuntimeTunables::new();
        let new_interval = Duration::from_millis(100);
        rt.set_server_poll_interval(new_interval);
        assert_eq!(rt.server_poll_interval(), new_interval);
    }

    #[test]
    fn set_conn_max_in_flight_then_read() {
        let rt = RuntimeTunables::new();
        rt.set_conn_max_in_flight(8);
        assert_eq!(rt.conn_max_in_flight(), 8);
    }

    /// Reads take `&self` — the struct is shareable via `Arc<RuntimeTunables>`.
    /// This test proves the API is callable on a shared reference.
    #[test]
    fn reads_are_shared_ref() {
        use std::sync::Arc;
        let rt = Arc::new(RuntimeTunables::new());
        let rt2 = rt.clone();
        // Both reads and writes work through a shared Arc<>.
        rt2.set_io_frame_buffer_cap(1234);
        assert_eq!(rt.io_frame_buffer_cap(), 1234);
    }
}
