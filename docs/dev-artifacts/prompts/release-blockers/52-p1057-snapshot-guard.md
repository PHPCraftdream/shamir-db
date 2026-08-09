# Brief 52 — #1057: hold a SnapshotGuard for Phase A against MVCC GC

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Context

Slice 1b of the online CREATE INDEX redesign (RFC v2 §2.2/§3/§4, approved
2026-08-09). Phase A's snapshot scan runs for potentially minutes without a
write barrier — nothing today prevents MVCC GC from reclaiming a version the
scan still needs mid-scan. `min_alive()` (`crates/shamir-tx/src/mvcc_store/mvcc_gc.rs:286`)
tracks "the oldest live snapshot, or `last_committed` when no snapshot is
open" — without a registered snapshot, the watermark moves forward and can
collect exactly the version Phase A pinned.

## Reachability — already verified, do not re-investigate

`RepoTxGate::open_snapshot()` (`crates/shamir-tx/src/repo_tx_gate.rs:356`)
returns the RAII `SnapshotGuard` needed. `TableManager` only reaches
`RepoTxGate` through `changefeed: Option<NonTxChangefeed>`
(`crates/shamir-engine/src/table/table_manager.rs:207,280-282`). This was
flagged as a possible gap (mirroring the F-71/#898 watermark-reachability
precedent) but is confirmed NOT a gap in production: the only production
call site attaching `mvcc_store`
(`crates/shamir-engine/src/repo/repo_instance.rs:396-418`) ALWAYS
immediately attaches `changefeed` with the SAME `gate` instance
(`:425`, `Ok(tbl.with_changefeed(Arc::clone(&gate)))` — same `gate` used to
construct the `MvccStore` at `:396`). So any production table with
`mvcc_store` attached also has `changefeed` attached — reachability holds.
Test helpers are a different story: grep confirms every OTHER call site of
`with_mvcc_store` is in `crates/shamir-engine/src/table/tests/*.rs` and does
NOT also call `with_changefeed` — your new tests for this task must set up
BOTH (see the Tests section below for how).

## Task

1. Add an accessor on `TableManager` to acquire the guard, e.g.:
   ```rust
   pub(crate) fn open_index_build_snapshot(&self) -> Option<shamir_tx::SnapshotGuard> {
       self.changefeed.as_ref().map(|cf| cf.gate.open_snapshot())
   }
   ```
   Note `open_snapshot()` returns a `SnapshotGuard` synchronously per its
   signature in `repo_tx_gate.rs` — verify the exact signature (async or
   not) before writing this; adjust to `.await` if needed.
   Name/placement are your call as long as the guard is reachable from
   wherever Phase A will start (slice 1d, #1059, not this task).
2. **Guard version vs. pin version — establish which is authoritative.**
   `SnapshotGuard::version()` (`repo_tx_gate.rs:229`) returns the FINAL
   (highest) registered version. Compare this against what
   `current_committed_version()` returns at the same call site. Decide:
   should Phase A pin at `guard.version()` (the version the guard actually
   protects) or at a separately-read `current_committed_version()` (risking
   a version that ISN'T the one registered)? The RFC's stated preference is
   the guard's own version, since that's the one provably protected from
   GC — implement it that way: acquire the guard FIRST, then use
   `guard.version()` as the pin, not two separate reads that could
   disagree.
3. **Document the early-drop hazard.** If the guard is dropped before Phase
   A's scan completes (panic, cancelled future), the scan must fail loudly,
   not silently read gaps left by GC. Since the guard's Drop just
   unregisters the version (it doesn't itself corrupt anything), the actual
   safety property is: **the scan must hold the guard for its own entire
   lifetime** (i.e., the guard should be owned by the same scope/future that
   drives the scan, not detached). Write a doc comment on the accessor
   stating this invariant explicitly. This task does not need to wire the
   guard into an actual running scan (that's #1059) — just provide the
   primitive and document the contract clearly enough that #1059 can't get
   it wrong.

## Tests (TDD — the anti-GC test must fail without the guard, verify this yourself)

Add to `crates/shamir-tx/src/tests/` (or wherever `TableManager`-level tests
for this live — check `crates/shamir-engine/src/table/tests/` conventions;
this task spans both `shamir-tx` primitives and the `TableManager` accessor,
so put the accessor's test in `shamir-engine`'s table tests, using a
`with_mvcc_store(...).with_changefeed(gate)` setup mirroring
`repo_instance.rs:396-425`'s pattern — check existing test helpers for how
to construct a `RepoTxGate` directly, e.g. `make_gate()` used in
#1056's own new test file
`crates/shamir-tx/src/tests/mvcc_store_tests/snapshot_stream_tests.rs`).

1. **Anti-GC test (write this FIRST, prove it fails without the guard).**
   Acquire the guard, capture its version as the pin. Write many new
   versions past the pin. FORCE a GC pass (find the existing GC-triggering
   test helper/method — grep `mvcc_gc.rs` and its tests for how existing
   tests force a collection pass). Then read the pinned version via
   `snapshot_stream(batch, pin)` (from #1056) → data must still be intact.
   **Before finalizing this test, temporarily comment out the guard
   acquisition and confirm the test FAILS** (data goes missing / read
   errors) — this is what proves the guard is load-bearing, not
   decorative. Put this confirmation in your report; don't just assert it.
2. **Release test.** After the guard drops, confirm `min_alive()` (or
   whatever public/pub(crate) accessor exposes it — check
   `mvcc_gc.rs`/`repo_tx_gate.rs`) returns to tracking the live watermark
   again — it must NOT stay pinned at the old version forever (that would
   permanently wedge GC).

## Boundaries

Do not wire this into `create_index` or any actual backfill path yet — that
is slice 1d (#1059). This task only adds the primitive (the accessor) and
proves it works in isolation.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-tx
./scripts/test.sh -p shamir-engine -- snapshot_guard
```

Report the exact diff, the exact test names, and explicit confirmation that
you verified the anti-GC test fails without the guard (show the failure
output, not just a claim).
