# F3+F7 — score HNSW candidates inside the read closure instead of cloning `Vec<u8>`/`Vec<f32>` per candidate

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Third item of "Этап 8 — Performance" (post-blocker, не гейт релиза;
`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 07 (`docs/dev-artifacts/research/2026-07-17-release-audit/
07-performance-optimizations.md`), findings **F3** (severity High) and **F7**
(severity Medium) — read that report's F3 section (lines 116-134) and F7
section (lines 209-226) in full first.

**IMPORTANT — the report's code snippets are STALE, read the ACTUAL current
code first.** Report 07 was written citing `read_async`/`vectors_u8.read_async(...)`.
Since that report was written, this same release-audit campaign's Этап 2
(task 2b/H3, commit `dcfaf825`) converted every one of these call sites from
`read_async`/`iter_async` to `read_sync`/`iter_sync` (a deadlock fix,
unrelated to this perf task — do NOT touch the async→sync choice, it must
stay `_sync`). **Read
`crates/shamir-index/src/vector/hnsw_adapter.rs` lines 1783-2065 yourself
before writing any code** — the actual current shape is `read_sync`/
`iter_sync` throughout, and every site already carries a `DEADLOCK FIX
(#589 class)` comment explaining why it must stay synchronous. Your fix
must preserve every one of those comments and the sync/async choice exactly
— only the **clone-vs-score-inline** behavior changes.

## The four sites (verified against current code, not the report's stale line numbers)

1. **`search_quantized_bruteforce`** (`hnsw_adapter.rs:1783-1823`). Currently
   TWO passes: pass 1 (`vectors_u8.iter_sync`, lines 1793-1796) clones EVERY
   stored `Vec<u8>` code into an owned `Vec<(usize, Vec<u8>)> pairs` (O(N·dim)
   bytes + N `Vec` allocs, unconditionally, before any deleted/rid filtering);
   pass 2 (lines 1801-1819) iterates `pairs`, filters `deleted`/`rid_map`,
   and scores. **Fix**: keep the two-pass structure (do NOT merge the
   `deleted`/`rid_map` checks into the `iter_sync` closure — that would nest
   `self.deleted.contains_sync`/`self.rid_map.read_sync` calls INSIDE
   `self.vectors_u8.iter_sync`'s callback, a new cross-map nesting pattern
   this codebase's established `#589`-class discipline has never needed
   before; keeping the existing two-pass shape avoids introducing that
   untested nesting). Instead: build `RescoreCtx` (currently built AFTER pass
   1, at line 1800 — move it BEFORE pass 1) and score INSIDE the `iter_sync`
   closure, collecting `Vec<(usize, f32)>` (12 bytes/entry) instead of
   `Vec<(usize, Vec<u8>)>` (dim bytes/entry). Pass 2 then filters
   `deleted`/`rid_map` over the much smaller `(usize, f32)` pairs exactly as
   today, using the pre-computed score instead of re-deriving it from cloned
   codes.
2. **`search_quantized_graph`** (`hnsw_adapter.rs:1831-1882`), line 1867:
   `self.vectors_u8.read_sync(&n.d_id, |_, c| c.clone())` then later (line
   1874) `ctx.score(&codes)` — clones a `Vec<u8>` of `dim` bytes for EACH of
   the overscan (`= 16k+64`, up to `MAX_TOPK`-clamped) candidates, only to
   score and drop it immediately. **Fix**: `self.vectors_u8.read_sync(&n.d_id,
   |_, c| ctx.score(c))` — compute the `f32` score directly inside the
   closure (`RescoreCtx::score` takes `&[u8]`, confirmed at
   `quantized_dist.rs:333` — synchronous, no `'static`/ownership need). This
   is the SAME single `read_sync` call already in the loop, just returning
   `f32` instead of `Vec<u8>` — zero change to the surrounding
   `deleted.contains_sync` / `rid_map.read_sync` sequence, zero new nesting.
3. **`search_cofilter_quantized`** (`hnsw_adapter.rs:1890-1942`), line 1927:
   identical pattern to #2 (`self.vectors_u8.read_sync(&n.d_id, |_, c|
   c.clone())` then `ctx.score(&codes)` at line 1934). Same fix, same
   zero-structural-risk justification.
4. **`search_prefilter`** (`hnsw_adapter.rs:1981-2065`), TWO branches:
   - Quantized (line 2044): `self.vectors_u8.read_sync(&internal, |_, c|
     c.clone())` then `ctx.score(&codes)` (line 2046) → fix to
     `self.vectors_u8.read_sync(&internal, |_, c| ctx.score(c))`.
   - Unquantized (line 2054): `self.vectors.read_sync(&internal, |_, v|
     v.clone())` then `dist.eval(query, &v)` (line 2056) → fix to
     `self.vectors.read_sync(&internal, |_, v| dist.eval(query, v))`
     (`ShamirDist::eval` takes `&[f32], &[f32]`, confirmed at
     `hnsw_adapter.rs:124` — synchronous, borrow-based).

## Related, OPTIONAL, judgment call (do NOT force this one)

`search_cofilter_quantized` line 1909: `let allow = Arc::new(allow_set.clone());`
— clones the whole allow-set once per co-filter query before wrapping in
`Arc` (needed for `spawn_blocking`'s `'static` bound). Report 07 suggests an
`Arc<TFxSet>` built once by the CALLER would remove this clone entirely.
Investigate the call chain into `search_cofilter_quantized` (grep for its
callers) to see how deep an `Arc`-ification would need to go — if it's a
clean, low-risk, localized change (e.g. the caller already owns/could own an
`Arc` without touching unrelated code), do it as a bonus fix in the same
commit and note it in your summary. If it requires touching several
call-chain signatures or the allow-set's construction site is far upstream,
leave it and explain why in your summary — this is explicitly a "nice to
have if cheap" item, not a mandatory part of this task (mirrors task 8a's
optional `$expr` allocation fix precedent — do it only if genuinely
low-risk).

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-index --full` green — run it TWICE (this
  session's established discipline: a transient full-suite flake unrelated
  to your diff should reproduce as a PASS on a second run; if the SAME test
  fails twice, investigate as a real regression). Pay particular attention
  to any existing HNSW/quantized-search correctness tests
  (`crates/shamir-index/src/vector/tests/`) — this is a pure perf fix, the
  actual scored results (which RIDs, in what order, with what distances)
  must be BYTE-IDENTICAL to before your change. If no existing test asserts
  exact scored output for at least one of the four functions touched, add
  one narrow regression test that does (build a small index, run the
  search, assert the exact `(RecordId, f32)` pairs returned) — this is the
  test that would catch a mistake like scoring the wrong candidate or
  swapping an argument order in the closure conversion.
- `cargo fmt -p shamir-index -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean (full
  workspace).
- Report literal command output for all of the above.
- For EACH of the four sites, confirm in your summary you preserved the
  exact `_sync` (not `_async`) method choice and the exact surrounding
  `DEADLOCK FIX (#589 class)` comments — do not delete or reword them
  (adjust ONLY if a comment specifically describes the clone behavior you
  removed, and even then keep the deadlock-safety explanation intact,
  just prune the now-stale clone-specific wording).

## Out of scope

- Do NOT touch F4 (SQ8 Cosine SIMD/norm-hoist) — that is the next task in
  this sequence (8d), a separate, higher-risk change to the same general
  vector-scoring area.
- Do NOT touch F5 (ForEach) or any Этап 1-7 artifact.
- Do NOT change any `_sync`/`_async` method choice anywhere in this file —
  that is settled by the #589/H3 deadlock-fix campaign (Этап 2, already
  completed) and is NOT part of this task's scope.
- Do NOT restructure `search_quantized_bruteforce`'s two-pass shape into a
  single pass (see site #1's fix direction above — this is a deliberate
  choice to avoid introducing untested cross-map nesting inside an
  `iter_sync` closure, not an oversight).
