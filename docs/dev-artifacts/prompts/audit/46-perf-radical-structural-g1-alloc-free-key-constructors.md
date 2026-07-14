Task: G1 — alloc-free hot-path key constructors + sweep residual
`Bytes::copy_from_slice` in cold paths. Task #525 (absorbs former #504 +
#505), steps 3+4 of `docs/dev-artifacts/design/record-key-128-migration-plan.md` (task
#491/#503's structural migration). Step 2 (the `RecordKey` alias cutover
to `KeyBytes`, mechanical, no logic change) landed in commit `83e1def9`.

## Read first

`docs/dev-artifacts/design/record-key-128-migration-plan.md` §4 items 3 and 4 — this
brief summarizes them but the plan doc has the authoritative file list and
reasoning.

Also read the `@fl` review note from task #503's commit (`83e1def9`'s
message, or `git show 83e1def9 --stat`): the reviewer found that some
in-memory overlay/reservation maps (MVCC history, table streaming) stayed
`Bytes`-keyed after the alias cutover, so a `KeyBytes` value built inline
now gets converted BACK to `Bytes` (a real heap allocation —
`Bytes::copy_from_slice` under the hood) on every commit/scan op via
`.into()`. Specific sites already identified: `crates/shamir-engine/src/tx/pre_commit.rs:85`,
`crates/shamir-tx/src/mvcc_store/mvcc_history.rs:361,440,473,571`,
`crates/shamir-engine/src/table/table.rs:378,392`,
`crates/shamir-tx/src/mvcc_store/mod.rs:1502`. This IS the primary target
of this task — confirm each site, and where the map/structure can be
re-keyed to `RecordKey` (`KeyBytes`) directly instead of `Bytes`, do so
(this eliminates the round-trip allocation entirely, not just moves it).
Where re-keying isn't safe/local (e.g. a type shared with something
outside this migration's scope), leave the `.into()` conversion but note
it in your report rather than forcing an unrelated refactor.

## Part (a) — alloc-free hot-path key constructors

Replace `rid.to_bytes()` (which allocates a `Bytes` via
`Bytes::copy_from_slice`, per `crates/shamir-types/src/types/record_id.rs`)
with `RecordKey::from_slice(rid.as_bytes())` (alloc-free for the common
≤23-byte `RecordId` case — 16 bytes inlines) at hot call sites. Per the
plan doc, the known call sites are in:
`crates/shamir-engine/src/table/table_manager_crud.rs`,
`crates/shamir-engine/src/tx/drainer.rs` (around the batch-append path),
`crates/shamir-engine/src/tx/recovery.rs`,
`crates/shamir-engine/src/table/read_temporal.rs`,
`crates/shamir-engine/src/table/read_index_scan.rs`,
`crates/shamir-engine/src/table/table.rs`,
`crates/shamir-engine/src/table/table_manager_streaming.rs`, and the
staging-store writers in `crates/shamir-tx/`. Re-grep for `.to_bytes()` on
a `RecordId`/`InternerKey`-shaped value across these files and the ones the
`@fl` nit identified (§ above) — the exact line numbers may have shifted
since the plan doc was written and since #503's cutover touched many of
these files already.

Do NOT blanket-replace every `.to_bytes()` call in the workspace — only
ones where the result feeds a `RecordKey`/key position (Store trait calls,
staging maps, MVCC lookups). A `.to_bytes()` that produces a value payload,
a wire response, or feeds serde directly is NOT in scope — changing those
would be a real behavior/type change, not this migration.

## Part (b) — sweep residual `Bytes::copy_from_slice` + confirm automatic wins

1. Confirm `InMemoryStore`'s `TreeIndex<RecordKey, ...>` and `CachedStore`'s
   map already benefit automatically from the alias cutover (they should —
   no further change needed there, just confirm by reading the current
   code, don't add anything).
2. Sweep for remaining `Bytes::copy_from_slice(id.as_bytes())` or
   equivalent patterns that construct a KEY (not a value) via the heap-only
   path when `RecordKey::from_slice` would inline instead — the plan doc
   flags `storage_fjall.rs`'s insert path (may already be fixed by #503 —
   confirm, don't duplicate work) and residual `.to_bytes()` in
   `interner_manager.rs` / `record_counter.rs` / meta paths (cold paths —
   fixing them is free/harmless even though they're not hot, per the plan
   doc's own framing, but don't go out of your way hunting for exotic
   cold-path instances beyond what's flagged here).

## Constraints

- This is a representation/construction-site optimization, NOT a logic
  change. If a fix requires touching a type's public shape (e.g. changing
  a struct field's type from `Bytes` to `RecordKey`), that's in scope IF
  it's purely internal (private field, no public API break) — flag
  anything that would change a `pub` signature to the orchestrator instead
  of doing it silently.
- Never derive `Eq`/`Ord`/`Hash` on anything wrapping `KeyBytes`'s
  internals — always delegate to `KeyBytes`'s own impls (same rule as
  #503's brief).

## Performance verification (MANDATORY — this is the actual payoff step of the migration)

Bench before/after using the existing benches: `engine_perf`,
`storage_*_pump` (`storage_fjall_pump`, `storage_cached_pump`,
`storage_membuffer_pump` if present), `posting_cache_hit`. Follow this
repo's `CARGO_TARGET_DIR=<isolated dir>` bench-cache-isolation convention
(POSIX-style path if invoking from bash on Windows — see prior tasks this
session for the exact gotcha with backslash paths). Report honest
before/after numbers — this is exactly the kind of change that can show a
measurable per-op allocation-count/latency win on hot commit/scan paths;
report flat/no-improvement results honestly if that's what you find, never
fabricate.

## Verification (lighter per-task gate — agreed this session)

Per-task gate is `cargo check` + SCOPED test only (NOT a full build/fmt/
clippy/test --full pass — that's deferred to a separate FINAL-GATE task at
the end of the whole remaining series):

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-storage -p shamir-tx -p shamir-index -p shamir-engine -p shamir-db
```

Do not run `cargo fmt --all -- --check` / `cargo clippy --workspace ...` /
`./scripts/test.sh --full` for this task — those are FINAL-GATE's job.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[Implementation] Status: fixed / partially-fixed
  > Part (a): list every hot-path .to_bytes() -> RecordKey::from_slice
    swap made, with file:line
  > Part (b): confirm automatic backend wins + list every residual
    copy_from_slice fix made
  > @fl nit follow-through: which Bytes-keyed maps got re-keyed to
    RecordKey (eliminating the round-trip allocation) vs left as-is
    (with reason)
  > Bench: before/after numbers for engine_perf / storage_*_pump /
    posting_cache_hit
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p ... : pass/fail, exact failure list if any
```
