# F-84 (#912) — fix F-80's loom model's tautological interleaving assertion

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

This is a finding from an `@oh` adversarial review of the F-69..F-81
remediation wave (see `docs/checkpoints/p0-p1-wave-complete.md`, task
#912), independently CONFIRMED by the orchestrating session's own
sabotage-then-restore proof during F-80's (#907) own verification.

`crates/shamir-engine/src/table/writer_drain_barrier.rs`'s `loom_model`
module (lines ~512-585, added by F-56/#882, wired into CI by F-80/#907's
new `.github/workflows/ci.yml` `loom-model` job) has ONE test:

```rust
fn run_writer(m: Arc<Model>) -> bool {
    m.active.fetch_add(1, Ordering::SeqCst);
    let fast = !m.flag.load(Ordering::SeqCst);
    if fast {
        m.writer_wrote.store(true, Ordering::SeqCst);
    }
    m.active.fetch_sub(1, Ordering::SeqCst);
    fast
}

fn run_drainer(m: &Model) {
    m.flag.store(true, Ordering::SeqCst);
    while m.active.load(Ordering::SeqCst) != 0 {
        thread::yield_now();
    }
    // drain() has returned — every fast-path writer is out of the drain set.
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
                m.writer_wrote.load(Ordering::SeqCst),
                "drain returned before the fast-path writer's write landed — \
                 an interleaving hole in the drain contract"
            );
        }
    });
}
```

**The bug:** the assertion checks `m.writer_wrote` AFTER `writer.join()`.
`join()` always blocks until the writer thread has fully returned from
`run_writer` — regardless of what the drainer observed or when. Since
`run_writer` unconditionally stores `writer_wrote = true` before
returning whenever `fast == true`, the implication `took_fast == true ⟹
writer_wrote == true` holds **purely from thread completion**, with ZERO
dependence on what `run_drainer`'s spin loop actually did. The test never
samples state at the moment `drain()` (i.e. `run_drainer`'s loop) actually
returns — only at the moment the ENTIRE writer thread has finished,
which is a strictly later (or equal) point in every interleaving.

**Proof of vacuity** (reproduce this yourself before writing the fix, to
confirm you understand exactly what's broken): temporarily delete the
entire spin loop body from `run_drainer`, leaving only
`m.flag.store(true, Ordering::SeqCst);` (drain returns immediately,
unconditionally, without ever checking `active`). Run
`cargo nextest run -p shamir-engine --features loom --lib -E
'test(loom_model)' --nocapture` (touch the source file first if a stale
build reuses cached object code — this workspace hit a stale-build issue
during F-80's own verification; `touch
crates/shamir-engine/src/table/writer_drain_barrier.rs` before rebuilding
if a build finishes suspiciously fast with no "Compiling shamir-engine"
line). The test PASSES even with drain broken to be a complete no-op —
this is the exact regression class the CI job exists to catch, currently
caught by nothing.

## The fix

Capture a snapshot of `writer_wrote` INSIDE `run_drainer`, at the exact
instant its spin loop exits (i.e., the instant `drain()` would return in
the real, non-model code) — not after `join()`. Then assert against that
snapshot, not against post-join state.

```rust
struct Model {
    active: AtomicUsize,
    flag: AtomicBool,
    writer_wrote: AtomicBool,
    // New: snapshot of `writer_wrote` taken the instant the drainer's spin
    // loop observes `active == 0` and returns — this is what "drain()
    // returned" actually means for the interleaving contract, as opposed
    // to "the writer thread has fully finished" (which `join()` alone
    // already guarantees regardless of the drain protocol).
    snapshot_at_drain_return: AtomicBool,
}

fn run_drainer(m: &Model) {
    m.flag.store(true, Ordering::SeqCst);
    while m.active.load(Ordering::SeqCst) != 0 {
        thread::yield_now();
    }
    // drain() has returned — sample writer_wrote RIGHT HERE, at the
    // instant of return, not after the caller later joins the writer.
    m.snapshot_at_drain_return
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
                m.snapshot_at_drain_return.load(Ordering::SeqCst),
                "drain returned before the fast-path writer's write landed — \
                 an interleaving hole in the drain contract"
            );
        }
    });
}
```

Adjust field/variable names as you see fit, but the SHAPE must be: sample
inside the drainer at return time, assert against that sample after join
(join is still needed to obtain `took_fast`, which is legitimately only
known once the writer thread returns its bool — that part of the
original test was fine).

**Verify your fix actually catches the bug the old test missed:**
reproduce the "delete the spin loop" sabotage described above AGAINST
YOUR NEW test and confirm it now goes RED (loom must find at least one
interleaving where the drainer's `flag.store` runs, the drainer
immediately falls through to the snapshot line with `active` possibly
still `1`, snapshotting `writer_wrote == false`, while the writer
independently completes and returns `took_fast == true`). Then restore
the spin loop and confirm GREEN. Report both results in the commit
message — this IS this task's own sabotage-then-restore proof; the
"bug" being sabotaged is the ORIGINAL vacuous-test defect, and "restore"
means putting the spin loop back, not reverting your fix.

## Definition of done

- `loom_model`'s test snapshots `writer_wrote` at drain-return time inside
  `run_drainer`, not after `writer.join()`.
- Reproduced the vacuity bug against the OLD test (spin loop deleted →
  still passes) and against the NEW test (spin loop deleted → now fails)
  — both results reported in the commit message.
- `cargo nextest run -p shamir-engine --features loom --lib -E
  'test(loom_model)' --nocapture` passes with the spin loop restored.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-engine --full` green.
- Do not touch the real (non-`cfg(loom)`) `WriterDrainBarrier`
  implementation — this task is test-only, fixing the model's assertion,
  not the production code it models.

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
