//! ROUND 4 FIFO-owner ordering tests (RV-STORAGE-e70a0ddc).
//!
//! Every test here targets the single FIFO owner task in
//! `storage_cached.rs`. All parking is deterministic one-shot oneshot
//! gates (`entered` fires when the mock parked; the test holds the
//! `release` half); blocked-ness is proven with `poll!`, never with
//! spawned tasks and counter races. `HANG` timeouts are ONLY hang
//! detectors. Independent revert that fails exactly this file:
//! reintroduce any caller-side cache/backing mutation (per-key locks,
//! op-zone admission, or a caller that touches the mirror before the
//! owner's backing await completes) — the `poll!` blocking proofs and
//! FIFO log order below then break.
//!
//! Card revert, precise: removing FIFO ownership makes queued B/C execute
//! before the gate fires, and an old caller-owned reload whose caller is
//! cancelled never finishes (so a cancelled-caller test hangs instead of
//! completing). T1/T2 additionally seed an obsolete key into the REAL
//! backing before the constructor (direct `inner`, no gate) and remove it
//! inside queued transaction C; the final full equality proves the
//! removal propagated to both sides, plus the exact cache size.

use crate::storage_cached::CachedStore;
use crate::storage_in_memory::InMemoryStore;
use crate::types::{KvOp, RecordKey, Store};
use async_stream::stream;
use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use std::pin::{pin, Pin};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use tokio::sync::{oneshot, watch};

/// 10 s outer bound: ONLY a hang detector, never part of the protocol.
const HANG: std::time::Duration = std::time::Duration::from_secs(10);

fn rk(b: &[u8]) -> RecordKey {
    RecordKey::from_slice(b)
}

/// Deterministic wait until the mock's entered counter reaches `n`.
async fn entered_reaches(rx: &mut watch::Receiver<usize>, n: usize) {
    while *rx.borrow() < n {
        rx.changed().await.expect("mock entered watch closed");
    }
}

/// Await a gate's `entered` oneshot (HANG is only a hang detector).
async fn gate_entered(rx: oneshot::Receiver<()>) {
    tokio::time::timeout(HANG, rx)
        .await
        .expect("mock never entered the gated op")
        .expect("entered oneshot dropped");
}

/// One-use parking gate: the mock takes the stored `(entered_tx,
/// release_rx)` pair, signals `entered`, and parks until the test sends
/// `release`. Later takers find the slot empty and never park.
type Gate = Arc<Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>>;

fn new_gate() -> (Gate, oneshot::Receiver<()>, oneshot::Sender<()>) {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    (
        Arc::new(Mutex::new(Some((entered_tx, release_rx)))),
        entered_rx,
        release_tx,
    )
}

/// Take the gate (if still present), signal `entered`, park on `release`.
/// Send-safe: the `MutexGuard` is dropped before the `.await`.
async fn park_on_gate(gate: &Gate) {
    let taken = gate.lock().unwrap().take();
    if let Some((entered_tx, release_rx)) = taken {
        let _ = entered_tx.send(());
        let _ = release_rx.await;
    }
}

type BatchStream = Pin<
    Box<
        dyn futures::stream::Stream<Item = Result<Vec<(RecordKey, Bytes)>, crate::error::DbError>>
            + Send,
    >,
>;

/// Full-content oracle: drain an entire scan stream and sort by key.
async fn collect_sorted(s: &mut BatchStream) -> Vec<(RecordKey, Bytes)> {
    let mut out = Vec::new();
    while let Some(batch) = s.next().await {
        out.extend(batch.expect("scan must not fail"));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

// ============================================================================
// T1 — Sync mode: same-key A gated at the backing store; B and a transact
// C must not execute until A's backing await completes (strict FIFO).
// ============================================================================

/// First `set` applies to `inner`, logs, signals `entered`, THEN parks on
/// the oneshot gate — so the caller's ack is undeliverable while parked.
/// `transact` performs the REAL backing mutation and logs its `KvOp::Set`
/// values in order, so `applied` is the full FIFO mutation log. Errors
/// from `inner` are propagated, never ignored.
struct SyncGateStore {
    inner: Arc<InMemoryStore>,
    entered: watch::Sender<usize>,
    gate: Gate,
    applied: Mutex<Vec<Bytes>>,
}

impl SyncGateStore {
    async fn enter(&self) -> usize {
        let mut n = 0usize;
        self.entered.send_modify(|e| {
            *e += 1;
            n = *e;
        });
        n
    }
}

#[async_trait]
impl Store for SyncGateStore {
    async fn insert(&self, value: Bytes) -> crate::error::DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> crate::error::DbResult<bool> {
        let n = self.enter().await;
        self.applied.lock().unwrap().push(value.clone());
        let created = self.inner.set(key, value).await?;
        if n == 1 {
            park_on_gate(&self.gate).await;
        }
        Ok(created)
    }
    async fn get(&self, key: RecordKey) -> crate::error::DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> crate::error::DbResult<bool> {
        let n = self.enter().await;
        let r = self.inner.remove(key).await;
        if n == 1 {
            park_on_gate(&self.gate).await;
        }
        r
    }
    async fn transact(&self, ops: Vec<KvOp>) -> crate::error::DbResult<()> {
        let n = self.enter().await;
        let set_vals: Vec<Bytes> = ops
            .iter()
            .filter_map(|op| match op {
                KvOp::Set(_, v) => Some(v.clone()),
                KvOp::Remove(_) => None,
            })
            .collect();
        self.inner.transact(ops).await?;
        self.applied.lock().unwrap().extend(set_vals);
        if n == 1 {
            park_on_gate(&self.gate).await;
        }
        Ok(())
    }
    fn iter_stream(&self, batch_size: usize) -> BatchStream {
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> BatchStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

#[tokio::test]
async fn rv_storage_e70a0ddc_sync_same_key_gate_blocks_set_and_transact() {
    // Obsolete key seeded into the REAL backing before the constructor
    // (direct inner access — no gate involved).
    let obsolete = rk(b"obsolete");
    let inner = Arc::new(InMemoryStore::new());
    inner
        .set(obsolete.clone(), Bytes::from_static(b"stale"))
        .await
        .unwrap();

    let (gate, entered_rx, release_tx) = new_gate();
    let mock = Arc::new(SyncGateStore {
        inner,
        entered: watch::channel(0).0,
        gate,
        applied: Mutex::new(Vec::new()),
    });
    let store = Arc::new(
        CachedStore::new_sync(mock.clone() as Arc<dyn Store>)
            .await
            .unwrap(),
    );
    let k = rk(b"k");

    // A: the owner enters the mock (applies A, parks at the gate). A's
    // caller is still blocked awaiting its FIFO reply.
    let mut a = pin!(store.set(k.clone(), Bytes::from_static(b"A")));
    assert!(matches!(futures::poll!(a.as_mut()), Poll::Pending));
    let mut entered = mock.entered.subscribe();
    entered_reaches(&mut entered, 1).await;
    gate_entered(entered_rx).await; // the oneshot fired for A, too
    assert!(matches!(futures::poll!(a.as_mut()), Poll::Pending));

    // B: queued behind A — must NOT enter the backing store.
    let mut b = pin!(store.set(k.clone(), Bytes::from_static(b"B")));
    assert!(matches!(futures::poll!(b.as_mut()), Poll::Pending));
    assert_eq!(*entered.borrow(), 1, "B entered the backing store early");

    // C (transact): also strictly queued behind A and B; it also removes
    // the pre-seeded obsolete key from the real backing.
    let mut c = pin!(store.transact(vec![
        KvOp::Set(k.clone(), Bytes::from_static(b"C")),
        KvOp::Remove(obsolete.clone()),
    ]));
    assert!(matches!(futures::poll!(c.as_mut()), Poll::Pending));
    assert_eq!(
        *entered.borrow(),
        1,
        "transact entered the backing store early"
    );

    // Release A: the owner must then run A, B, C in exactly that order.
    let _ = release_tx.send(());
    entered_reaches(&mut entered, 2).await; // B applied
    assert!(matches!(futures::poll!(a.as_mut()), Poll::Ready(Ok(_))));
    entered_reaches(&mut entered, 3).await; // transact applied
    assert!(matches!(futures::poll!(b.as_mut()), Poll::Ready(Ok(_))));
    assert!(matches!(futures::poll!(c.as_mut()), Poll::Ready(Ok(_))));

    assert_eq!(
        *mock.applied.lock().unwrap(),
        vec![
            Bytes::from_static(b"A"),
            Bytes::from_static(b"B"),
            Bytes::from_static(b"C")
        ],
        "backing set mutations must land in FIFO submission order"
    );
    assert_eq!(
        store.get(k.clone()).await.unwrap(),
        Bytes::from_static(b"C")
    );
    assert_eq!(
        store.inner().get(k.clone()).await.unwrap(),
        Bytes::from_static(b"C")
    );
    // Full mirror/backing equality incl exact size after the barrier.
    let mut mirror = store.iter_stream(64);
    let mirror = collect_sorted(&mut mirror).await;
    let mut backing = store.inner().iter_stream(64);
    let backing = collect_sorted(&mut backing).await;
    let expected = vec![(k.clone(), Bytes::from_static(b"C"))];
    assert_eq!(mirror, expected, "mirror must equal backing");
    assert_eq!(backing, expected, "backing must hold the final value");
    assert_eq!(store.cache_size(), 1);
    // The removal from C must have propagated to BOTH sides.
    assert!(store.cache().peek_with(&obsolete, |_, _| ()).is_none());
    assert!(mock.inner.get(obsolete).await.is_err());
}

// ============================================================================
// T2 — Async mode: A acks (tentative cache publication) while the backing
// write is gated; queued transact/reload/B stay blocked; after release the
// final mirror equals backing in command order.
// ============================================================================

/// First `set` parks (oneshot gate) BEFORE applying to `inner` — the
/// caller's ack (already sent by the owner) is observable while the
/// backing write is suspended. `scan_entered` counts `iter_stream` calls;
/// the constructor scan is separately identified (the test resets the
/// baseline after the completed constructor).
struct AsyncGateStore {
    inner: Arc<InMemoryStore>,
    entered: watch::Sender<usize>,
    gate: Gate,
    scan_entered: watch::Sender<usize>,
    applied: Mutex<Vec<Bytes>>,
}

impl AsyncGateStore {
    async fn enter(&self) -> usize {
        let mut n = 0usize;
        self.entered.send_modify(|e| {
            *e += 1;
            n = *e;
        });
        n
    }
}

#[async_trait]
impl Store for AsyncGateStore {
    async fn insert(&self, value: Bytes) -> crate::error::DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> crate::error::DbResult<bool> {
        let n = self.enter().await;
        if n == 1 {
            park_on_gate(&self.gate).await;
        }
        self.applied.lock().unwrap().push(value.clone());
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> crate::error::DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> crate::error::DbResult<bool> {
        let n = self.enter().await;
        if n == 1 {
            park_on_gate(&self.gate).await;
        }
        self.inner.remove(key).await
    }
    async fn transact(&self, ops: Vec<KvOp>) -> crate::error::DbResult<()> {
        let n = self.enter().await;
        if n == 1 {
            park_on_gate(&self.gate).await;
        }
        self.inner.transact(ops).await
    }
    fn iter_stream(&self, batch_size: usize) -> BatchStream {
        self.scan_entered.send_modify(|e| *e += 1);
        self.inner.iter_stream(batch_size)
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> BatchStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

#[tokio::test]
async fn rv_storage_e70a0ddc_async_ack_before_backing_fifo_blocks_rest() {
    let (gate, entered_rx, release_tx) = new_gate();
    let inner = Arc::new(InMemoryStore::new());
    // Obsolete key seeded into the REAL backing before the constructor
    // (direct inner access — no gate involved).
    let obsolete = rk(b"obsolete");
    inner
        .set(obsolete.clone(), Bytes::from_static(b"stale"))
        .await
        .unwrap();
    let mock = Arc::new(AsyncGateStore {
        inner,
        entered: watch::channel(0).0,
        gate,
        scan_entered: watch::channel(0).0,
        applied: Mutex::new(Vec::new()),
    });
    let store = Arc::new(
        CachedStore::new_async(mock.clone() as Arc<dyn Store>)
            .await
            .unwrap(),
    );
    // The constructor scan is separately identified: reset the scan
    // baseline now that the constructor has COMPLETED, so waits below
    // target reload scans only.
    mock.scan_entered.send_modify(|e| *e = 0);
    let k = rk(b"k");

    // A: ack delivered while the backing write is parked at the gate.
    let mut a = pin!(store.set(k.clone(), Bytes::from_static(b"A")));
    assert!(matches!(futures::poll!(a.as_mut()), Poll::Pending));
    let mut entered = mock.entered.subscribe();
    entered_reaches(&mut entered, 1).await;
    gate_entered(entered_rx).await; // the oneshot fired for A, too
    assert!(
        matches!(futures::poll!(a.as_mut()), Poll::Ready(Ok(true))),
        "async set must ack (tentative publish) while backing is gated"
    );
    // A is observable through the cache, but NOT yet in the backing store.
    assert_eq!(
        store.cache().peek_with(&k, |_, v| v.clone()),
        Some(Bytes::from_static(b"A"))
    );
    assert!(mock.inner.get(k.clone()).await.is_err());

    // B, a transaction C and a reload must ALL stay blocked (FIFO).
    let mut b = pin!(store.set(k.clone(), Bytes::from_static(b"B")));
    assert!(matches!(futures::poll!(b.as_mut()), Poll::Pending));
    let mut c = pin!(store.transact(vec![
        KvOp::Set(k.clone(), Bytes::from_static(b"C")),
        KvOp::Remove(obsolete.clone()),
    ]));
    assert!(matches!(futures::poll!(c.as_mut()), Poll::Pending));
    let mut scan = mock.scan_entered.subscribe();
    assert_eq!(*scan.borrow(), 0, "reload started before earlier commands");
    let mut reload = pin!(store.reload());
    // Poll ONCE to submit the reload into the FIFO (behind B and C) and
    // prove it is blocked, not completed — otherwise the scan watch below
    // would never advance.
    assert!(
        matches!(futures::poll!(reload.as_mut()), Poll::Pending),
        "reload must stay blocked behind the queued commands"
    );

    // Release: A lands, then B, then C, then the reload's scan runs.
    let _ = release_tx.send(());
    entered_reaches(&mut entered, 2).await; // B applied
    entered_reaches(&mut entered, 3).await; // transact applied
    entered_reaches(&mut scan, 1).await; // reload scan entered (post-ctor baseline)
    assert!(matches!(futures::poll!(b.as_mut()), Poll::Ready(Ok(_))));
    assert!(matches!(futures::poll!(c.as_mut()), Poll::Ready(Ok(_))));
    tokio::time::timeout(HANG, reload.as_mut())
        .await
        .expect("reload hung")
        .expect("reload must succeed");

    assert_eq!(
        *mock.applied.lock().unwrap(),
        vec![Bytes::from_static(b"A"), Bytes::from_static(b"B")],
        "backing set order must be A then B (C applied via transact)"
    );
    assert_eq!(
        store.get(k.clone()).await.unwrap(),
        Bytes::from_static(b"C")
    );
    assert_eq!(
        store.inner().get(k.clone()).await.unwrap(),
        Bytes::from_static(b"C")
    );
    // Full mirror/backing equality incl exact size after the barrier.
    let mut mirror = store.iter_stream(64);
    let mirror = collect_sorted(&mut mirror).await;
    let mut backing = store.inner().iter_stream(64);
    let backing = collect_sorted(&mut backing).await;
    let expected = vec![(k.clone(), Bytes::from_static(b"C"))];
    assert_eq!(mirror, expected, "mirror must equal backing");
    assert_eq!(backing, expected, "backing must hold the final value");
    assert_eq!(store.cache_size(), 1);
    // The removal from C must have propagated to BOTH sides.
    assert!(store.cache().peek_with(&obsolete, |_, _| ()).is_none());
    assert!(mock.inner.get(obsolete).await.is_err());
}

// ============================================================================
// T3 — A parked reader's stale Fill must not resurrect a removed /
// externally-reloaded key.
// ============================================================================

/// `get` freezes the real read result, signals `entered`, then parks on
/// the oneshot gate before the reader can submit its Fill. ONLY the
/// constructor scan (call 1) filters out key "k" so the mirror starts
/// cold on k; every LATER scan returns the complete backing, so a reload
/// observes external mutations.
struct GatedGetStore {
    real: Arc<InMemoryStore>,
    gate: Gate,
    scan_calls: AtomicUsize,
}

#[async_trait]
impl Store for GatedGetStore {
    async fn insert(&self, value: Bytes) -> crate::error::DbResult<RecordKey> {
        self.real.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> crate::error::DbResult<bool> {
        self.real.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> crate::error::DbResult<Bytes> {
        let res = self.real.get(key).await;
        park_on_gate(&self.gate).await;
        res
    }
    async fn remove(&self, key: RecordKey) -> crate::error::DbResult<bool> {
        self.real.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> BatchStream {
        let n = self.scan_calls.fetch_add(1, Ordering::SeqCst) + 1;
        let real = self.real.clone();
        Box::pin(stream! {
            let mut s = real.iter_stream(batch_size);
            while let Some(batch) = s.next().await {
                match batch {
                    Ok(items) => {
                        let items = if n == 1 {
                            // Constructor scan only: keep k out of the
                            // initial mirror.
                            items
                                .into_iter()
                                .filter(|(k, _)| k.as_slice() != &b"k"[..])
                                .collect()
                        } else {
                            // Later scans are complete (reload sees
                            // EXTERNAL).
                            items
                        };
                        yield Ok(items);
                    }
                    Err(e) => yield Err(e),
                }
            }
        })
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> BatchStream {
        self.real.scan_prefix_stream(prefix, batch_size)
    }
}

#[tokio::test]
async fn rv_storage_e70a0ddc_stale_fill_not_resurrect_after_remove_and_reload() {
    let real = Arc::new(InMemoryStore::new());
    real.set(rk(b"k"), Bytes::from_static(b"V1")).await.unwrap();
    let (gate, entered_rx, release_tx) = new_gate();
    let mock = Arc::new(GatedGetStore {
        real: real.clone(),
        gate,
        scan_calls: AtomicUsize::new(0),
    });
    let store = Arc::new(
        CachedStore::new_sync(mock.clone() as Arc<dyn Store>)
            .await
            .unwrap(),
    );
    let k = rk(b"k");

    // Reader misses the mirror, freezes V1 in the mock, parks before
    // submitting its Fill.
    let mut reader = pin!(store.get(k.clone()));
    assert!(matches!(futures::poll!(reader.as_mut()), Poll::Pending));
    gate_entered(entered_rx).await;

    // While parked: the owner removes k, then a quiescent external change
    // plus a reload establishes a DIFFERENT value in the mirror.
    let remover = tokio::time::timeout(HANG, store.remove(k.clone()));
    tokio::time::timeout(HANG, pin!(remover))
        .await
        .expect("remove hung")
        .unwrap()
        .unwrap();
    real.set(k.clone(), Bytes::from_static(b"EXTERNAL"))
        .await
        .unwrap();
    tokio::time::timeout(HANG, store.reload())
        .await
        .expect("reload hung")
        .unwrap();
    assert_eq!(
        store.cache().peek_with(&k, |_, v| v.clone()),
        Some(Bytes::from_static(b"EXTERNAL"))
    );

    // Release the reader: its Fill carries the pre-remove epoch, so the
    // owner rejects it. The reader still returns its own earlier read, but
    // the mirror must NOT be overwritten with the stale V1.
    let _ = release_tx.send(());
    let got = tokio::time::timeout(HANG, reader.as_mut())
        .await
        .expect("reader hung")
        .unwrap();
    assert_eq!(got, Bytes::from_static(b"V1"));
    assert_eq!(
        store.cache().peek_with(&k, |_, v| v.clone()),
        Some(Bytes::from_static(b"EXTERNAL")),
        "stale fill resurrected a removed/reloaded key"
    );
    assert_eq!(store.cache_size(), 1);
}

// ============================================================================
// T4 — Two queued reloads; cancelling the first CALLER does not cancel the
// accepted command; the second reload must not start early. The cancelled
// caller's command is pinned/polled (already accepted) BEFORE the drop,
// R2's non-entry is proven by its not-yet-fired scan oneshot, a queued
// writer is submitted BEHIND R2 while R1 is still parked (and must not
// execute before R1 and R2 finish), and after the barrier the full mirror
// and backing are compared key-sorted, INCLUDING the externally removed
// key's absence.
// ============================================================================

/// Call 1 is the constructor scan (pass-through). The FIRST RELOAD scan
/// (call 2) takes the oneshot gate: it signals `entered1` and parks until
/// `release1`. Every reload scan (calls 2, 3, ...) fires the next sender
/// pre-loaded into `scan_entered` when it STARTS, so the test can prove
/// R2 has NOT started by polling its oneshot for Pending.
struct ReloadGateStore {
    inner: Arc<InMemoryStore>,
    scan_calls: AtomicUsize,
    gate: Gate,
    scan_entered: Mutex<Vec<oneshot::Sender<()>>>,
}

#[async_trait]
impl Store for ReloadGateStore {
    async fn insert(&self, value: Bytes) -> crate::error::DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> crate::error::DbResult<bool> {
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> crate::error::DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> crate::error::DbResult<bool> {
        self.inner.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> BatchStream {
        let n = self.scan_calls.fetch_add(1, Ordering::SeqCst) + 1;
        let gate = self.gate.clone();
        let inner = self.inner.clone();
        // Fire the pre-loaded oneshot for this reload scan (calls >= 2);
        // FIFO order — R1's sender was pre-loaded first.
        if n >= 2 {
            let tx = {
                let mut v = self.scan_entered.lock().unwrap();
                if v.is_empty() {
                    None
                } else {
                    Some(v.remove(0))
                }
            };
            if let Some(tx) = tx {
                let _ = tx.send(());
            }
        }
        let gated = n == 2; // R1 only; the constructor scan never parks.
        Box::pin(stream! {
            if gated {
                park_on_gate(&gate).await;
            }
            let mut s = inner.iter_stream(batch_size);
            while let Some(batch) = s.next().await {
                yield batch;
            }
        })
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> BatchStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

#[tokio::test]
async fn rv_storage_e70a0ddc_two_reloads_first_caller_cancel_command_persists() {
    let (gate, entered1_rx, release1_tx) = new_gate();
    // Pre-load one oneshot per reload scan: [R1, R2].
    let (r1_tx, r1_rx) = oneshot::channel();
    let (r2_tx, mut r2_rx) = oneshot::channel();
    let mock = Arc::new(ReloadGateStore {
        inner: Arc::new(InMemoryStore::new()),
        scan_calls: AtomicUsize::new(0),
        gate,
        scan_entered: Mutex::new(vec![r1_tx, r2_tx]),
    });

    // Backing seeded BEFORE the constructor: the mirror starts as {a, b}.
    mock.inner
        .set(rk(b"a"), Bytes::from_static(b"1"))
        .await
        .unwrap();
    mock.inner
        .set(rk(b"b"), Bytes::from_static(b"2"))
        .await
        .unwrap();
    let store = Arc::new(
        CachedStore::new_sync(mock.clone() as Arc<dyn Store>)
            .await
            .unwrap(),
    );
    let k = rk(b"k");

    // Quiescent external change while the mirror is cold on it: c added,
    // a REMOVED. After any successful reload both sides must be {b, c}.
    mock.inner
        .set(rk(b"c"), Bytes::from_static(b"3"))
        .await
        .unwrap();
    mock.inner.remove(rk(b"a")).await.unwrap();

    // R1: Box::pin (a stack `pin!` would NOT be dropped by `drop` of the
    // `Pin<&mut>`); polled ONCE — Pending proves the command was submitted
    // AND is still blocked (its scan is about to park).
    let mut r1 = Box::pin(store.reload());
    assert!(
        matches!(futures::poll!(r1.as_mut()), Poll::Pending),
        "R1 must block on its gated scan"
    );
    gate_entered(entered1_rx).await; // R1's scan is parked inside the gate
    gate_entered(r1_rx).await; // (same scan fired its oneshot too)

    // R2: pinned and polled ONCE — Pending proves the command is QUEUED
    // behind R1 (accepted, not executed).
    let mut r2 = pin!(store.reload());
    assert!(
        matches!(futures::poll!(r2.as_mut()), Poll::Pending),
        "R2 must stay queued behind R1"
    );
    assert!(
        matches!(futures::poll!(&mut r2_rx), Poll::Pending),
        "R2's scan must NOT have started while R1 is parked"
    );

    // A writer submitted WHILE R1 is parked: it must queue behind R2 (and
    // therefore behind R1) and not execute early.
    let mut w = pin!(store.set(k.clone(), Bytes::from_static(b"v")));
    assert!(
        matches!(futures::poll!(w.as_mut()), Poll::Pending),
        "writer must stay queued behind R2 while R1 is parked"
    );
    assert!(
        matches!(futures::poll!(&mut r2_rx), Poll::Pending),
        "R2's scan must still not have started after the writer was queued"
    );

    // Cancel the R1 CALLER (not the owner): dropping the boxed future
    // cancels the caller; the already-accepted command persists in the
    // FIFO and runs to completion; its reply send fails silently.
    // Card revert (precise): without FIFO ownership the cancelled
    // caller's reload would simply never finish and R2 would hang below.
    drop(r1);

    // Release R1's scan; the owner then finishes R1, runs R2, then the
    // queued writer in FIFO order. Awaited DbResults are the evidence
    // (HANG only detects hangs).
    let _ = release1_tx.send(());
    tokio::time::timeout(HANG, r2.as_mut())
        .await
        .expect("R2 hung behind cancelled R1")
        .expect("R2 reload must succeed");
    gate_entered(r2_rx).await; // R2's scan really ran

    // The queued writer runs last, after both reloads.
    tokio::time::timeout(HANG, w.as_mut())
        .await
        .expect("queued writer hung behind the reloads")
        .unwrap();

    // Full mirror/backing equality incl the removal: key "a" is absent
    // from BOTH sides, and the writer's value landed in both.
    let mut mirror = store.iter_stream(64);
    let mirror = collect_sorted(&mut mirror).await;
    let mut backing = store.inner().iter_stream(64);
    let backing = collect_sorted(&mut backing).await;
    let expected = vec![
        (rk(b"b"), Bytes::from_static(b"2")),
        (rk(b"c"), Bytes::from_static(b"3")),
        (k, Bytes::from_static(b"v")),
    ];
    assert_eq!(
        mirror, expected,
        "mirror must equal backing after R1 cancel"
    );
    assert_eq!(backing, expected, "backing must equal the expected image");
    assert_eq!(store.cache_size(), 3);
}

// ============================================================================
// T5 — A reload whose scan fails on a late batch must leave the live
// mirror completely unchanged, and the store must stay healthy.
// ============================================================================

/// Normal scan on call 1 (the constructor). From call 2 on: yields ONE
/// good batch, then a failed batch (late error).
struct LateScanErrStore {
    inner: Arc<InMemoryStore>,
    scan_calls: AtomicUsize,
}

#[async_trait]
impl Store for LateScanErrStore {
    async fn insert(&self, value: Bytes) -> crate::error::DbResult<RecordKey> {
        self.inner.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> crate::error::DbResult<bool> {
        self.inner.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> crate::error::DbResult<Bytes> {
        self.inner.get(key).await
    }
    async fn remove(&self, key: RecordKey) -> crate::error::DbResult<bool> {
        self.inner.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> BatchStream {
        let n = self.scan_calls.fetch_add(1, Ordering::SeqCst) + 1;
        let inner = self.inner.clone();
        Box::pin(stream! {
            if n == 1 {
                // Constructor scan: delegate normally.
                let mut s = inner.iter_stream(batch_size);
                while let Some(batch) = s.next().await {
                    yield batch;
                }
            } else {
                // Reload scan: one good batch, then a late failure.
                let mut s = inner.iter_stream(1);
                if let Some(first) = s.next().await {
                    yield first;
                }
                yield Err(crate::error::DbError::Storage(
                    "simulated late scan failure".into(),
                ));
            }
        })
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> BatchStream {
        self.inner.scan_prefix_stream(prefix, batch_size)
    }
}

#[tokio::test]
async fn rv_storage_e70a0ddc_reload_scan_error_leaves_live_mirror_unchanged() {
    let inner = Arc::new(InMemoryStore::new());
    inner.set(rk(b"a"), Bytes::from_static(b"1")).await.unwrap();
    inner.set(rk(b"b"), Bytes::from_static(b"2")).await.unwrap();
    let mock = Arc::new(LateScanErrStore {
        inner: inner.clone(),
        scan_calls: AtomicUsize::new(0),
    });
    let store = Arc::new(CachedStore::new_sync(mock as Arc<dyn Store>).await.unwrap());

    // Quiescent external change the failed reload must NOT partially apply.
    inner
        .set(rk(b"b"), Bytes::from_static(b"CHANGED"))
        .await
        .unwrap();
    inner.set(rk(b"c"), Bytes::from_static(b"3")).await.unwrap();

    let res = tokio::time::timeout(HANG, store.reload())
        .await
        .expect("reload hung")
        .expect_err("reload must report the late scan failure");

    // Live mirror is EXACTLY the pre-reload image, byte for byte.
    assert_eq!(store.cache_size(), 2);
    assert_eq!(
        store.cache().peek_with(&rk(b"a"), |_, v| v.clone()),
        Some(Bytes::from_static(b"1"))
    );
    assert_eq!(
        store.cache().peek_with(&rk(b"b"), |_, v| v.clone()),
        Some(Bytes::from_static(b"2")),
        "failed reload must leave the live tree unchanged (got {res})"
    );
    assert!(store.cache().peek_with(&rk(b"c"), |_, _| ()).is_none());

    // Scan failure is NOT terminal: subsequent commands work.
    tokio::time::timeout(HANG, store.set(rk(b"d"), Bytes::from_static(b"4")))
        .await
        .expect("store wedged after failed reload")
        .unwrap();
    assert_eq!(
        store.cache().peek_with(&rk(b"d"), |_, v| v.clone()),
        Some(Bytes::from_static(b"4"))
    );
    assert_eq!(store.cache_size(), 3);
}

// ============================================================================
// T6 — A reader that captures its epoch DURING an ongoing mutation (the
// odd in-flight epoch) with its inner snapshot frozen BEFORE the mutation
// commits must have its Fill rejected AT COMPLETION.
//
// EXACT REVERT (this test's own card, RV-STORAGE-e70a0ddc): the fill is
// rejected by TWO cooperating mechanisms, so the test fails only when the
// card's full production protocol is reverted — i.e. EITHER the entire
// FIFO+epoch protocol (odd pre-command bump, completion bump via
// `finish_mutation`, and the `Fill` command's exact-even check) is
// removed, OR BOTH the completion bump AND the odd/in-flight capture
// rejection in `Command::Fill` (`cur != captured || cur % 2 == 1`) are
// removed together. Reverting the completion bump ALONE does NOT fail
// this test: with the pre-command odd bump still in place the captured
// epoch is odd and the Fill is still rejected by the odd-capture check —
// the card is protected in depth.
// ============================================================================

/// `set` parks on the `set_gate` oneshot INSIDE the backing write (after
/// the owner's pre-command epoch bump); `get` freezes the real read
/// result, signals `get_entered`, then parks on the `get_gate` oneshot
/// before returning. ONLY the constructor scan (call 1) filters out key
/// "k" so the mirror starts cold on k; later scans are complete.
struct RaceFillStore {
    real: Arc<InMemoryStore>,
    set_gate: Gate,
    get_gate: Gate,
    scan_calls: AtomicUsize,
}

#[async_trait]
impl Store for RaceFillStore {
    async fn insert(&self, value: Bytes) -> crate::error::DbResult<RecordKey> {
        self.real.insert(value).await
    }
    async fn set(&self, key: RecordKey, value: Bytes) -> crate::error::DbResult<bool> {
        park_on_gate(&self.set_gate).await;
        self.real.set(key, value).await
    }
    async fn get(&self, key: RecordKey) -> crate::error::DbResult<Bytes> {
        let res = self.real.get(key).await;
        park_on_gate(&self.get_gate).await;
        res
    }
    async fn remove(&self, key: RecordKey) -> crate::error::DbResult<bool> {
        self.real.remove(key).await
    }
    fn iter_stream(&self, batch_size: usize) -> BatchStream {
        let n = self.scan_calls.fetch_add(1, Ordering::SeqCst) + 1;
        let real = self.real.clone();
        Box::pin(stream! {
            let mut s = real.iter_stream(batch_size);
            while let Some(batch) = s.next().await {
                match batch {
                    Ok(items) => {
                        let items = if n == 1 {
                            items
                                .into_iter()
                                .filter(|(k, _)| k.as_slice() != &b"k"[..])
                                .collect()
                        } else {
                            items
                        };
                        yield Ok(items);
                    }
                    Err(e) => yield Err(e),
                }
            }
        })
    }
    fn scan_prefix_stream(&self, prefix: Bytes, batch_size: usize) -> BatchStream {
        self.real.scan_prefix_stream(prefix, batch_size)
    }
}

#[tokio::test]
async fn rv_storage_e70a0ddc_fill_captured_during_mutation_rejected_at_completion() {
    let real = Arc::new(InMemoryStore::new());
    real.set(rk(b"k"), Bytes::from_static(b"OLD"))
        .await
        .unwrap();
    let (set_gate, set_entered_rx, set_release_tx) = new_gate();
    let (get_gate, get_entered_rx, get_release_tx) = new_gate();
    let mock = Arc::new(RaceFillStore {
        real: real.clone(),
        set_gate,
        get_gate,
        scan_calls: AtomicUsize::new(0),
    });
    let store = Arc::new(
        CachedStore::new_sync(mock.clone() as Arc<dyn Store>)
            .await
            .unwrap(),
    );
    let k = rk(b"k");

    // Start the mutation: the owner bumps the epoch (now ODD) and parks
    // INSIDE the backing write — the mirror is still cold on k.
    let mut mutation = pin!(store.set(k.clone(), Bytes::from_static(b"NEW")));
    assert!(matches!(futures::poll!(mutation.as_mut()), Poll::Pending));
    gate_entered(set_entered_rx).await;
    assert!(store.cache().peek_with(&k, |_, _| ()).is_none());

    // The reader starts WHILE the mutation is in flight: it captures the
    // ODD in-flight epoch, misses the mirror, freezes OLD in the mock's
    // get, and parks before submitting its Fill.
    let mut reader = pin!(store.get(k.clone()));
    assert!(matches!(futures::poll!(reader.as_mut()), Poll::Pending));
    gate_entered(get_entered_rx).await;

    // The mutation commits: completion bump back to EVEN, mirror = NEW.
    let _ = set_release_tx.send(());
    tokio::time::timeout(HANG, mutation.as_mut())
        .await
        .expect("mutation hung")
        .expect("mutation must succeed");
    assert_eq!(
        store.cache().peek_with(&k, |_, v| v.clone()),
        Some(Bytes::from_static(b"NEW"))
    );

    // Release the reader: its Fill carries the odd in-flight epoch and
    // must be rejected AT COMPLETION. The reader still returns its own
    // earlier read, but the mirror must keep the committed value.
    let _ = get_release_tx.send(());
    let got = tokio::time::timeout(HANG, reader.as_mut())
        .await
        .expect("reader hung")
        .unwrap();
    assert_eq!(got, Bytes::from_static(b"OLD"));
    assert_eq!(
        store.cache().peek_with(&k, |_, v| v.clone()),
        Some(Bytes::from_static(b"NEW")),
        "stale fill captured during the in-flight mutation resurrected OLD"
    );
    assert_eq!(store.cache_size(), 1);
}
