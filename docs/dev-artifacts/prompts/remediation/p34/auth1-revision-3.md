# TOKEN-AUTH1 revision 3 - apply REVIEW-3: compile/lint blockers, shutdown safety, admission DoS, missing tests

Base: `auth1-login-http-server.md`, `auth1-revision-1.md`, `auth1-revision-2.md` (every rule there applies verbatim: NO LOCAL COMPILATION, git-mutation prohibition, layout limits, clippy classes, no machine-local paths). The independent compiler-style review is `docs/dev-artifacts/audits/2026-11-02-token-auth1/REVIEW-3.md` - read ALL of it; its sections 1, 2, 4 (findings S1-S9) and 7 contain the exact fixes.

## Work, in this order

1. **P0 blockers (5 min)**: C1 (`service/tests/begin_tests.rs` two sites: use `fixture.context.pair_key("alice", source().address())`), C2 (`ADMIN_PASSWORD` visibility in `tests/e2e/http_rest/common.rs`), L1 (`service/begin.rs` let_and_return), L2 (`service/audit.rs` len_zero). Exact replacements are in REVIEW-3 sections 1-2.
2. **L3 (15 min)**: extract the audit worker into `async fn run_worker(..)` so rustfmt formats `audit.rs`; run `cargo fmt -p shamir-server`.
3. **S1 + S2 (40 min)**: detached FINISH workers with an in-flight counter and a bounded `shutdown(budget)` (WARN on timeout); independent audit cancellation token plus drop guard so the audit consumer outlives the workers. Record the remaining risk in DESIGN.md.
4. **S4 (60 min)**: the O(N) eviction fix in the admission tables (`parent_slot` and the one-pass marking in `obtain`); keep the existing `table_tests` green and add one test that bounds the work done per new source on a full table.
5. **Remaining time, cheapest first** (REVIEW-3 section 7 item 5): `assert_eq!(challenge.security, 0)` in e2e `login()`; ineligible-FINISH registers exactly one failure; compensation injection (floor bump between create and recheck, assert mirror count and 401); S7 clock refresh in `compensate`.
6. Deferred (record in DESIGN.md as known deviations): S3 per-listener audit queue, S5 pair-drop metrics, histogram buckets, e2e launches (b) and (d), remaining S9 items.
7. Rewrite DESIGN.md status table for items 4-12 (DONE / PARTIAL with file:line and what needs CI).

## Rules (restated)

- NO LOCAL COMPILATION: only `cargo fmt -p <crate>` (and `-- --check`) and `python -I scripts/ci/check_layout.py`. No cargo check/clippy/build/test, no rustc, no npm. Do not edit `Cargo.toml` / `Cargo.lock`; a new dependency means stop and report. A child that runs cargo check/clippy/build/test violates the policy (three children already did and left 2 GB `target/` directories): say so in every child prompt.
- NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index; only edit files; the orchestrator commits.
- Children ONLY edit files, are awaited, and every file a child touched is re-read as rustc/clippy/rust-guard (imports, visibility, Send/'static across `.await`, no local `use`, no `#[allow]`, entry counts by `ls` max 7, files max 1000 lines, test files mounted).
- No machine-local absolute paths in any file you write. Forward slashes.
- Time: run `date` at start; stop editing at the time in the run file, report exactly what is done; never revert or empty files; never finish with a child outstanding.
