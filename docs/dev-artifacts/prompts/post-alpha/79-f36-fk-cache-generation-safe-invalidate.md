# Brief for F-36 (#844, P0) — FK reverse cache invalidate-vs-build race can publish a stale snapshot

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

Depends on F-35 (#843, landed, commit `44c4317e`) — both touch
`crates/shamir-engine/src/repo/fk_reverse_cache.rs`, done sequentially to
avoid overlapping diffs. **Read the current file in full first** (it's
short, ~300 lines) — this brief describes the exact race in the code as it
stands right now, not a hypothetical.

A readonly review (`docs/dev-artifacts/research/2026-07-27-new-wave-readonly-review.md`,
finding P0-2) found a real cache-aside invalidate/build race in
`FkReverseCache`:

```rust
pub fn invalidate(&self) {
    self.state.store(std::sync::Arc::new(None));
}

pub async fn get_or_build_by_parent<F, Fut, E>(
    &self,
    parent_table: &str,
    build: F,
) -> Result<Vec<ReverseFkEntry>, E>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Vec<TaggedReverseFkEntry>, E>>,
{
    if let Some(hit) = self.lookup_by_parent(parent_table) {
        return Ok(hit);
    }
    let all_entries = build().await?;   // <-- can race with a concurrent invalidate()
    self.populate(all_entries);          // <-- publishes UNCONDITIONALLY, no generation check
    Ok(self.lookup_by_parent(parent_table).unwrap_or_default())
}
```

**The race** (confirmed by reading the code, matching the review exactly):

1. Query A sees a cache miss (`state` is `None`) and starts the O(tables)
   `build()` scan against the CURRENT (about-to-be-stale) schema.
2. Concurrently, DDL B changes the FK schema (adds/removes an FK, changes
   an action) and calls `invalidate()` — `state.store(None)`.
3. Query A's scan finishes (it started before DDL B, so it reflects the
   OLD schema) and calls `populate(all_entries)` — this ALWAYS succeeds,
   there is no check that the cache is still "the same generation" it was
   when A started.
4. The cache is now warm again, but with A's STALE (pre-DDL-B) data — and
   it stays wrong until the NEXT invalidation, not until the next genuinely
   correct rebuild.

This is not just a performance stampede (two concurrent misses today each
run their own full independent O(tables) scan — the module's own doc
comment claiming "build exactly once" is not actually true under
concurrency). It's a correctness bug: this cache's data directly decides
Serializable-vs-Snapshot isolation upgrades and `require_footprint_for`
child-table footprint widening (see F-28 Step 5 / F-35's work in this same
file). A stale reverse-FK snapshot can miss a newly-added FK and silently
reopen the exact dangling-reference race F-28/F-35 exist to close.

## What to build

### Generation-safe, single-flight rebuild

Recommended shape (implement this unless you find a clearly better
alternative that satisfies the same invariant — state your reasoning
either way in your summary):

1. Add a generation counter, e.g. `generation: std::sync::atomic::AtomicU64`,
   alongside the existing `state: ArcSwap<Option<CacheState>>`.
   `invalidate()` increments it (`fetch_add(1, Ordering::AcqRel)`) in
   addition to storing `None`.
2. Add a single-flight guard, e.g. `build_lock: tokio::sync::Mutex<()>`
   (this project's sanctioned exception for "guard held across `.await`,
   bounded contention" — see the workspace's concurrency ideology in
   `CLAUDE.md`; this is a low-frequency path, not a hot one).
3. Rework `get_or_build_by_parent` as: fast-path lookup outside the lock
   (unchanged, cheap `ArcSwap::load`); on a miss, acquire `build_lock`,
   then loop: re-check for a hit (double-checked locking — someone else
   may have populated while we waited for the lock), otherwise capture the
   CURRENT generation, run `build()`, and only `populate()` if the
   generation is STILL the one captured before the scan started
   (`self.generation.load(...) == gen_at_start`, compared right before the
   `ArcSwap::store` — do the compare-and-store as tightly as possible to
   minimize the window, though a perfectly atomic compare-and-swap across
   two different atomics isn't achievable here; document the residual
   window precisely rather than hand-waving it away). If the generation
   changed (a concurrent `invalidate()` raced the scan), do NOT return a
   stale-or-empty answer for this call — loop and rebuild again, still
   holding `build_lock`, against the NEW generation. Only return once a
   generation-matched populate has actually happened (or the pre-loop
   lookup already hit).
4. This closes BOTH problems the review names: no stale snapshot is ever
   published (generation check), and two concurrent misses no longer run
   two independent scans (the single-flight lock serializes them — the
   second one's post-lock-acquire re-check will find the first one's
   result already warm, in the common case where no invalidate raced it).
5. `F: FnOnce() -> Fut` must become `F: Fn() -> Fut` (callable more than
   once, for the rare retry-after-generation-mismatch loop iteration).
   Check `query_runner.rs`'s two call sites
   (`require_footprint_if_fk_child`/`implicit_tx_isolation_for_fk_parent`)
   — their closures currently `move` capture `resolver`/`repo_name` into a
   single-use `async move` block. Adjust them to be re-callable (e.g. clone
   `repo_name` inside the closure body per call, `resolver` is already a
   `&dyn TableResolver` reference and trivially `Copy`) — this is a small,
   mechanical signature change, not a redesign.

### Secondary (do if cheap while you're already touching this; otherwise
   explicitly defer — it's separately tracked as P2-5 in the review, not
   part of this correctness fix)

`lookup_by_parent` currently clones the whole `Vec<ReverseFkEntry>`
(including every `String` field) on every cache hit. Switching the
`by_parent` index's value type to `Arc<[ReverseFkEntry]>` (or similar) so a
hit only clones an `Arc` would remove that per-hit allocation cost. Only do
this if it falls out naturally from the generation-safety rework above
without meaningfully growing the diff; otherwise leave it for the P2-5
follow-up and say so explicitly in your summary.

## Tests — MANDATORY, in the same commit

Extend or add to
`crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs`
(or a new dedicated test file if the cache-internals test doesn't fit that
file's existing scope/shape — check first and decide, state which you
picked and why) with a deterministic test that reproduces the EXACT race
from this brief:

- **"build paused → invalidate → resume old build → assert old snapshot
  never published"**: pause a build partway through (inject a controllable
  delay/gate the same way `fk_race_closure_tests.rs`'s existing
  `RaceInjectingResolver` pattern injects a concurrent writer — read that
  pattern first and reuse its shape rather than inventing a new
  injection mechanism), call `invalidate()` while the build is paused,
  let the paused build resume and finish, and assert:
  - the cache does NOT end up serving the paused (now-stale) build's data;
  - a subsequent `get_or_build_by_parent` call returns data reflecting the
    schema state AFTER the invalidate (the fresh, correct answer), not
    before it.
- **Two concurrent misses run at most one real scan** (or, if you kept
  the design honest about a narrow unavoidable double-scan window, assert
  whatever the ACTUAL guarantee is — don't assert something the
  implementation doesn't truly provide): a test with two concurrent
  `get_or_build_by_parent` calls on a cold cache, confirming they don't
  each independently and redundantly scan (use a counter in the injected
  `build` closure to prove it's called once, not twice, for the
  non-raced case).
- A plain, non-race regression: `invalidate()` then a single subsequent
  `get_or_build_by_parent` call still returns fresh, correct data (basic
  sanity that the generation-check logic doesn't accidentally make the
  cache permanently stuck cold or permanently stuck stale).

## Constraints

- Do NOT touch F-35's role-flag split (`on_delete`/`on_update`,
  `is_fk_parent_with_delete_action`/`is_fk_parent_with_update_action`) —
  already landed and correct; this task is scoped to the invalidate/build
  race only.
- Do NOT change `FkReverseCache`'s public API shape beyond what's strictly
  needed for the `Fn`-vs-`FnOnce` bound change and (optionally) the
  `Arc<[...]>` hit-type change — no unrelated refactors.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- fk
./scripts/test.sh -p shamir-engine --full
```
