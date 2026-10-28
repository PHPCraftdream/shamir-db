// F-56 (#882) — loom model of the writer-drain barrier's interleaving contract.
//
// F-84 (#912) correction: loom 0.7.x does not model the total order of
// SeqCst accesses; its SeqCst-specific access behavior is a documented no-op.
// A SeqCst store/load pair can therefore exhibit store-buffering (SB), unlike
// real Rust SeqCst accesses. Explicit `fence(SeqCst)` is enforced by loom and
// is used here solely to model the production SC-order guarantee. These fences
// have no production counterpart. The fenced model does not prove the production
// orderings themselves: the production guarantee rests on the SC total-order
// proof documented on `WriterDrainBarrier` and the orderings pin gate.
//
// The model does guard the protocol's interleaving contract. It snapshots the
// write at the exact point drain returns, before joining the writer; sampling
// after join would be tautological because join waits for the writer to finish.
use crate::table::support::guards::writer_drain_barrier::WriterDrainBarrier;
use loom::sync::atomic::{fence, AtomicBool, AtomicUsize, Ordering};
use loom::sync::Arc;
use loom::thread;

/// The original F-84 interleaving model, retained verbatim in behavior.
struct Model {
    active: AtomicUsize,
    flag: AtomicBool,
    writer_wrote: AtomicBool,
    wrote_at_drain_return: AtomicBool,
}

impl Model {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            active: AtomicUsize::new(0),
            flag: AtomicBool::new(false),
            writer_wrote: AtomicBool::new(false),
            wrote_at_drain_return: AtomicBool::new(false),
        })
    }
}

fn run_writer(m: Arc<Model>) -> bool {
    m.active.fetch_add(1, Ordering::SeqCst);
    // Modeling-only fence compensates for loom 0.7's SeqCst-access limitation.
    fence(Ordering::SeqCst);
    let fast = !m.flag.load(Ordering::SeqCst);
    if fast {
        m.writer_wrote.store(true, Ordering::SeqCst);
    }
    m.active.fetch_sub(1, Ordering::SeqCst);
    fast
}

fn run_drainer(m: &Model) {
    m.flag.store(true, Ordering::SeqCst);
    // Modeling-only fence; production uses real SeqCst accesses.
    fence(Ordering::SeqCst);
    while m.active.load(Ordering::SeqCst) != 0 {
        thread::yield_now();
    }
    m.wrote_at_drain_return
        .store(m.writer_wrote.load(Ordering::SeqCst), Ordering::SeqCst);
}

#[test]
fn drain_returns_only_after_fast_path_writer_completes() {
    loom::model(|| {
        let m = Model::new();
        let m_w = Arc::clone(&m);
        let writer = thread::spawn(move || run_writer(m_w));
        run_drainer(&m);
        let took_fast = writer.join().unwrap();
        if took_fast {
            assert!(
                m.wrote_at_drain_return.load(Ordering::SeqCst),
                "drain returned before the fast-path writer's write landed"
            );
        }
    });
}

#[test]
fn rv_engine_898ecb5e_production_barrier_drain_sees_inflight_writer_loom_model() {
    loom::model(|| {
        let barrier = WriterDrainBarrier::new();
        let flag = Arc::new(AtomicBool::new(false));
        let wrote = Arc::new(AtomicBool::new(false));
        let writer_barrier = barrier.clone();
        let writer_flag = Arc::clone(&flag);
        let writer_wrote = Arc::clone(&wrote);
        let writer = thread::spawn(move || {
            let guard = writer_barrier.enter_writer();
            let took_fast = !writer_flag.load(Ordering::Relaxed);
            if took_fast {
                writer_wrote.store(true, Ordering::Relaxed);
            }
            drop(guard);
            took_fast
        });

        flag.store(true, Ordering::Relaxed);
        barrier.begin_drain();
        while !barrier.is_drained() {
            thread::yield_now();
        }
        let snapshot = wrote.load(Ordering::Relaxed);
        let took_fast = writer.join().unwrap();
        assert!(!took_fast || snapshot);
    });
}

#[test]
#[should_panic(expected = "unfenced SB")]
fn rv_engine_898ecb5e_oracle_detects_unfenced_sb_loom_model() {
    loom::model(|| {
        let active = Arc::new(AtomicUsize::new(0));
        let flag = Arc::new(AtomicBool::new(false));
        let wrote = Arc::new(AtomicBool::new(false));
        let writer_active = Arc::clone(&active);
        let writer_flag = Arc::clone(&flag);
        let writer_wrote = Arc::clone(&wrote);
        let writer = thread::spawn(move || {
            writer_active.fetch_add(1, Ordering::Relaxed);
            fence(Ordering::SeqCst);
            let took_fast = !writer_flag.load(Ordering::Relaxed);
            if took_fast {
                writer_wrote.store(true, Ordering::Relaxed);
            }
            writer_active.fetch_sub(1, Ordering::Release);
            took_fast
        });

        flag.store(true, Ordering::Relaxed);
        fence(Ordering::SeqCst);
        while active.load(Ordering::Acquire) != 0 {
            thread::yield_now();
        }
        let snapshot = wrote.load(Ordering::Relaxed);
        let took_fast = writer.join().unwrap();
        assert!(!took_fast || snapshot, "unfenced SB: drain missed writer");
    });
}
