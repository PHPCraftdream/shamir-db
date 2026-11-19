//! Sequential four-writer ack oracle for the online-copy gate test
//! (brief 50-r2a-4, items 1–2).
//!
//! Each writer runs a strictly sequential loop: write k+1 is issued only
//! after write k has been acknowledged. A write enters the in-memory
//! [`Ledger`] (the oracle) ONLY after its successful acknowledgement, and
//! the acknowledgement is time-stamped with [`Instant`] at the moment the
//! successful alias response is received — this is what makes the
//! "acknowledged before StartBackup" lower bound (design §6.1 item (a))
//! and the writer-gap bound (§6.2, R3) decidable.
//!
//! Stop/control contract: the runner observes a `tokio::sync::watch`
//! stop flag, checked after every awaited request (never across a guard:
//! no lock is held over any `.await`). Every awaited request goes through
//! the fixture's [`paced_execute`], so it is bounded by the fixture's
//! [`REQUEST_TIMEOUT`] (120 s — deliberately not the R3 measurement bound)
//! and paced by [`WRITER_PACE`] (≥ 4 ms) before every attempt; there are
//! no fixed sleeps beyond that pacing.
//!
//! Termination: a writer that reaches [`WRITER_ROW_LIMIT`] rows without
//! having been stopped returns [`WriterError::Exhausted`]; the gate test
//! asserts exhaustion-with-stop rather than letting writers finish early
//! before `BackupStatus` reports `Done` (non-vacuity).

use std::sync::{Arc, Mutex};
use std::time::Instant;

use shamir_client::builder::batch::{Batch, Durability};
use shamir_client::builder::write;
use shamir_client::Client;
use tokio::sync::watch;

use super::fixture::{paced_execute, DB, REPO, REQUEST_TIMEOUT, TABLE};

/// Column holding the writer id.
pub const COL_WRITER: &str = "writer";
/// Column holding the per-writer sequence number.
pub const COL_SEQ: &str = "seq";
/// Column holding the derived value (`value(writer, seq)`); indexed.
pub const COL_VALUE: &str = "value";

/// Number of writer tasks in the gate test.
pub const WRITER_COUNT: usize = 4;

/// Hard per-writer row limit. 1600 small rows per writer (4 × 1600 = 6400
/// rows total) fits the §6.4 data budget and stays well below the 180 s
/// nextest per-test kill; reaching it without a stop signal is an error,
/// not a success.
pub const WRITER_ROW_LIMIT: u64 = 1600;

/// Pacing applied before EVERY attempt of a writer request (including
/// retries of a `rate_limited` rejection): ≥ 4 ms, so a burst of writers
/// never re-fires the pre-handler limiter it just got rejected by.
pub const WRITER_PACE: std::time::Duration = std::time::Duration::from_millis(4);

/// Value derivation shared by writers and the read-back oracle: writer id
/// times a fixed stride plus the sequence number, so a torn or misplaced
/// record is detectable from `(writer, seq)` alone.
///
/// Stride 1_000_000 comfortably exceeds [`WRITER_ROW_LIMIT`], so the map
/// `(writer, seq) ↦ value` is injective over the whole legal domain.
pub const VALUE_STRIDE: u64 = 1_000_000;

/// The derived value for a `(writer, seq)` pair. Exported so the restore
/// oracle recomputes expectations without duplicating the arithmetic.
///
/// `u64 → QueryValue` is a verified conversion (`From<u64> for
/// Value<String>` promotes to `Value::Int` for values below `i64::MAX`),
/// so the doc fields stay plain integers on the wire.
#[must_use]
pub fn value(writer: u64, seq: u64) -> u64 {
    writer * VALUE_STRIDE + seq
}

/// One acknowledged write, as recorded by the oracle.
#[derive(Debug, Clone)]
pub struct Ack {
    /// Writer that issued and got the ack for this write.
    pub writer: u64,
    /// Sequence number of the write (1-based, contiguous per writer).
    pub seq: u64,
    /// Stored value, `value(writer, seq)`.
    pub value: u64,
    /// Monotonic timestamp taken immediately AFTER the successful
    /// response was received (a real ACK receipt, never a send stamp).
    pub acked_at: Instant,
}

/// Shared in-memory oracle ledger: append-only, one entry per ack.
#[derive(Debug, Default, Clone)]
pub struct Ledger {
    inner: Arc<Mutex<Vec<Ack>>>,
}

impl Ledger {
    /// A fresh empty ledger.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one acknowledged write. Called only after a successful
    /// `execute`; the timestamp is taken by the caller at response time
    /// and passed in, so the ledger never stamps on its own schedule.
    fn record(&self, ack: Ack) {
        self.inner.lock().expect("ledger mutex poisoned").push(ack);
    }

    /// Snapshot of all acknowledgements so far, ordered per writer by
    /// sequence (each writer appends strictly sequentially).
    #[must_use]
    pub fn snapshot(&self) -> Vec<Ack> {
        self.inner.lock().expect("ledger mutex poisoned").clone()
    }
}

/// Why a writer stopped.
#[derive(Debug)]
pub enum WriterError {
    /// A request failed or timed out; the writer stops immediately and
    /// the error is surfaced (never swallowed).
    Request {
        /// Which sequence was in flight.
        seq: u64,
        /// The underlying failure, formatted for diagnostics.
        source: String,
    },
    /// [`WRITER_ROW_LIMIT`] reached without a stop signal: the gate test
    /// treats this as a failure (writers must be stopped, not run dry,
    /// before `Done` — otherwise the copy could pass vacuously).
    Exhausted {
        /// Limit that was hit.
        limit: u64,
    },
    /// The run was stopped mid-loop by the control channel.
    Stopped {
        /// Next sequence that would have been issued.
        next_seq: u64,
    },
}

impl std::fmt::Display for WriterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WriterError::Request { seq, source } => {
                write!(f, "writer request failed at seq {seq}: {source}")
            }
            WriterError::Exhausted { limit } => {
                write!(
                    f,
                    "writer hit its hard row limit {limit} without a stop signal"
                )
            }
            WriterError::Stopped { next_seq } => {
                write!(f, "writer stopped before seq {next_seq}")
            }
        }
    }
}

impl std::error::Error for WriterError {}

/// Control handle for the writer fleet: flip `stop` to `true` once the
/// backup has reached `Done` (or on failure). Writers poll the flag
/// after every awaited request — no lock is held across any `.await`.
#[derive(Clone)]
pub struct WriterControl {
    tx: watch::Sender<bool>,
}

impl WriterControl {
    /// A control channel starting un-stopped.
    #[must_use]
    pub fn new() -> Self {
        let (tx, _) = watch::channel(false);
        Self { tx }
    }

    /// A receiver for one writer task.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.tx.subscribe()
    }

    /// Signal all writers to stop after their current request settles.
    pub fn stop(&self) {
        let _ = self.tx.send(true);
    }
}

/// Run one writer's sequential loop to exhaustion or stop.
///
/// `client` is this writer's OWN connection (taken by value); `rx` is the
/// writer's stop watcher; `ledger` is the shared ack oracle.
///
/// Loop shape (strictly sequential, design §6.1):
/// 1. check stop → return [`WriterError::Stopped`] if set;
/// 2. build ONE typed insert op for the next sequence;
/// 3. await the bounded `execute(DB, batch.build())`;
/// 4. verify a TYPED acknowledgement (non-skipped result AND an explicit
///    committed `commit_version` — the batch is transactional); only
///    after that verified committed receipt, stamp `Instant::now()` as
///    the real ACK receipt time and append to the ledger;
/// 5. repeat.
///
/// The `Instant` returned by [`paced_execute`] is a SEND stamp (taken
/// before the request goes out) and is deliberately DISCARDED here: it
/// is the ordering anchor for the server-side status/StartBackup
/// comparisons in the gate test, but it is never an ack time.
///
/// Every insert goes through `Insert::with_repo(REPO, TABLE)` under
/// `Durability::Synced` in an explicitly transactional batch — the ack
/// below is verified against the actual commit receipt, so "acked"
/// means "flush durable backing before ack".
pub async fn run_writer(
    writer: u64,
    client: Client,
    mut rx: watch::Receiver<bool>,
    ledger: &Ledger,
) -> Result<(), WriterError> {
    debug_assert!(
        (1..=WRITER_COUNT as u64).contains(&writer),
        "writer id out of 1..=4"
    );
    let mut seq: u64 = 1;
    loop {
        // Stop is checked after every awaited request (and up front), so
        // a stop signal lands within one request of being sent.
        if *rx.borrow_and_update() {
            bounded_close(client).await;
            return Err(WriterError::Stopped { next_seq: seq });
        }
        if seq > WRITER_ROW_LIMIT {
            bounded_close(client).await;
            return Err(WriterError::Exhausted {
                limit: WRITER_ROW_LIMIT,
            });
        }

        let doc = write::doc()
            .set_value(COL_WRITER, writer)
            .set_value(COL_SEQ, seq)
            .set_value(COL_VALUE, value(writer, seq))
            .build();

        // Unique request id per (writer, seq): the response echoes it,
        // so late/aliased responses can never be attributed to the wrong
        // write. `writer` is bounded by `WRITER_COUNT` and `seq` is
        // contiguous per writer, so ids cannot collide.
        let mut batch = Batch::new();
        batch.id(format!("writer-{writer}-seq-{seq}"));
        batch.transactional();
        batch.durability(Durability::Synced);
        batch.insert(TABLE, write::Insert::with_repo(REPO, TABLE).row(doc));

        // Paced, retry-once-per-`rate_limited` execution bounded by the
        // fixture's 120 s request deadline. The returned `Instant` is the
        // SEND stamp (captured after pacing, immediately before the
        // successful wire attempt) — the ordering anchor for the server
        // side, NOT an ack time, so it is discarded here.
        let (_send_at, result) = paced_execute(&client, DB, batch.build(), WRITER_PACE).await;
        let response = match result {
            Ok(response) => response,
            Err(e) => {
                bounded_close(client).await;
                return Err(WriterError::Request {
                    seq,
                    source: e.to_string(),
                });
            }
        };

        // Ack = a TYPED acknowledgement for OUR alias: the alias must be
        // present, NOT cascade-skipped, AND the (explicitly transactional)
        // batch must report a committed `commit_version`. An "Ok" response
        // without that receipt is NOT an ack — the write may not have
        // happened.
        let result = response.results.get(TABLE);
        if result.is_none_or(|r| r.skipped) {
            bounded_close(client).await;
            let source = if result.is_some() {
                format!("'{TABLE}' alias was cascade-skipped")
            } else {
                format!("response missing the '{TABLE}' alias")
            };
            return Err(WriterError::Request { seq, source });
        }
        let commit_version = match response.transaction {
            Some(tx) if tx.is_committed() => tx.commit_version,
            other => {
                bounded_close(client).await;
                return Err(WriterError::Request {
                    seq,
                    source: format!(
                        "batch not committed: transaction info {other:?} without a commit version"
                    ),
                });
            }
        };
        if commit_version.is_none() {
            bounded_close(client).await;
            return Err(WriterError::Request {
                seq,
                source: "committed transaction carries no commit_version".to_string(),
            });
        }

        // ACK time = `Instant::now()` taken immediately AFTER the typed
        // non-skipped committed receipt above was verified — a real
        // response-receipt timestamp (the write is durably committed when
        // this stamp is taken), never the pre-send stamp of
        // `paced_execute`.
        ledger.record(Ack {
            writer,
            seq,
            value: value(writer, seq),
            acked_at: Instant::now(),
        });
        seq += 1;
    }
}

/// Close a writer client without letting the close itself hang the gate:
/// bounded by [`REQUEST_TIMEOUT`], best-effort (the server shutdown in the
/// fixture validates the real close report). Consumes the client, so it
/// is called exactly once per return path.
async fn bounded_close(client: Client) {
    let _ = tokio::time::timeout(REQUEST_TIMEOUT, client.close()).await;
}
