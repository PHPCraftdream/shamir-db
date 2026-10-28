# P1 stage F10b — p1-index-fts-topk

## Goal

Split from F10 after four rounds. Fix cards RV-INDEX-DC5C642E and RV-INDEX-8F977BF5 (read the whole `### RV-INDEX-...` sections of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P1.md`).

This worktree is based on branch fix/p1-index-fts-sorted (PR #336, P1 F10, not yet merged). Its changes are a given: build on them, do not revert or rework them.

## Cards

### RV-INDEX-8F977BF5 — Per-row deep-clone of the whole index-definition set in every write planner

**Verdict:** `confirmed-open`; **scoped risk:** `medium`; **тип:** structural cost — без измерений. **Модули:** `shamir-index`.
### RV-INDEX-DC5C642E — FTS lookup materializes every matching posting list and sorts all results — no top-k bound

**Verdict:** `confirmed-open`; **scoped risk:** `medium`; **тип:** structural cost — без измерений. **Модули:** `shamir-index`.

## Binding scope

1. **DC5C642E.** The FTS backend contract (`crates/shamir-index/src/backends/backend.rs`, `fts_ranked_backend.rs`) gets a `limit`. Ranked lookup keeps a bounded min-heap of size k instead of materializing and sorting every scored candidate; the query path with a limit routes through it, the unlimited path keeps its current behaviour. Regressions: the collector never holds more than k candidates (test hook or counted `cfg(test)` seam), and top-k equals the first k of the full sort, including ties (define and document the tie order).
2. **8F977BF5.** The production fix (borrowing the index-definition snapshot instead of deep-cloning per row) may already be on this branch — confirm from code. Add a regression on the SINGULAR planner path that observes the snapshot is borrowed (`Arc::ptr_eq` or strong count), so reverting to a deep clone fails it. Related tests: `base_index/tests/info/index_info_tests.rs`, sorted `basic_tests.rs`.

## Requirements

1. Before editing, confirm each mechanism in current code (read callers and guards). If a card is already fixed or wrong at HEAD, do not change code for it: say so with file:line evidence in the final message.
2. Fix at the narrowest layer that owns the invariant. No unrelated refactors, renames or comment edits.
3. TDD for runtime cards: add a regression test whose name contains the card's hex suffix in lower case (for example `rv_storage_98f6650d_...`). The test must fail on the unfixed code and pass on the fix — reason about that from the code, you cannot run it. Tests live in the module's `tests/` folder (`tests/mod.rs` is a manifest only; no inline `#[cfg(test)] mod tests {}`) and are registered there. Prefer deterministic constructions (injected failures, barriers, paused clocks) over sleeps and timing. Docs-only, layout-only and comment cards need no test: say so in the final message.
4. House rules: imports at the top of the file; `mod.rs` re-exports only; one primary export per file; at most 7 entries per folder and 1000 lines per source file for new/changed code (check_layout is the gate); Fx hashers; no `std::sync::Mutex`/`RwLock` on hot paths; `Result` instead of panics; no private machine paths in committed files; terse comments; queries only through the query builder.
5. Do not bump crate or dependency versions, do not add dependencies, do not touch CI workflows (except where a card says so).
6. Run only the allowed local gates from the policy (clippy, fmt, layout) on the touched crates until clean. If a changed public signature affects dependent crates, run clippy on those too.
7. Other agents work concurrently on other crates; stay inside the files your cards name plus their tests.

## Final message (mandatory)

1. Per card: fixed / already-fixed / refuted / docs-only, with the changed file:line and the regression test name.
2. Why each test fails on the old code (one sentence each).
3. Real exit codes of clippy, fmt --check and check_layout. Anything you could not verify.
