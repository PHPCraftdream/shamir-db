# Root-cause investigation and fix: Windows "clean shutdown timed out during flush" (#1378)

## Observed symptom (evidence, not interpretation)

On the GitHub-hosted `cargo test lib (windows-latest)` job, two tests of `shamir-db` failed within one
second of each other (full log saved at `D:\system_artefact\orch\flake_win_20261023.log`, a plain
UTF-8 log with the CI prefixes already removed; the failing region is around lines 15313-15480; the
summary line is `7372 tests run: 7370 passed, 2 failed`):

- `shamir_db::tests::durability_policy_tests::boot_reopen_inherits_policy_and_recovered_prefix`
  (37.4 s), panic at `crates/shamir-db/src/shamir_db/seed/tests/durability_policy_tests.rs:236` with
  `Storage("unclean shutdown: [(\"__system__/system\", \"Storage backend error: clean shutdown timed out during flush\")]")`
- `shamir_db::tests::repo::durability_levels_tests::buffered_batch_executes_successfully`
  (32.8 s), same error.
- Both stderr show: `shutdown stage slow: repo 'system' stage 'flush_buffers' was cancelled or still
  running after 14.7107037s`, then `worker lifecycle quarantine: clean shutdown failed`, then
  `flush_all: system store: Storage backend error: clean shutdown timed out during flush`.
- The code is `RepoInstance::shutdown_clean_inner` in
  `crates/shamir-engine/src/repo/instance/repo_instance/lifecycle/recovery/clean_horizon_shutdown_impl.rs`:
  one absolute deadline `now + budget` covers lifecycle drain, commit quiescence, `flush_buffers()` and
  the checkpoint; the `flush_buffers` stage is wrapped in `tokio::time::timeout_at(deadline, ...)`.
- The same two test names were already reported as the Windows flake task #1378 earlier ("Windows
  `durability_levels_tests::buffered_batch_executes_successfully`"), and an older log from another
  occurrence may exist as `D:\system_artefact\orch\ci189_win_lib.log` (check; use any other saved
  Windows logs in `D:\system_artefact\orch\` too). Master CI passes these tests most of the time.

## Task

Find the REAL mechanism and fix it at its root, in this same change. The owner's rule: a flaky test
is never papered over (no raised timeouts, no `#[ignore]`, no retries around the assertion, no
looser asserts). Work like an investigator:

1. Separate what is observed from what is inferred; rank hypotheses by evidence. Candidates to
   evaluate, each against the code and the log: (a) the flush really takes >14.7 s because of I/O
   contention (fjall `persist`/fsync on a runner that runs many test processes in parallel) —
   check what `flush_buffers()` does for the `system` repo (read `RepoInstance::flush_buffers`, the
   storage backend `flush`/`persist` path in `crates/shamir-storage`, fjall sync mode) and whether
   anything inside it blocks a tokio worker thread with synchronous I/O (current-thread test
   runtime!) so that the timeout timer itself cannot fire or that other tasks starve;
   (b) a lock / await cycle: `flush_buffers` waiting for a background task (compaction, memtable
   flush worker, journal drainer, `FjallError::Locked`-style asynchronous release after drop, see
   the earlier flake fixes #241/#245 where background tasks hold clones until they run) that is
   itself waiting for the runtime the test holds; (c) the shutdown budget in the tests being a fixed
   small value that the CI runner under load cannot meet, while production is fine; (d) something
   specific to the two tests (they share `boot_reopen`/`buffered` Synced durability paths on a real
   disk-backed DB).
2. Read the two tests and the full call chain from the test's shutdown to the storage `flush`. Use
   the per-stage timestamps in the log (the lines before 15313 for both tests) to establish when each
   test started its flush relative to the other test and to the whole job (the two failures are one
   second apart: decide if that points to a shared stall or to a deterministic property of the tests).
3. Decide the cause. If it is real I/O contention on the runner: the fix is to bound the contention
   at its source for exactly these fsync-heavy tests (for example a nextest test-group with a small
   `max-threads` in `.config/nextest.toml` for the disk-synced shamir-db/shamir-engine tests on
   Windows, or making the tests not use a synchronous fsync path when they do not test it), with the
   evidence of why this removes the stall. If it is a code bug (blocking I/O on the runtime thread, a
   lock held across an await, a missing wake-up, an uncancelled background task): fix the code.
   Whatever the cause is, add a regression guard that would have caught it (a deterministic test of
   the mechanism if one can be written without relying on timing; otherwise say clearly why not).
4. Do not raise any timeout and do not touch `.config/nextest.toml` slow-timeouts. Do not change
   unrelated behaviour. Keep production semantics (durability order, shutdown contract) identical.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command
  that mutates the working tree or index (including `git add` / `git commit`). Only edit files; the
  orchestrator commits.
- Compile / build / test policy: see the policy text at the top of this run (verbatim). Allowed
  locally: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D
  warnings`, `cargo fmt -p <crate>`, `python -I scripts/ci/check_layout.py`. NOT allowed: tests,
  benches, builds. The fix can only be verified by CI, so reason carefully and say what CI must show.
- NEVER stop processes by image name or pattern. Never use the shell builtin `xargs`. Capture exit
  codes without pipes. Logs under `.scratch/`. Never send the user's e-mail or personal data anywhere.
  No machine-specific absolute paths in committed files (the saved log path above is for reading only).
- Layout limits: at most 7 entries per folder, at most 1000 lines per file; imports at the top;
  `mod.rs` re-exports only.
- Before finishing, start ONE read-only verifier child (the whole policy pasted verbatim) that
  re-derives the mechanism from the code and the log independently and checks the fix against it.

## Final message

1. Observed vs inferred, the ranked hypotheses and the evidence that decided between them (file:line
   and log line numbers).
2. The root cause in plain words and exactly what changed (file:line), including why it removes the
   stall and not only reduces its probability; if you can only reduce the probability, say so
   explicitly.
3. The regression guard (or why none can be deterministic).
4. Real exit codes of the allowed gates; what CI must show to count the flake as fixed.
