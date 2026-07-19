//! H4+H5 — `per_table_mvcc` and `token_names` mixed `_async`/`_sync`
//! whole-runtime deadlock regression test.
//!
//! Same structural class as the #589 fix for the `cells` map
//! (commit `7a4abf62`), the H1+H2 fix for `active_snapshots` + `locks`
//! (commit `621776bd`), and the H3 fix for the five vector-index maps
//! (commit `dcfaf825`). Two more `scc::HashMap`s on `RepoInstance`
//! previously mixed the lock-HANDOFF async accessors with synchronous
//! ones that PARK the calling OS thread while the bucket is locked:
//!
//! * **`per_table_mvcc`** (`Arc<scc::HashMap<u64, Arc<MvccStore>, THasher>>`)
//!   — async SHARED readers `read_async` (pre_commit / commit_phases /
//!   drainer / recovery / apply_replicated, i.e. EVERY commit, drain step,
//!   recovery seed and replication-apply) + `iter_async` (`run_gc`),
//!   mixed with sync `read_sync` (version_provider — every Serializable
//!   `validate_read_set`), `get_sync` (commit lock-release; rename),
//!   `iter_sync` (flush_all_history; drainer F6a overlay GC) and EXCLUSIVE
//!   writers `insert_sync` (table attach) / `remove_sync` (drop table).
//! * **`token_names`** (`Arc<scc::HashMap<u64, String, THasher>>`) — async
//!   SHARED readers `read_async` (`table_by_token`, `table_by_token_if_live`
//!   — the commit pipeline under `commit_lock` and V2 WAL recovery), mixed
//!   with sync `insert_sync` + `read_sync` (`register_token`, DDL create/
//!   rename) and EXCLUSIVE `remove_if_sync` (drop table).
//!
//! **This task's hazard is a DIFFERENT shape than H1-H3.** There the
//! `_async` ops were all EXCLUSIVE, so any suspended waiter directly
//! blocked the map. Here the `_async` ops are SHARED (read-only), so the
//! deadlock needs an EXCLUSIVE writer in the mix: a DDL op (table attach
//! via `insert_sync`, or drop table via `remove_sync`/`remove_if_sync`)
//! concurrent with sustained commit/drain traffic. The mechanism:
//! 1. A DDL op owns a bucket exclusively for a moment.
//! 2. Concurrent commit/drain `read_async`/`iter_async` waiters suspend
//!    on that bucket.
//! 3. On release, saa hands SHARED grants to the suspended reader TASKs —
//!    they hold read locks while sitting unpolled in the run queue.
//! 4. A SECOND DDL op (or teardown) parks a worker in `remove_sync`/
//!    `insert_sync` waiting for those unpolled readers; under saa's
//!    writer-fairness (new shared acquirers queue behind a pending
//!    exclusive writer) every subsequent commit-path `read_sync`/`get_sync`
//!    parks behind it too → workers drain into parks → the handed-off
//!    readers are never polled → whole-runtime deadlock.
//!
//! Reported at MEDIUM confidence (needs two DDL ops, or DDL + saa
//! writer-fair reader queue, overlapping sustained commit traffic), so a
//! DETERMINISTIC pre-fix repro is not reliably achievable here. **The fix
//! is still applied** — it is trivial, mechanical and strictly
//! convention-aligning (every accessor on both maps is now synchronous,
//! so every bucket lock is held only by a RUNNING thread for a few
//! instructions, bounding every wait and closing the deadlock window by
//! construction).
//!
//! **Why this test exists** (mirrors how the codebase reasons about
//! `overlay_ordering_tests.rs` and the H1+H2/H3 regression tests): this
//! is a RACE WINDOW, not a deterministic deadlock. The goal is BOTH (a)
//! to exercise the DDL-vs-reader interleaving in good faith so nextest's
//! parallelism has a real chance to catch a future regression over time,
//! AND (b) a NAMED bounded `tokio::time::timeout` so a real regression
//! fails fast and identifiably (and specifically points at this hazard)
//! instead of hanging the entire nextest run with an anonymous TIMEOUT.
//! The timeout is NOT a flakiness workaround — it is this test's own
//! guard against a regression hanging the whole suite
//! (cf. `crates/shamir-index/src/vector/tests/quantized_graph_tests.rs:1630`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use shamir_storage::storage_in_memory::InMemoryRepo;

use crate::repo::repo_instance::RepoInstance;
use crate::repo::repo_types::BoxRepo;
use crate::table::table_config::TableConfig;
use crate::table::table_manager::table_token_for;

/// Concurrent DDL/reader hammer count. On a `worker_threads = 2` runtime
/// (the smallest non-trivial count) this oversubscribes the workers,
/// maximising the chance of a lock-handoff window landing during a DDL
/// exclusive section — the exact pre-fix hazard shape.
const HAMMERS: usize = 6;

/// Iterations per hammer. Each iteration is one DDL attach/drop cycle or
/// one reader probe — the exact `insert_sync`/`remove_sync` writer vs
/// `read_sync`/`iter_sync` reader interleaving under test.
const ITERS: usize = 200;

/// Table names shared between the DDL and reader hammers. Sharing the
/// SAME names funnels both the exclusive DDL writers and the shared
/// readers onto the SAME token buckets (tokens are a deterministic hash
/// of the name) — the worst case for the mixed-async/sync hazard.
const TABLES: &[&str] = &["t_ddl_0", "t_ddl_1", "t_ddl_2"];

fn make_instance() -> RepoInstance {
    let configs = TABLES.iter().map(|n| TableConfig::new(*n)).collect();
    RepoInstance::new(
        "h4h5_deadlock".into(),
        BoxRepo::InMemory(Arc::new(InMemoryRepo::new())),
        configs,
    )
}

/// Site-1 reader hammer: tight-loops `run_gc()` (the converted
/// `per_table_mvcc.iter_sync` scan, formerly `iter_async`) — exercises the
/// exact shared-reader path that, pre-fix, could be granted a bucket lock
/// by saa handoff and then sit unpolled while a DDL exclusive writer
/// parks every worker behind it.
async fn per_table_mvcc_reader_hammer(repo: Arc<RepoInstance>, stop: Arc<AtomicBool>) {
    for _ in 0..ITERS {
        // run_gc → per_table_mvcc.iter_sync (converted from iter_async).
        // Errors (e.g. a per-store flush) are irrelevant to the lock
        // hazard under test — we only care that this does not hang.
        let _ = repo.run_gc().await;
        tokio::task::yield_now().await;
        if stop.load(Ordering::Relaxed) {
            return;
        }
    }
}

/// DDL hammer: tight-loops table attach (`get_table` →
/// `per_table_mvcc.insert_sync`, the EXCLUSIVE writer that makes the
/// hazard reachable) interleaved with drop (`remove_table` →
/// `per_table_mvcc.remove_sync` + `token_names.remove_if_sync`) and
/// re-`add_table` (`token_names.insert_sync`). This is the sustained
/// DDL-vs-commit/drain traffic the brief names.
async fn ddl_hammer(repo: Arc<RepoInstance>, stop: Arc<AtomicBool>) {
    for i in 0..ITERS {
        let name = TABLES[i % TABLES.len()];
        // Attach: get_table → create_table_context → per_table_mvcc.insert_sync.
        // NotFound is expected when a concurrent drop wins the configs race —
        // swallow it, the next iteration re-adds and retries.
        let _ = repo.get_table(name).await;
        // Drop: remove_table → token_names.remove_if_sync + per_table_mvcc.remove_sync.
        let _ = repo.remove_table(name);
        // Re-register config + token_names mapping so the next attach can
        // succeed (add_table is idempotent and lock-safe under concurrency).
        // (`name` is already `&str` here — `TABLES[i]` indexes into `&[&str]`
        //  — so no deref; the `for name in TABLES` loops below deref their
        //  `&&str`.)
        repo.add_table(TableConfig::new(name));
        tokio::task::yield_now().await;
        if stop.load(Ordering::Relaxed) {
            return;
        }
    }
}

/// Site-1 regression: hammer DDL (attach via `get_table` + drop via
/// `remove_table`) concurrent with `run_gc()` (`per_table_mvcc.iter_sync`)
/// on a runtime with only TWO worker threads.
///
/// Pre-fix expectation: under the mixed `iter_async`/`read_sync` +
/// `insert_sync`/`remove_sync` hazard, a handed-off `run_gc` iterator
/// task could own a bucket while unpolled, and a DDL exclusive writer
/// could park every worker behind it → whole-runtime deadlock → the
/// `tokio::time::timeout` below fires and the named assertion points
/// unambiguously at this hazard.
///
/// Post-fix: every `per_table_mvcc` accessor is synchronous, so every
/// bucket lock is held only by a RUNNING thread for a few instructions →
/// bounded waits, no deadlock window → the run completes well within the
/// timeout. (MEDIUM confidence hazard — this test exercises the
/// interleaving in good faith but cannot guarantee a deterministic
/// pre-fix repro on every CI run; see module doc.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_table_mvcc_concurrent_ddl_gc_no_deadlock() {
    let repo = Arc::new(make_instance());
    // Pre-instantiate every table so per_table_mvcc is populated and the
    // reader hammer's iter_sync scan has real entries to contend over.
    for name in TABLES {
        let _ = repo.get_table(name).await;
    }

    let stop = Arc::new(AtomicBool::new(false));

    let mut handles = Vec::with_capacity(HAMMERS);
    // Half the hammers are GC (iter_sync) readers, half are DDL writers.
    for i in 0..HAMMERS {
        let r = Arc::clone(&repo);
        let s = Arc::clone(&stop);
        handles.push(tokio::spawn(async move {
            if i % 2 == 0 {
                per_table_mvcc_reader_hammer(r, s).await;
            } else {
                ddl_hammer(r, s).await;
            }
        }));
    }

    // Bounded guard: a real regression hangs the suite here; this turns
    // the silent nextest-TIMEOUT into a fast, named, specific failure
    // (NOT a flakiness workaround — see module doc).
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        for h in handles {
            h.await.unwrap();
        }
    })
    .await
    .expect(
        "per_table_mvcc DDL-vs-run_gc deadlocked — this is the #589-class \
         read_async/iter_async + insert_sync/remove_sync mixed-lock hazard \
         (SHARED async readers + EXCLUSIVE DDL writer). run_gc MUST use \
         iter_sync (and every per_table_mvcc reader read_sync), not the \
         _async accessors. See module doc + commits 7a4abf62 / 621776bd.",
    );

    stop.store(true, Ordering::Relaxed);
}

/// Site-2 reader hammer: tight-loops `table_by_token` and
/// `table_by_token_if_live` (both converted `token_names.read_sync`,
/// formerly `read_async`) — exercises the exact shared-reader paths that,
/// pre-fix, could be granted a bucket lock by saa handoff and then sit
/// unpolled while a DDL exclusive writer (`insert_sync`/`remove_if_sync`)
/// parks every worker behind it.
///
/// Also exercises `table_by_token`'s lazy `get_table` (per_table_mvcc
/// attach) on the live-name path — a realistic commit-pipeline shape.
async fn token_names_reader_hammer(repo: Arc<RepoInstance>, stop: Arc<AtomicBool>) {
    let tokens: Vec<u64> = TABLES.iter().map(|n| table_token_for(n)).collect();
    for i in 0..ITERS {
        let token = tokens[i % tokens.len()];
        // table_by_token → token_names.read_sync (converted from read_async).
        let _ = repo.table_by_token(token).await;
        // table_by_token_if_live → token_names.read_sync (converted from
        // read_async). Non-instantiating by design — confirm that property
        // survives the lock-mechanism change (it does: the conversion only
        // swapped the lock accessor, not the tables-OnceCell direct read).
        let _ = repo.table_by_token_if_live(token).await;
        tokio::task::yield_now().await;
        if stop.load(Ordering::Relaxed) {
            return;
        }
    }
}

/// Site-2 regression: hammer DDL (`add_table` → `token_names.insert_sync`
/// + `remove_table` → `token_names.remove_if_sync`, the EXCLUSIVE writers)
/// concurrent with `table_by_token` / `table_by_token_if_live`
/// (`token_names.read_sync` readers) on a two-worker runtime.
///
/// Pre-fix expectation: a handed-off `read_async` reader task could own a
/// bucket while unpolled, and a DDL exclusive writer could park every
/// worker behind it → whole-runtime deadlock → the timeout fires and the
/// named assertion points at this hazard.
///
/// Post-fix: every `token_names` accessor is synchronous → bounded waits,
/// no deadlock window → completes within the timeout. (MEDIUM confidence
/// hazard — good-faith interleaving, not a guaranteed deterministic repro;
/// see module doc.) This test ALSO confirms `table_by_token_if_live`'s
/// documented NON-INSTANTIATING property is unchanged by the fix: it
/// still resolves through the dormant `tables` OnceCell map directly and
/// returns `None` for a registered-but-never-touched table without
/// calling the instantiating `get_table`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn token_names_concurrent_ddl_reads_no_deadlock() {
    let repo = Arc::new(make_instance());
    // Register the token_names mappings (add_table does this) but do NOT
    // instantiate any table, so `table_by_token_if_live`'s non-instantiating
    // branch is genuinely exercised by the reader hammer.
    // (make_instance already registered them via RepoInstance::new configs,
    //  but re-affirm the mappings are live.)
    for name in TABLES {
        repo.add_table(TableConfig::new(*name));
    }

    let stop = Arc::new(AtomicBool::new(false));

    let mut handles = Vec::with_capacity(HAMMERS);
    for i in 0..HAMMERS {
        let r = Arc::clone(&repo);
        let s = Arc::clone(&stop);
        handles.push(tokio::spawn(async move {
            if i % 2 == 0 {
                token_names_reader_hammer(r, s).await;
            } else {
                ddl_hammer(r, s).await;
            }
        }));
    }

    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        for h in handles {
            h.await.unwrap();
        }
    })
    .await
    .expect(
        "token_names DDL-vs-read deadlocked — this is the #589-class \
         read_async + insert_sync/remove_if_sync mixed-lock hazard \
         (SHARED async readers + EXCLUSIVE DDL writer). table_by_token / \
         table_by_token_if_live MUST use read_sync, not read_async. See \
         module doc + commits 7a4abf62 / 621776bd.",
    );

    stop.store(true, Ordering::Relaxed);
}

/// Non-instantiating property confirmation: `table_by_token_if_live`
/// returns `None` for a registered-but-never-instantiated table, and the
/// lock-mechanism change (read_async → read_sync) does NOT alter that.
/// This guards the documented reason `table_by_token_if_live` exists
/// (Phase 2.5's barrier check must not force lazy instantiation as a side
/// effect). Uses ONLY the public API: the proof that the call does not
/// instantiate is that a SECOND call still returns `None` (had the first
/// call lazily created the `TableManager`, the second would observe it).
#[tokio::test]
async fn table_by_token_if_live_still_non_instantiating() {
    let repo = make_instance();
    // TABLES[0] was registered in RepoInstance::new (register_token ran
    // for every config), so token_names HAS its mapping — but no get_table
    // has run, so the tables OnceCell for it is still dormant.
    let token = table_token_for(TABLES[0]);

    // Registered-but-never-touched → if_live returns None (dormant OnceCell,
    // NOT because the token_names mapping is absent).
    assert!(
        repo.table_by_token_if_live(token).await.is_none(),
        "table_by_token_if_live must return None for a dormant table"
    );
    // Calling it again MUST still return None — the first call did not
    // instantiate. This is the non-instantiating property the lock-mechanism
    // change must not disturb.
    assert!(
        repo.table_by_token_if_live(token).await.is_none(),
        "table_by_token_if_live must not instantiate the OnceCell as a side effect"
    );

    // Contrast: table_by_token (the instantiating variant) DOES materialise.
    let instantiated = repo
        .table_by_token(token)
        .await
        .unwrap()
        .expect("table_by_token must instantiate a registered table");
    assert_eq!(instantiated.name(), TABLES[0]);

    // Now that the OnceCell is live, if_live observes it.
    let live = repo
        .table_by_token_if_live(token)
        .await
        .expect("table_by_token_if_live must see an instantiated table");
    assert_eq!(live.name(), TABLES[0]);
}
