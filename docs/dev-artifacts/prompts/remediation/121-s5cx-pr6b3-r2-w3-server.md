# S5c-X PR6b-3 / R2-W3 — server: adoption visibility + admission-time resume refusal

Worktree: current directory. R2-W1 (engine) landed, including
`shamir_db::engine::meta::repl::bookmark_v3::{classify_follower_bookmark,
FollowerBookmarkState, receipt_pointer_resolves}` and gate-init visibility derivation.

## Hard rules (verbatim)
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git
  command that mutates the working tree or index. Only edit files; the orchestrator commits.
- NO `cargo test` / `build` / `check` / `clippy` / `nextest` or any other compile or test
  command, by you or any child agent. Only `cargo fmt -p shamir-server -p shamir-db` as your
  LAST step (shamir-db only if you actually touch it).
- Never silence a lint with `allow`. Layout: <=7 entries/folder, <=1000 lines/file (over-limit
  trees must not grow). Imports at top. No machine-specific absolute paths.

## 1. Warm-gate visibility at identity adoption (zero-event followers)
When identity adoption persists a NEW fence record, the WARM gate must gain
receipt-native visibility in the same step (a zero-event follower never runs the receipt
pipeline, so nothing else enables it before the next restart).
- `crates/shamir-server/src/replication/identity_fence/checks.rs`: internal `decide(...)` now
  returns `Result<bool, ReplError>` — `true` exactly when it persisted a NEW record (the
  `existing.is_none()` save path). Keep every refusal path unchanged.
- `fence_hello_identity(info, ...)` keeps its signature and result type `Result<(), ReplError>`
  (it ignores the bool; info-level tests depend on this) — verify its callers with rg before
  deciding.
- `fence_hello_recorded(db, db_name, repo_name, ...)`: on `true`, resolve the repo (it already
  does) and `if let Ok(gate) = repo.tx_gate().await { gate.enable_receipt_native_visibility(); }`
  with a comment: best-effort, the durable v3 record is the authority and gate-init derives
  the flag on every later open.
- `fence_pull_identity(info, response, receipt)`: change to take the repo as an additional
  parameter (rg its callers first — expected: `follower_loop.rs` only) and enable the same way
  on adoption. Update the follower_loop call site (~line 410) to pass `&repo_instance`.
- Check `recorded.rs`, `source/seed/fence.rs`, the supervisor and every test that calls these
  functions (adoption_durability_tests, warn_tests) — update signatures/uses minimally and
  list every touched call site in your report. warn_tests' warning-once semantics must stay
  intact (the warning fires inside `decide`, unchanged).

## 2. Admission-time refusal of Resume for a corrupt/legacy destination
`crates/shamir-server/src/replication/supervisor.rs` Resume block (~343-391): currently, after
the classify-Ok path a resume starts loops and re-trips at the pipeline. Fix: when
classification returns `Ok(FollowerBookmarkState::Native(v3))`, call
`receipt_pointer_resolves(&info, &v3).await` (propagate real IO errors as today's
`ReplError::Start` mapping); if it returns `false`, mark the subscription terminal
("reopen_required", message naming the corrupt/retired durable receipt pointer) and return
`Err(ReplError::ReopenRequired { db, repo, detail })` — BEFORE any loop is spawned. Keep the
existing legacy/malformed refusal branch exactly as landed (typed FollowerStateRefused →
resync_required terminal).

## 3. supervisor_tests.rs — the resume test must prove REFUSAL, not re-trip
`replication/tests/supervisor_tests.rs::terminal_receipt_corruption_stops_supervisor_and_refuses_resume`
(~706-771): keep the first-trip assertions (loop stops, terminal `reopen_required`, zero
progress). Replace the resume tail: record `source.pulls` (and bookmark/counter) BEFORE the
admin resume; then `alter(...resume()...)` + `sup.reconcile()`; assert IMMEDIATELY that
`!sup.is_running("sub_a")` (the loop was never started), the subscription state is still
`reopen_required`, `source.pulls` is UNCHANGED (no pull happened), and bookmark/counter are
still 0. Update the test's doc comment: admission-time refusal, not detect-and-retrip.
Keep `PartialSource`'s pull counter mechanics as they are.

## 4. Sweeps
- rg both crates for any OTHER caller of `fence_pull_identity` / `fence_hello_identity` /
  `decide` broken by the signature changes; fix all.
- Confirm no test still asserts the old detect-and-retrip resume behavior for the corrupt
  pointer; grep for `re-trip`/`retrip` comments and update wording where the behavior changed.

## Last step
`cargo fmt -p shamir-server` (and `-p shamir-db` only if touched). NO other cargo command.

## Report
Per change: file:line; the complete caller list you updated for the `decide`/
`fence_pull_identity` changes; the resume-refusal ordering you implemented; the new test
assertion list; files over 900 lines; deviations.
