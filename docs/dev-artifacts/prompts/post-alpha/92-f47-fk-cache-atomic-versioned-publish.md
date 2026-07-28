# Brief for F-47 (#858, P0) — atomic versioned publish for the reverse-FK cache

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

The 2026-07-28 readonly review
(`docs/dev-artifacts/research/2026-07-28-new-wave-readonly-review.md`, §3
P0-2) found that F-36's "generation-safe" reverse-FK cache
(`crates/shamir-engine/src/repo/fk_reverse_cache.rs`) still has a real
stale-publish window — and the module's OWN doc comment already names it
as a "Residual window (documented, not closed)" at lines 203-215. **Read
the whole file in full first** (it's short, ~407 lines) — the doc
comments on `FkReverseCache` (:116-135), `invalidate` (:152-172), and
`get_or_build_by_parent` (:174-266) already explain the exact mechanism
and its gap precisely; do not skip them.

**The gap, restated from the code's own documentation:**

`generation: AtomicU64` (:125) and `state: ArcSwap<Option<CacheState>>`
(:117) are TWO SEPARATE atomics. `invalidate` (:161-172) bumps
`generation` first, then clears `state`. `get_or_build_by_parent`
(:216-266) captures `generation` before its scan (:252), runs the scan
(an `.await`, :254), then re-reads `generation` (:261) and — if
unchanged — calls `populate` (:262), which does an unconditional
`ArcSwap::store` (:358-361) with no re-check.

The interleaving that slips through (exactly as the module's own doc
already states at :205-215, and the review's §3 P0-2 restates): a
concurrent `invalidate`'s `fetch_add` can land AFTER the builder's
post-scan generation re-read but its `state.store(None)` can land BEFORE
the builder's own `populate`'s `store(Some(...))` — so the builder's
stale (pre-invalidate) result overwrites the just-cleared state, and
nothing re-checks this. The doc calls this "bounded by the next
invalidate", but if no further DDL ever runs against that repo, the stale
snapshot is never corrected. Since this cache answers `is_fk_child`,
`is_fk_parent_with_delete_action`, `is_fk_parent_with_update_action`
(consulted by `query_runner.rs`'s `require_footprint_if_fk_child` /
`implicit_tx_isolation_for_fk_parent`, and by `fk_actions.rs`/
`fk_on_update.rs`'s discovery functions), a stale publish can silently
re-permit a dangling FK reference or run a stale reverse action.

## What to do

### 1. Settle the design: merge generation + state into ONE versioned snapshot

The fix must make the generation-check-then-publish a SINGLE atomic
operation, not two. Read `arc_swap` 1.7's actual API (this workspace pins
`arc-swap = "1.7"` — check `crates/shamir-engine/Cargo.toml:53` and the
crate's docs.rs page for the exact method signatures available in this
version, do not assume from memory) for a compare-and-swap-style
primitive keyed on object identity (e.g. `ArcSwap::compare_and_swap`,
which CAS-replaces only if the currently-stored `Arc` pointer matches a
given expected `Arc`/guard) or `ArcSwap::rcu`/`rcu_with_old` (a retry-loop
RCU helper that re-applies a closure against the LATEST value until its
own store wins uncontested).

Proposed shape (adjust based on what the actual `arc_swap` 1.7 API
supports once you've read it):

- Replace the two separate fields (`generation: AtomicU64`,
  `state: ArcSwap<Option<CacheState>>`) with ONE
  `state: ArcSwap<VersionedState>` where
  `struct VersionedState { generation: u64, cache: Option<CacheState> }`.
- `invalidate` becomes a single `ArcSwap::store` (or `rcu`) that
  publishes a NEW `VersionedState { generation: old.generation + 1, cache: None }`
  in one atomic operation — no separate "bump generation, then clear
  state" two-step.
- `get_or_build_by_parent`'s publish path: capture the CURRENT
  `Arc<VersionedState>` (not just the `u64` generation) before the scan,
  run the scan, then attempt to publish via a compare-and-swap AGAINST
  THAT EXACT CAPTURED ARC (pointer identity, not just generation-number
  equality) — either via `ArcSwap::compare_and_swap` if it exists in this
  version, or via `ArcSwap::rcu`'s closure form which is inherently
  retry-safe (the closure re-runs against whatever the CURRENT value is
  if another writer won the race, and you decide inside the closure
  whether to keep publishing your scanned result or bail out and let the
  caller re-scan against the new state).
- If the CAS/rcu attempt loses (someone else — an invalidate OR another
  builder — changed the state since capture), the builder must NOT
  publish its (now possibly stale) scan result; it should re-check
  whether a re-scan is needed (mirroring today's `loop` at :251-265) and
  retry, exactly as today's generation-mismatch retry does, but now with
  a REAL single-atomic guarantee instead of the two-atomic race.
- Preserve `build_lock`'s existing single-flight behavior (still needed
  to collapse the concurrent-miss stampede so N concurrent misses run
  ONE scan, not N) — this fix is about publish atomicity, not about
  removing single-flighting.

State your reasoning for whichever specific `arc_swap` primitive you end
up using, and why it genuinely closes the window (not just narrows it
further) — if `arc_swap` 1.7 turns out not to expose a suitable
compare-and-swap/rcu primitive for this shape, say so explicitly and
propose the alternative (e.g. serializing invalidate through the SAME
`build_lock` the build path already holds — investigate whether that's
sufficient given invalidate is rare and build_lock is already the
serialization point for the build side; every `invalidate` call site
would need converting from a bare fn to something that can take an async
lock, check whether `invalidate`'s current callers — `RepoInstance::add_table`/
`remove_table`, `ShamirDb::compile_table_schema` — can tolerate that).

### 2. Adversarial red test FIRST

Write a deterministic test proving the CURRENT (unfixed) code publishes a
stale snapshot in the exact interleaving the doc/review describe: a
build's generation-capture happens, then (via a test-only pause seam
mirroring this repo's established style — see
`fk_reverse_cache_race_tests.rs`'s existing counter+`Notify` pause/resume
handshake, and `commit.rs`'s new `TEST_POST_VALIDATE_PRE_PUBLISH_HOOK`
from the F-46 task landed just before this one, commit `57382bab`, for
the most recent example of this pattern in this exact area of the
codebase) a concurrent `invalidate()` fires and completes BEFORE the
builder's `populate()` call — proving the builder's `store` overwrites
the fresh `None` with its stale result. Confirm this test FAILS (or,
more precisely, observably demonstrates the stale overwrite — the cache
answers a query with stale data after a bump-and-clear invalidate) on
the current code before applying the fix.

### 3. Apply the fix, make the red test pass

Verify: warm-cache fast path is unaffected (still a single `ArcSwap::load`,
no lock, no behavior change for the overwhelming common case); concurrent
misses still collapse to one scan (single-flight preserved); a genuine
generation-mismatch/CAS-loss during a slow scan retries and rebuilds
against the NEW state rather than publishing stale data — no scenario
publishes a snapshot older than the most recent completed `invalidate`.

### 4. Update documentation

The module's own doc comment (:203-215, "Residual window (documented, not
closed)") describes the window this task closes. Once genuinely fixed,
rewrite that section to state the window is CLOSED and describe the new
mechanism (briefly — this is doc-in-code, keep it proportionate to the
existing style, don't bloat it). Do NOT touch
`docs/guide-docs/KNOWN_LIMITATIONS.md` in this task — that document's FK
entry doesn't currently reference this specific F-36 residual by name in
a way that needs updating for this fix; if you find it does after
reading the current entry, note it in your summary but leave the actual
edit to F-51 (the tracked truthfulness-sweep task) unless it's a
one-line, obviously-safe correction directly caused by this fix.

## Constraints

- Do not touch `FkReverseCache`'s public API shape
  (`get_or_build_by_parent`, `is_fk_parent_with_delete_action`,
  `is_fk_parent_with_update_action`, `is_fk_child`, `invalidate`) —
  callers in `query_runner.rs`/`fk_actions.rs`/`fk_on_update.rs` must be
  unaffected. Internal storage representation is fair game.
- Do not touch `FkReverseCache`'s single-flight `build_lock` semantics
  beyond what's needed for the versioned-publish fix — don't remove
  single-flighting to "simplify" the fix.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- fk_reverse_cache
./scripts/test.sh -p shamir-engine -- fk
./scripts/test.sh -p shamir-engine --full
```

When done, give your final summary as plain text: which `arc_swap`
primitive you settled on and why (with the actual API you found in
1.7's docs), the red test's proof (what it demonstrated on the unfixed
code), the exact fix applied, what happens on a CAS-loss/retry path, full
test run output, and confirmation fmt/clippy are clean.
