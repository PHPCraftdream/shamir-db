//! Non-blocking logging initialisation with runtime-adjustable namespace masks.
//!
//! Routes all `tracing` output through a background writer so emitting a log
//! event never blocks a hot-path thread. A lock-free [`LogMask`] (stored in a
//! process-global `ArcSwap`) allows per-namespace level changes to take effect
//! live — no restart required.
//!
//! # Two modes (selected by `LoggingConfig::file`)
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
//! # Lock-free namespace masks (slice 3)
//!
//! The [`ns`] module defines curated log-target constants. Log sites SHOULD
//! use `tracing::info!(target: ns::WAL, …)` to opt into per-namespace
//! filtering. The mask works for **any** target string — module-path targets
//! are also matched.
//!
//! The hot-path decision (`enabled()`) is a single `ArcSwap::load` (one
//! atomic read) + longest-prefix lookup in a small override table. No
//! `std::sync::Mutex`, `RwLock`, or `tracing_subscriber::reload` (which
//! internally uses an `RwLock`).
//!
//! Runtime API: [`set_mask`], [`set_namespace_level`], [`current_mask`].

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use once_cell::sync::Lazy;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::layer::Filter;
use tracing_subscriber::prelude::*;
use tracing_subscriber::{fmt, registry, Layer};

use crate::config::LoggingConfig;

/// In-memory buffer threshold that triggers an early flush. 256 KiB.
const BURST_FLUSH_THRESHOLD: usize = 256 * 1024;

// ---------------------------------------------------------------------------
// Namespace taxonomy
// ---------------------------------------------------------------------------

/// Curated log-target constants for per-namespace filtering.
///
/// Log sites SHOULD use `tracing::info!(target: ns::WAL, …)` to participate
/// in namespace-aware filtering. Module-path targets (`shamir_engine::tx`)
/// are also matched — the mask works for **any** target string.
///
/// A wire admin-op / SIGHUP trigger to call [`set_namespace_level`] live is
/// a noted follow-up, not this slice.
pub mod ns {
    /// Access control / permissions enforcement (Shomer).
    pub const SHOMER: &str = "shomer";
    /// Write-ahead log operations.
    pub const WAL: &str = "wal";
    /// Transaction / commit pipeline.
    pub const TX: &str = "tx";
    /// KV storage backends.
    pub const STORAGE: &str = "storage";
    /// Table manager / general engine.
    pub const ENGINE: &str = "engine";
    /// Query planner / batch executor.
    pub const QUERY: &str = "query";
    /// HNSW / brute-force vector index.
    pub const VECTOR: &str = "vector";
    /// Full-text search.
    pub const FTS: &str = "fts";
    /// WASM function engine.
    pub const FUNC: &str = "fn";
    /// SCRAM / sessions / RBAC.
    pub const AUTH: &str = "auth";
    /// Connect / protocol / transports.
    pub const WIRE: &str = "wire";
    /// Server lifecycle / launcher.
    pub const SERVER: &str = "server";
    /// Online schema migration.
    pub const MIGRATION: &str = "migration";
}

// ---------------------------------------------------------------------------
// LogMask — pure, testable decision type
// ---------------------------------------------------------------------------

/// Immutable, cloneable filter mask: a default level plus per-target overrides.
///
/// The mask is stored inside an `ArcSwap<LogMask>` so swapping a new mask is
/// lock-free (RCU). [`LogMask::allows`] is the pure decision function tested
/// without any tracing infrastructure.
#[derive(Debug, Clone)]
pub struct LogMask {
    /// Level applied when no override matches.
    default: LevelFilter,
    /// `(target_prefix, level)` pairs. Longest-prefix match wins.
    /// Kept sorted by prefix length descending for efficient scan.
    overrides: Vec<(String, LevelFilter)>,
}

impl LogMask {
    /// Create a mask with only a default level (no overrides).
    pub fn new(default: LevelFilter) -> Self {
        Self {
            default,
            overrides: Vec::new(),
        }
    }

    /// Add (or replace) an override for `target`. Builder-style.
    pub fn with_override(mut self, target: &str, level: LevelFilter) -> Self {
        if let Some(pos) = self.overrides.iter().position(|(t, _)| t == target) {
            self.overrides[pos].1 = level;
        } else {
            self.overrides.push((target.to_owned(), level));
        }
        // Keep longest-prefix first so the scan short-circuits.
        self.overrides.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        self
    }

    /// Pure decision: would an event at `level` on `target` be emitted?
    ///
    /// Longest-prefix (or exact) override wins; otherwise the `default` applies.
    /// This is the core logic unit-tested without tracing `Metadata`.
    pub fn allows(&self, target: &str, level: &tracing::Level) -> bool {
        let event_level = LevelFilter::from_level(*level);
        let effective = self
            .overrides
            .iter()
            .find(|(prefix, _)| target.starts_with(prefix.as_str()))
            .map(|(_, lf)| *lf)
            .unwrap_or(self.default);
        event_level <= effective
    }
}

// ---------------------------------------------------------------------------
// Lock-free runtime mask handle
// ---------------------------------------------------------------------------

/// Process-global mask. Reads on the log path are one `ArcSwap::load` (single
/// atomic load, no lock). Writes (`set_mask` / `set_namespace_level`) do a
/// lock-free RCU `store`.
static GLOBAL_MASK: Lazy<ArcSwap<LogMask>> =
    Lazy::new(|| ArcSwap::from_pointee(LogMask::new(LevelFilter::INFO)));

/// Replace the entire runtime mask. Lock-free RCU swap.
pub fn set_mask(mask: LogMask) {
    GLOBAL_MASK.store(Arc::new(mask));
}

/// Change the level for a single namespace/target at runtime.
///
/// Loads the current mask, clones it, applies the override, and stores it
/// back — all without any `Mutex`/`RwLock` on the read path.
pub fn set_namespace_level(target: &str, level: LevelFilter) {
    let current = GLOBAL_MASK.load();
    let updated = Arc::unwrap_or_clone(Arc::clone(&current)).with_override(target, level);
    GLOBAL_MASK.store(Arc::new(updated));
}

/// Obtain a snapshot of the current mask. Lock-free single atomic load.
pub fn current_mask() -> Arc<LogMask> {
    GLOBAL_MASK.load_full()
}

// ---------------------------------------------------------------------------
// MaskFilter — tracing-subscriber Layer filter
// ---------------------------------------------------------------------------

/// A [`Filter`] implementation that reads [`GLOBAL_MASK`] on every `enabled()`
/// call. One `ArcSwap::load` + longest-prefix lookup — no locks.
#[derive(Debug, Clone)]
struct MaskFilter;

impl<S> Filter<S> for MaskFilter {
    fn enabled(
        &self,
        meta: &tracing::Metadata<'_>,
        _cx: &tracing_subscriber::layer::Context<'_, S>,
    ) -> bool {
        let mask = GLOBAL_MASK.load();
        mask.allows(meta.target(), meta.level())
    }
}

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
pub fn env_filter(level: &str) -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_new(level)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"))
}

/// Parse a level string (e.g. `"info"`) into a `LevelFilter`, defaulting to
/// `INFO` on failure.
fn parse_level_filter(level: &str) -> LevelFilter {
    level.parse::<LevelFilter>().unwrap_or(LevelFilter::INFO)
}

/// Initialise the global tracing subscriber.
///
/// * `cfg.file == None` → non-blocking stdout (slice 1).
/// * `cfg.file == Some(path)` → batched file appender with periodic flush.
///
/// The boot default for the runtime mask comes from `cfg.level`. The mask
/// can be changed at runtime via [`set_mask`] / [`set_namespace_level`].
///
/// Returns a [`LogGuard`] the caller must keep alive for the whole process.
pub fn init(cfg: &LoggingConfig) -> LogGuard {
    // Set the boot mask from config level.
    let boot_mask = LogMask::new(parse_level_filter(&cfg.level));
    set_mask(boot_mask);

    let mask_filter = MaskFilter;

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
                let fmt_layer = fmt::layer().with_writer(w);
                registry().with(fmt_layer.with_filter(mask_filter)).init();
                return LogGuard::Stdout(g);
            }
        };
        let fmt_layer = fmt::layer().with_writer(move || sender.clone());
        registry().with(fmt_layer.with_filter(mask_filter)).init();
        LogGuard::File(guard)
    } else {
        let (writer, guard) = tracing_appender::non_blocking(std::io::stdout());
        let fmt_layer = fmt::layer().with_writer(writer);
        registry().with(fmt_layer.with_filter(mask_filter)).init();
        LogGuard::Stdout(guard)
    }
}

#[cfg(test)]
mod tests;
