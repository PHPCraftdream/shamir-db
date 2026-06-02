//! Non-blocking logging initialisation.
//!
//! Routes all `tracing` output through a background writer so emitting a log
//! event never blocks a hot-path thread.
//!
//! Two modes, selected by `LoggingConfig::file`:
//!
//! * **stdout** (`file == None`, slice 1) — `tracing_appender::non_blocking`
//!   wrapping stdout. Lossy channel; lines dropped on overflow.
//! * **batched file** (`file == Some(path)`) — a bounded MPSC channel drained
//!   by ONE worker thread that accumulates lines in a `BufWriter<File>`.
//!   The worker flushes to disk every `flush_interval_ms` (timer tick) or
//!   when the in-memory buffer exceeds the size threshold. On guard drop a
//!   dedicated shutdown channel is closed; the worker drains remaining lines,
//!   does a final flush, and exits — no loss on shutdown.
//!
//! The durable audit chain (`audit_appender`) is separate and unaffected.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use tracing_subscriber::EnvFilter;

use crate::config::LoggingConfig;

/// In-memory buffer threshold that triggers an early flush. 256 KiB.
const BURST_FLUSH_THRESHOLD: usize = 256 * 1024;

// ---------------------------------------------------------------------------
// Unified guard
// ---------------------------------------------------------------------------

/// Guard that keeps the background logging infrastructure alive.
///
/// Dropping it flushes buffered output and joins the worker thread (if any).
/// The caller must keep it alive for the whole process.
pub enum LogGuard {
    /// Stdout non-blocking writer guard (slice 1).
    Stdout(tracing_appender::non_blocking::WorkerGuard),
    /// Batched file writer guard.
    File(BatchedFileGuard),
}

// ---------------------------------------------------------------------------
// Batched file writer internals
// ---------------------------------------------------------------------------

/// Handle that closes the shutdown channel and joins the worker on drop.
///
/// The guard owns the *shutdown sender* — a dedicated `mpsc::channel<()>`
/// pair. The worker selects on both the data channel and the shutdown
/// channel. When the guard is dropped, the shutdown sender is dropped,
/// the worker's `recv()` on the shutdown channel returns `Disconnected`,
/// the worker drains any remaining data, flushes, and exits.
pub struct BatchedFileGuard {
    shutdown_tx: Option<mpsc::Sender<()>>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Drop for BatchedFileGuard {
    fn drop(&mut self) {
        // Drop the shutdown sender → worker's shutdown rx sees Disconnected.
        drop(self.shutdown_tx.take());
        // Join the worker — it will drain remaining lines and flush.
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Cloneable sender that implements [`Write`]. Used as the `MakeWriter`
/// output. Each `write` call pushes bytes onto the bounded channel.
/// If the channel is full (hot-path pressure) the line is silently dropped.
#[derive(Clone)]
pub struct BatchedSender {
    tx: SyncSender<Vec<u8>>,
}

impl Write for BatchedSender {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let len = buf.len();
        if self.tx.try_send(buf.to_vec()).is_err() {
            // Channel full or closed — drop the line (non-blocking).
        }
        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Background worker that drains log lines from the channel and writes them
/// to a `BufWriter<File>`.
///
/// Uses a separate `shutdown_rx` channel for clean termination. The guard
/// owns the `shutdown_tx`; when it is dropped the worker drains + flushes.
fn worker_loop(
    rx: Receiver<Vec<u8>>,
    mut bw: BufWriter<File>,
    interval: Duration,
    shutdown_rx: Receiver<()>,
) {
    // Poll cadence is decoupled from the flush cadence: we wake at least
    // every ~100 ms (or sooner if the interval is shorter) so a shutdown
    // signal is noticed promptly, but only FLUSH once `interval` has
    // elapsed. Without this, a long `flush_interval_ms` would make graceful
    // shutdown wait a whole interval inside `recv_timeout` before the guard
    // could join — stalling the process stop.
    let poll = interval.min(Duration::from_millis(100));
    let mut last_flush = Instant::now();
    loop {
        match rx.recv_timeout(poll) {
            Ok(line) => {
                let _ = bw.write_all(&line);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Data channel fully closed — drain + flush + exit.
                drain_and_flush(&rx, &mut bw);
                break;
            }
        }

        // Flush on the burst threshold OR once the flush interval elapsed.
        if bw.buffer().len() >= BURST_FLUSH_THRESHOLD || last_flush.elapsed() >= interval {
            let _ = bw.flush();
            last_flush = Instant::now();
        }

        // The guard signals shutdown by DROPPING `shutdown_tx`, which
        // surfaces here as `Disconnected` (not `Ok`) — break on it too, or
        // (when the data sender lives on, e.g. held forever by the global
        // subscriber) the worker would loop forever and the guard's `join()`
        // on drop would hang the process at shutdown.
        match shutdown_rx.try_recv() {
            Ok(()) | Err(mpsc::TryRecvError::Disconnected) => {
                drain_and_flush(&rx, &mut bw);
                break;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }
}

/// Drain all remaining items from the data channel and flush.
fn drain_and_flush(rx: &Receiver<Vec<u8>>, bw: &mut BufWriter<File>) {
    for line in rx.try_iter() {
        let _ = bw.write_all(&line);
    }
    let _ = bw.flush();
}

/// Build a batched file writer pair for testing.
///
/// Returns a `(impl Write + Clone, BatchedFileGuard)` — write bytes to the
/// first element, keep the guard alive, and on drop everything is flushed.
pub fn batched_file_writer(
    path: &Path,
    flush_interval_ms: u64,
) -> Result<(BatchedSender, BatchedFileGuard), io::Error> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    // Capacity == the burst threshold so lines truly accumulate in memory
    // (batched) up to ~256 KiB before hitting the disk — not the default
    // 8 KiB, which would auto-flush constantly and defeat the batching.
    let bw = BufWriter::with_capacity(BURST_FLUSH_THRESHOLD, file);

    let (tx, rx) = mpsc::sync_channel::<Vec<u8>>(4096);
    let (shutdown_tx, shutdown_rx) = mpsc::channel::<()>();
    let interval = Duration::from_millis(flush_interval_ms);

    let handle = thread::Builder::new()
        .name("shamir-log-writer".into())
        .spawn(move || worker_loop(rx, bw, interval, shutdown_rx))?;

    let sender = BatchedSender { tx };
    let guard = BatchedFileGuard {
        shutdown_tx: Some(shutdown_tx),
        handle: Some(handle),
    };
    Ok((sender, guard))
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build an [`EnvFilter`] from a level/directive string (e.g. `"info"` or
/// `"info,shamir_engine=debug"`), falling back to `"info"` on a parse
/// error. Pure + testable.
pub fn env_filter(level: &str) -> EnvFilter {
    EnvFilter::try_new(level).unwrap_or_else(|_| EnvFilter::new("info"))
}

/// Initialise the global tracing subscriber.
///
/// * `cfg.file == None` → non-blocking stdout (slice 1).
/// * `cfg.file == Some(path)` → batched file appender with periodic flush.
///
/// Returns a [`LogGuard`] the caller must keep alive for the whole process.
pub fn init(cfg: &LoggingConfig) -> LogGuard {
    if let Some(ref path) = cfg.file {
        let p = Path::new(path);
        let (sender, guard) = match batched_file_writer(p, cfg.flush_interval_ms) {
            Ok(pair) => pair,
            Err(e) => {
                eprintln!(
                    "failed to open log file {}: {e}, falling back to stdout",
                    path
                );
                let (w, g) = tracing_appender::non_blocking(std::io::stdout());
                tracing_subscriber::fmt()
                    .with_env_filter(env_filter(&cfg.level))
                    .with_writer(w)
                    .init();
                return LogGuard::Stdout(g);
            }
        };
        tracing_subscriber::fmt()
            .with_env_filter(env_filter(&cfg.level))
            .with_writer(move || sender.clone())
            .init();
        LogGuard::File(guard)
    } else {
        let (writer, guard) = tracing_appender::non_blocking(std::io::stdout());
        tracing_subscriber::fmt()
            .with_env_filter(env_filter(&cfg.level))
            .with_writer(writer)
            .init();
        LogGuard::Stdout(guard)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_filter_single_level() {
        let _ = env_filter("warn");
    }

    #[test]
    fn env_filter_directive_string() {
        let _ = env_filter("info,shamir_engine=debug");
    }

    #[test]
    fn env_filter_garbage_falls_back_to_info() {
        let _ = env_filter("not_a_real_level!!!{}");
    }

    #[test]
    fn env_filter_empty_falls_back_to_info() {
        let _ = env_filter("");
    }
}
