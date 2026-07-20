//! D2 — overlay-ordering race test: "reader sees version ⟹ reader sees value".
//!
//! Turns the D2 ack-path ordering invariant from *proof-by-construction* into
//! *proof-by-test*. The ack-path
//! ([`MvccStore::apply_committed_visible`](crate::mvcc_store::MvccStore::apply_committed_visible))
//! orders strictly: `overlay.insert` → cell publish (`finalize_reservation`,
//! `cell.version = V`) → floor advance (`publish_committed_max`).
//!
//! Guarantee under test: **there is no window where a reader observes
//! `cell.version == V` while the value for `V` is NEITHER in the overlay NOR
//! drained to history.** Contrapositive (the test oracle): a reader that
//! observes freshness `version_of(K) == V (> 0)` MUST then be able to resolve
//! the value at a snapshot `>= V` to `Some(_)`. A `None` at that point would
//! mean the cell advertised `V` before the value was anywhere — i.e. the
//! ordering window the invariant forbids. Catching `None` here is a REAL D2
//! bug, not a test-tuning issue.
//!
//! How the window is hit: the reader reads the cell-version (`version_of`)
//! SEPARATELY from the value (`get_at`). These are two distinct lock-free
//! observations with no atomic coupling, so under `multi_thread` parallelism a
//! reader can land its `version_of` read at any interleaving relative to the
//! writer's `overlay.insert` / cell-publish steps. If the writer published the
//! cell BEFORE inserting into the overlay, a reader interleaving between the
//! two would see the bumped version yet resolve no value — exactly the window.
//! The production order (overlay BEFORE cell) closes it.
//!
//! Two arms:
//! * **overlay arm** — the value lives in the overlay (no drain between
//!   commits). `get_at` resolves via the overlay-probe branch.
//! * **history arm** — the value has been drained to history and evicted from
//!   the overlay (`write_committed_to_history` + `gc_overlay_to`). The cell
//!   still reports `V`; `get_at` must now resolve from history. Same invariant,
//!   the other plane.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use shamir_storage::types::KvOp;

use super::helpers::{make_gate, make_mvcc_with_gate};
use crate::mvcc_store::MvccStore;

/// The single contended key both arms hammer.
const KEY: &[u8] = b"overlay-ordering-K";

/// Build the value payload for version `v` so a resolved read can be checked
/// against a *real* committed version (not just "non-None"): the value encodes
/// its own version.
fn val_for(v: u64) -> Bytes {
    Bytes::from(format!("v{v}").into_bytes())
}

/// Parse the version back out of a value produced by [`val_for`]. Returns
/// `None` if the bytes are not in the `v<NNN>` shape (which would itself be a
/// corruption bug).
fn version_in(val: &Bytes) -> Option<u64> {
    let s = std::str::from_utf8(val).ok()?;
    s.strip_prefix('v')?.parse::<u64>().ok()
}

/// One reader iteration, shared by both arms.
///
/// Observe the cell-version SEPARATELY from the value, then assert the
/// invariant. Returns `Err(message)` on a violation (caller stops the run).
async fn read_once(mvcc: &MvccStore) -> Result<(), String> {
    // Observation 1: cell freshness (the version the reader "sees").
    let v = mvcc.version_of(KEY);
    if v == 0 {
        // No version published yet — nothing claimed, nothing to resolve.
        return Ok(());
    }
    // Observation 2: resolve the value at a snapshot >= the observed version.
    // `get_at(KEY, v)` takes the direct overlay-probe → history path for
    // `cur_v <= v`. If the cell could ever report `v` before the value was
    // reachable, this returns `None` → invariant broken.
    match mvcc.get_at(KEY, v).await {
        Ok(Some(got)) => {
            // The resolved value must belong to SOME really-committed version
            // (it encodes its own). It may be a value newer than `v` if the
            // writer advanced the cell again between our two observations —
            // that is fine: still Some, still a real version. The forbidden
            // outcome is None.
            match version_in(&got) {
                Some(rv) if rv >= 1 => Ok(()),
                _ => Err(format!(
                    "resolved value for observed version {v} is not a valid \
                     committed payload: {got:?}"
                )),
            }
        }
        Ok(None) => Err(format!(
            "INVARIANT VIOLATION: reader observed cell.version == {v} but \
             get_at(KEY, {v}) resolved None — value is neither in overlay nor \
             history. This is the D2 ordering window (cell published before \
             value was reachable)."
        )),
        Err(e) => Err(format!("unexpected storage error during read: {e:?}")),
    }
}

/// The reader spin-loop, extracted so it can be wrapped in a
/// [`tokio::time::timeout`] at each call site. Spins until `stop` is set or a
/// violation is recorded, exactly as the original inline loop did.
async fn poll_until_stopped(
    stop: &Arc<AtomicBool>,
    mvcc: &MvccStore,
    id: usize,
    violation: &Arc<scc::HashMap<usize, String, shamir_collections::THasher>>,
) {
    while !stop.load(Ordering::Relaxed) {
        if let Err(msg) = read_once(mvcc).await {
            let _ = violation.insert_sync(id, msg);
            return;
        }
        tokio::task::yield_now().await;
    }
}

// ============================================================================
// Overlay arm — value lives ONLY in the overlay (no drain between commits).
// ============================================================================

/// Writer commits NEW versions V = 1,2,3,… of `KEY` via the real ack-path
/// ([`MvccStore::apply_committed_visible`], the inline visible half the
/// production commit routes), WITHOUT draining to history. The value therefore
/// lives only in the overlay, so every reader resolution exercises the
/// overlay-probe branch of `resolve_read`.
///
/// Parallel readers, in a tight loop, observe `version_of` then `get_at` and
/// assert the value is ALWAYS `Some`. Any `None` is a genuine ordering bug.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlay_ordering_reader_sees_version_implies_value_overlay_arm() {
    let gate = make_gate();
    let mvcc = Arc::new(make_mvcc_with_gate(gate.clone()));

    // Rounds × writes-per-round chosen to thoroughly interleave insert↔publish
    // across worker threads (hundreds of commits, several reader tasks each
    // doing many reads).
    const ROUNDS: u64 = 12;
    const WRITES_PER_ROUND: u64 = 60;
    const READERS: usize = 4;

    let stop = Arc::new(AtomicBool::new(false));
    // Records a violation string from any reader (first one wins).
    let violation: Arc<scc::HashMap<usize, String, shamir_collections::THasher>> = Arc::new(
        scc::HashMap::with_hasher(shamir_collections::THasher::default()),
    );

    // Spawn the readers FIRST so they are already hammering while the writer
    // races through its commit loop.
    let mut readers = Vec::with_capacity(READERS);
    for id in 0..READERS {
        let mvcc = Arc::clone(&mvcc);
        let stop = Arc::clone(&stop);
        let violation = Arc::clone(&violation);
        readers.push(tokio::spawn(async move {
            // Local wall-clock bound (30 s): the writer does 720 synchronous
            // in-memory commits (ROUNDS × WRITES_PER_ROUND), which completes
            // in milliseconds under normal conditions. 30 s is ~60× headroom
            // over that, yet 6× shorter than nextest's 180 s anonymous kill.
            // If the writer deadlocks (the exact bug class this test exists
            // to catch), this fires FAST with a diagnostic message naming the
            // condition, instead of an anonymous nextest TIMEOUT.
            let reached = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                poll_until_stopped(&stop, &mvcc, id, &violation),
            )
            .await;
            assert!(
                reached.is_ok(),
                "overlay-arm reader {id}: `stop` flag was never set within 30 s — \
                 the writer likely deadlocked in the ack-path (apply_committed_visible), \
                 the exact hazard this test exists to catch"
            );
        }));
    }

    // Writer: real ack-path commits, no drain → value stays in overlay.
    let writer = {
        let mvcc = Arc::clone(&mvcc);
        let gate = gate.clone();
        tokio::spawn(async move {
            for _ in 0..ROUNDS {
                for _ in 0..WRITES_PER_ROUND {
                    let v = gate.assign_next_version();
                    let ops = vec![KvOp::Set(Bytes::copy_from_slice(KEY).into(), val_for(v))];
                    // The exact production ack-path under proof: overlay.insert
                    // → cell publish → floor advance, synchronously.
                    mvcc.apply_committed_visible(&ops, v);
                    tokio::task::yield_now().await;
                }
            }
        })
    };

    writer.await.unwrap();
    // Let readers drain a few more iterations against the final state, then
    // signal stop and join.
    for _ in 0..READERS {
        tokio::task::yield_now().await;
    }
    stop.store(true, Ordering::Relaxed);
    for r in readers {
        r.await.unwrap();
    }

    // Assert no reader recorded an invariant violation.
    if !violation.is_empty() {
        let mut msgs = Vec::new();
        violation.iter_sync(|id, m| {
            msgs.push(format!("reader {id}: {m}"));
            true
        });
        panic!("overlay-arm invariant violated:\n{}", msgs.join("\n"));
    }

    // Sanity: the final cell version is resolvable to Some (post-run quiescent
    // check — the loop above already covers the racy window).
    let final_v = mvcc.version_of(KEY);
    assert!(
        final_v > 0,
        "writer must have published at least one version"
    );
    assert!(
        mvcc.get_at(KEY, final_v).await.unwrap().is_some(),
        "final committed version must resolve to a value",
    );
}

// ============================================================================
// History arm — value drained to history AND evicted from the overlay; the
// cell still reports V, so the reader must resolve from history.
// ============================================================================

/// After each ack-path commit the writer ALSO drains the value to history
/// ([`MvccStore::write_committed_to_history`]) and evicts the overlay copy
/// ([`MvccStore::gc_overlay_to`]). The cell stays at `V` (the drain re-publishes
/// it idempotently), but the value no longer lives in the overlay — so the
/// reader's `get_at(KEY, V)` MUST resolve from the durable history log. Same
/// invariant (`version implies value`), exercised on the history plane.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn overlay_ordering_reader_sees_version_implies_value_history_arm() {
    let gate = make_gate();
    let mvcc = Arc::new(make_mvcc_with_gate(gate.clone()));

    const ROUNDS: u64 = 12;
    const WRITES_PER_ROUND: u64 = 40;
    const READERS: usize = 4;

    let stop = Arc::new(AtomicBool::new(false));
    let violation: Arc<scc::HashMap<usize, String, shamir_collections::THasher>> = Arc::new(
        scc::HashMap::with_hasher(shamir_collections::THasher::default()),
    );
    // Tracks the highest version the writer has DRAINED to history. Used to gate
    // the overlay GC threshold so we never evict an overlay entry whose version
    // the cell currently reports but whose history write has not landed.
    let drained = Arc::new(AtomicU64::new(0));

    let mut readers = Vec::with_capacity(READERS);
    for id in 0..READERS {
        let mvcc = Arc::clone(&mvcc);
        let stop = Arc::clone(&stop);
        let violation = Arc::clone(&violation);
        readers.push(tokio::spawn(async move {
            // Local wall-clock bound (30 s): the writer does 480 async
            // in-memory log writes (ROUNDS × WRITES_PER_ROUND) plus overlay
            // GC — still sub-second under normal conditions. 30 s gives ~60×
            // headroom while being 6× shorter than nextest's 180 s kill.
            let reached = tokio::time::timeout(
                std::time::Duration::from_secs(30),
                poll_until_stopped(&stop, &mvcc, id, &violation),
            )
            .await;
            assert!(
                reached.is_ok(),
                "history-arm reader {id}: `stop` flag was never set within 30 s — \
                 the writer likely deadlocked in the ack-path or history-drain \
                 (apply_committed_visible / write_committed_to_history), the exact \
                 hazard this test exists to catch"
            );
        }));
    }

    let writer = {
        let mvcc = Arc::clone(&mvcc);
        let gate = gate.clone();
        let drained = Arc::clone(&drained);
        tokio::spawn(async move {
            for _ in 0..ROUNDS {
                for _ in 0..WRITES_PER_ROUND {
                    let v = gate.assign_next_version();
                    let ops = vec![KvOp::Set(Bytes::copy_from_slice(KEY).into(), val_for(v))];
                    // Ack-path: make the version visible (overlay + cell + floor).
                    mvcc.apply_committed_visible(&ops, v);
                    // Drain it to durable history (the value now lives in the log).
                    mvcc.write_committed_to_history(&ops, v).await.unwrap();
                    drained.store(v, Ordering::Release);
                    // Evict the overlay copy for everything strictly below `v`.
                    // We keep `v` itself in the overlay (gc_upto drops `<= wm`),
                    // so the *previous* versions are forced onto the history
                    // plane while the latest stays overlay-resolvable. Over the
                    // run this means most reader resolutions hit the history
                    // branch for any observed version below the writer's head.
                    if v > 1 {
                        mvcc.gc_overlay_to(v - 1);
                    }
                    tokio::task::yield_now().await;
                }
            }
        })
    };

    writer.await.unwrap();
    for _ in 0..READERS {
        tokio::task::yield_now().await;
    }
    stop.store(true, Ordering::Relaxed);
    for r in readers {
        r.await.unwrap();
    }

    if !violation.is_empty() {
        let mut msgs = Vec::new();
        violation.iter_sync(|id, m| {
            msgs.push(format!("reader {id}: {m}"));
            true
        });
        panic!("history-arm invariant violated:\n{}", msgs.join("\n"));
    }

    // Drive the history plane explicitly post-run: drain + evict the head, then
    // assert the cell's version still resolves — now necessarily from history.
    let final_v = mvcc.version_of(KEY);
    assert!(
        final_v > 0,
        "writer must have published at least one version"
    );
    mvcc.gc_overlay_to(final_v);
    assert_eq!(
        mvcc.overlay_len(),
        0,
        "overlay must be fully evicted after gc_overlay_to(final_v)",
    );
    let got = mvcc
        .get_at(KEY, final_v)
        .await
        .unwrap()
        .expect("history must resolve the final version after overlay eviction");
    assert_eq!(
        version_in(&got),
        Some(final_v),
        "history-resolved value must be the final committed version",
    );
}
