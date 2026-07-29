# Brief for F-56 (#882, P0) — WriterDrainBarrier memory-ordering proof + loom + wire into all DDL

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. An independent readonly review of
snapshot `e145b1d3` (`docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md`,
section P0-2) found that `WriterDrainBarrier`'s memory-ordering argument is
WRONG. The orchestrator independently re-derived the bug from first
principles (not just trusting the review) — see the proof below.

### The primitive

`crates/shamir-engine/src/table/writer_drain_barrier.rs` — a fast-path
writer calls `enter_writer()` (a `Relaxed fetch_add` on an `active:
AtomicUsize` counter, RAII-guarded) BEFORE reading a barrier flag
(`TableManager::needs_write_barrier()`, which `Acquire`-loads one of two
separate `AtomicBool`s: `index2_create_barrier` /
`schema_activation_barrier`). A DDL "drainer" raises the flag
(`set_schema_activation_barrier`, an `Ordering::Release` store) while
holding `unique_write_lock`, then calls `drain()` (an `Acquire`-load spin
loop on `active`) to wait for any writer that read the flag as `false`
before it went up.

Four call sites use this exact sequence today:
`table_manager_crud.rs` (`insert`, `insert_many_returning_version`,
`delete_returning_version`, `set` — all at the
`enter_writer()`-then-`needs_write_barrier()` pattern), plus the tx-commit
prelock (`tx::pre_commit::pre_commit_prelock` via
`TableManager::enter_writer_drain`).

### The (wrong) proof in the doc comment

`writer_drain_barrier.rs:32-44` claims:

> the flag's coherence ordering carry the happens-before: if a writer
> reads `false` ... the drainer's drain ... observes the writer's
> increment via the coherence chain (`writer.fetch_add` sb→
> `flag.load == false` coherence-ordered-before→ `flag.store == true` sb→
> `drain.load`)

This is FALSE under the Rust/C++ memory model. `flag` and `active` are
**two independent atomic objects**. A `synchronizes-with` edge (the thing
that actually creates cross-thread happens-before) requires an `Acquire`
load to observe the value written by a **specific** `Release` store *on
the same atomic*. Here the writer's `flag.load` observes `false` — i.e.
it does NOT read the value the drainer's `flag.store(true)` wrote. Reading
an *earlier* value in one atomic's modification order does not
synchronize-with a *later* store on that same atomic, and critically it
creates **no cross-thread ordering relationship on `active` at all** — the
two atomics (`flag`, `active`) have entirely separate modification orders
under `Relaxed`/`Acquire`/`Release`. So the drainer's `active.load(Acquire)`
has no guarantee of observing the writer's `active.fetch_add(Relaxed)`,
even though in real time the writer incremented first. A legal weak-memory
outcome: the drainer's `drain()` sees `active == 0` and returns
immediately while the writer is still mid-flight through its
validate→write→index sequence — precisely the race this primitive exists
to prevent.

### Why SeqCst (not just Release/Acquire) actually closes it — worked proof

Making **all four operations** `SeqCst` — writer's `active.fetch_add`,
writer's `flag.load`, drainer's `flag.store`, drainer's `active.load` —
closes the race, and here is why (verify this reasoning yourself before
implementing; do not take it on faith either):

1. `SeqCst` operations across all threads participate in one single total
   order `S`, and per the C++/Rust memory model, if operation A is
   sequenced-before operation B (same thread, program order), A precedes B
   in `S`.
2. Writer: `active.fetch_add` is sequenced-before `flag.load` (same
   thread) ⟹ `fetch_add ≺ flag.load(false)` in `S`.
3. Drainer: `flag.store(true)` is sequenced-before `active.load` (same
   thread) ⟹ `flag.store(true) ≺ active.load` in `S`.
4. `S` must be consistent with each atomic's own modification order, and a
   load's position in `S` must be consistent with which store it observed.
   The writer's `flag.load` observed `false` (the OLD value), and the
   drainer's `flag.store` wrote `true` (the NEW value) — there is only one
   `false→true` transition on this flag in the scenario that matters (DDL
   raises it once). Because the load read the pre-transition value, it
   must precede the store in `S`: `flag.load(false) ≺ flag.store(true)`.
5. Chaining 2 → 4 → 3: `fetch_add ≺ flag.load(false) ≺ flag.store(true) ≺
   active.load`, all within the single total order `S`.
6. `fetch_add` and `active.load` are both `SeqCst` operations on the SAME
   atomic (`active`); `S` is required to respect `active`'s own
   modification order. Since `fetch_add ≺ active.load` in `S`, the
   drainer's `active.load` is guaranteed to observe a value at least as
   recent as the writer's increment (it cannot observe a "before the
   increment" state).

This is the standard "SeqCst fence via a third total order" argument —
it works precisely because `SeqCst` (unlike `Acquire`/`Release`) gives a
single global order across *different* atomics, which is exactly what
this primitive's cross-atomic (`flag` + `active`) dependency needs.
`Release`/`Acquire` alone can never provide this — no per-pair strengthening
of `Relaxed` to `Release` fixes it, because the bug is not about ONE
atomic's ordering, it's about the ABSENCE of any relationship between TWO
different atomics.

## What to do

1. **Verify the proof above yourself** (read `crates/shamir-engine/src/table/writer_drain_barrier.rs`
   and `table_manager.rs`'s `needs_write_barrier`/`set_schema_activation_barrier`/
   `drain_writers`/`enter_writer_drain` in full) — do not implement blind.
   If you find a flaw in the reasoning above, or believe a different
   protocol is clearly superior for this codebase (a seqlock/epoch
   handshake with mandatory writer re-check, or a gate object), you may
   choose it INSTEAD of SeqCst — but whichever you choose, the final code
   must carry a doc comment with an equally rigorous step-by-step proof
   (not hand-wavy language like "the coherence chain carries
   happens-before" with no worked argument), and the OLD wrong proof
   comment must be deleted, not left alongside the new one.

2. **Implement the chosen protocol.** If SeqCst (the recommended default):
   change `enter_writer`'s `fetch_add` from `Ordering::Relaxed` to
   `Ordering::SeqCst`, `drain`'s `active.load` from `Ordering::Acquire` to
   `Ordering::SeqCst`, `set_schema_activation_barrier`'s store from
   `Ordering::Release` to `Ordering::SeqCst`, and `needs_write_barrier`'s
   two flag loads from `Ordering::Acquire` to `Ordering::SeqCst`. Also
   check `table_manager_index_mgmt.rs` for `index2_create_barrier`'s own
   store site (search for where it's set to `true`/`false`) and align its
   ordering the same way — the fix must be applied everywhere this
   cross-atomic dependency exists, not just one call site.
   `WriterDrainGuard::drop`'s `fetch_sub` should also move to `SeqCst` for
   symmetry (a writer's exit must not race the drainer's read either).

3. **Loom model.** Add a `loom`-based test proving the chosen protocol is
   sound and — as a sanity check — that the ORIGINAL Relaxed/Acquire/Release
   version is NOT sound (loom should find the bad interleaving on the old
   code, confirming the model is actually exercising the race, not just
   passing vacuously). Loom is not currently a workspace dependency —
   add it as a `dev-dependency` scoped to `shamir-engine` only, gated the
   standard way (`#[cfg(loom)]` module, run via a documented
   `RUSTFLAGS="--cfg loom" cargo test ...` invocation — follow loom's own
   documented harness pattern, e.g. a small abstracted model of just the
   `active` counter + `flag` interaction rather than trying to loom-check
   the entire `TableManager`, which would explode the state space).
   Document the exact command to run this model in a comment at the top
   of the loom test module, since it does NOT run under the normal
   `./scripts/test.sh` (loom tests require the `--cfg loom` rustflag and
   are typically excluded from the default test run due to their cost —
   confirm this doesn't silently get pulled into the default `cargo tl`
   run in a way that slows every future test run down).

   **If a full loom model proves disproportionately large for this task**
   (e.g. state-space explosion even after abstracting to just the two
   atomics), you may substitute a best-effort interleaving-forcing test
   using the same `tokio::sync::Notify` pause/resume pattern this file's
   existing tests already use (see `drain_waits_until_all_writers_exit`
   for the style) — but you MUST say so explicitly in your final summary,
   not silently skip loom and claim full coverage.

4. **Wire the corrected drain into every DDL path that needs it.** This is
   the "wire into all DDL" half of the task. Confirm (via `rg
   "drain_writers"`) which DDL paths currently call `drain_writers()` and
   which don't:
   - `admin_schema.rs`'s schema activation — already calls it (confirm).
   - `create_index_v2` (`table_manager_index_mgmt.rs:29-360`) — per the
     review, this does NOT call `drain_writers()` today despite raising
     `index2_create_barrier` and taking `unique_write_lock`. This is a
     real, separate gap from the memory-ordering bug: even with a
     corrected protocol, a missing `drain_writers()` call means index2
     backfill can still race an in-flight writer. Add the call (under the
     lock, after raising the barrier, before the backfill snapshot) — this
     is explicitly the wiring F-50's own docs already promised
     ("F-50 will wire this SAME primitive into `create_index_v2`'s
     residual with no new design work" — the file's own comment says so;
     confirm this was never actually done, then do it).
   - Do NOT attempt to wire this into `create_index` (regular hash),
     `create_unique_index`, or the sorted-index path in this task — those
     are F-57's (#883) scope (a UNIFIED lifecycle across all four index
     kinds), not F-56's. F-56 fixes the PRIMITIVE and closes the ONE
     already-promised-but-missing `create_index_v2` wiring gap; F-57
     builds the shared guard on top of this corrected foundation.

5. **Add a regression test proving the race is closed** — reuse this
   file's existing `tokio::sync::Notify`-based interleaving style to
   deterministically force the specific bad interleaving (writer bumps
   counter, writer about to read flag, drainer sets flag then drains) and
   confirm the drainer correctly waits.

## What NOT to do

- Do NOT touch `create_index` (regular), `create_unique_index`, or
  `table_manager_sorted_index.rs` — those are F-57's scope.
- Do NOT simply swap `Relaxed` → `Release` on the counter alone and call
  it fixed — the proof above shows why that specific half-measure doesn't
  work (Release without a matching synchronizes-with Acquire on the SAME
  operation creates no cross-thread edge).
- Do NOT let loom become a workspace-wide default-test-run dependency —
  it must be opt-in via `--cfg loom`, invisible to `./scripts/test.sh`'s
  normal invocation.
- Do NOT touch F-55/F-58/F-59/F-60/F-61 (other tasks from the same
  review).

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`) for the
  NON-loom tests; the loom model runs via its own separate documented
  command.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- TDD: the new regression test (step 5) should be written to fail against
  the OLD ordering, confirmed red, then pass after the fix.
- Clean up any scratch/debug files created in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
```

Plus, personally, the loom command you document and a red-then-green
reproduction of the regression test.

When done, give your final summary as plain text: which protocol you
chose and why, the exact ordering changes made (file:line for each), the
loom model's command and what it proved (or the documented fallback if
loom wasn't used), the `create_index_v2` → `drain_writers()` wiring diff,
and confirmation fmt/clippy/tests are clean.
