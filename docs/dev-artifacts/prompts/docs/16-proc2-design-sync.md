# Brief: bring the `shamir-lang` design text and ADR D4 in line with the implementation

## Why

`docs/dev-artifacts/design/shamir-lang-design.md` and decision D4 of
`docs/dev-artifacts/design/wasm-functions-abi-upload-compiler-adr.md` were written before
milestones 7a, 7b and 8 and the follow-up work landed. Some statements are now stale
(for example "add a scoped Rust guard rejecting `std::process`": the guard is a
source-scan test, `crates/wasm/shamir-lang/src/api/tests/purity_tests.rs` plus
`purity_scanner.rs`), and some verified facts are missing. Documentation must say what
the code does, and mark honestly what is still design-only.

## What

Read the implementation first (it is the source of truth), then edit only these two
documents:

1. `shamir-lang-design.md`
   * Section 6 purity decision: replace the "add a scoped Rust guard" wording with what
     exists: the token-based source scan (strings, comments and raw strings are skipped)
     that rejects `std`/`core`/`alloc` aliases, `extern crate`, the forbidden modules
     (`process`, `fs`, `net`, `env`, `thread`) in any import spelling, and the
     `env!`/`option_env!` macros, plus the dependency pin test. Read
     `purity_scanner.rs` and `purity_tests.rs` for the exact rules; do not claim more
     than the tests prove (a rust-guard rule does not exist; say so).
   * Sections 3/4 (procedures, database stage, typed builder): align with the
     implemented intrinsics (`db_get`, `db_insert`, `db_query`, `batch`, `batch_read`,
     `batch_insert`, `request`, `db_execute`, `response_rows`, `response_inserted`):
     signatures, contextual result types, the `T010` diagnostic, the guest keep-order
     flag around `db_execute` (queries run in the order the procedure added them; see
     the host test `queries_keep_the_insertion_order_of_the_builder_calls`), and the DTO
     oracle tests that compare the guest request with `shamir_query_builder::batch::Batch`
     (`crates/wasm/shamir-wasm-host/src/tests/runtime/lang/batch_tests.rs`).
   * Section 7/9 and "Verified against the code": record the measured facts from
     `docs/dev-artifacts/perf/wasm-scale-second-stage-2026-11-09.md` (link it, copy only
     headline numbers: every source compiles under 10 ms, embedded runtime floor about
     22 KB, output cap 64 KiB error `B001`, tier-up break-even, hot_calls = 64 kept) and
     the security evidence: end-to-end `Security::Definer`/`Invoker` tests in
     `crates/engine/shamir-db/tests/functions/gateway/shamir_lang_definer_e2e.rs` (if the
     file is not present in your tree yet, do not mention it).
   * "Not verified / design-only": move to "verified" what is now covered by tests,
     keep honest what is not (`call`, result references `$query`/`after`, update/delete
     in the builder are design-only: task PROC-2c).
2. ADR D4 in `wasm-functions-abi-upload-compiler-adr.md`: align the decision text and
   its status with the same facts (in-process compiler crate, purity guard as a test,
   milestone status). Change only statements that are now false or incomplete; keep the
   ADR's structure.

Every claim you add must be traceable to a file you read; write the path next to it
in your final message (a short list claim -> file) so the orchestrator can spot-check.

## Observable acceptance

1. No stale statement left about the purity guard, milestone status, or the
   `queries` ordering; no new claim without a source.
2. Only the two documents (and nothing else) are modified. Docs are not subject to
   the 1000-line layout limit, but keep each section focused.
3. No private machine paths, no mention of how or where CI runs, no version change.
4. `python -I scripts/ci/check_layout.py` passes.

## Non-goals

No code change, no new design (the `call`/references addendum is a separate task), no
renumbering of sections.

## Constraints

Documentation task: no builds or tests. The only commands allowed are read-only git,
reading files, and `python -I scripts/ci/check_layout.py`. No cargo, no nextest, no npm.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.
