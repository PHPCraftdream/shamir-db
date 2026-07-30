# F-68 (#895) cluster D follow-up — diagnostic instrumentation for the two 600s CI hangs

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`,
or any git command that mutates the working tree or index. Only edit files;
the orchestrator commits.

## Explicit user directive governing this task

The user reviewed the investigation so far and gave this exact instruction:
"мы стабильно в ci гитхаба воспроизводим - вот с помощью него и отлаживай"
("we reliably reproduce it on GitHub CI — use that to debug"). Concretely:
this is NOT a request to build a local repro environment (WSL/cgroups
setup was considered and explicitly rejected as too expensive for
uncertain payoff). The workflow is: add diagnostic instrumentation now →
orchestrator commits and dispatches it on real GitHub CI (which is where
this has actually occurred twice already) → read the resulting logs when
it recurs.

## Hard rule — this is instrumentation, not a functional fix

Do NOT change any behavior. Do NOT add retries, timeouts, or "if slow,
skip" logic. Only add `tracing` spans/events (this workspace already uses
`tracing` throughout — see the existing `tracing::warn!`/`tracing::info!`
call sites cited below) that make the NEXT occurrence of either hang
diagnosable from the CI log alone, without needing a debugger attached.

## Two independent hangs, two independent instrumentation targets

### Target 1 — `shamir-db::rename_table_durability::rename_populated_survives_cold_restart` (ubuntu-latest, TIMEOUT 600.016s)

This test (`crates/shamir-db/tests/rename_table_durability.rs`) does a
sequential (no internal concurrency) create-repo → insert × 3 →
`rename_table` (DDL) → cold restart → read-back cycle. It has no internal
concurrency of its own, so if it hangs, the stuck code is somewhere INSIDE
the engine's DDL/commit machinery, not the test's own logic.

There is a live, independently-filed suspicion (task #897 on this
project's TaskList, from an unrelated code review) of a lock-order
inversion: `pre_commit_prelock` (`crates/shamir-engine/src/tx/pre_commit.rs`,
around lines 452-489) holds `WriterDrainGuard`s ACROSS
`unique_write_lock` acquisition, while DDL paths (including
`rename_table`, wired up as part of F-57's unified lock+drain protocol)
do lock-then-drain — the OPPOSITE order. If that's real, it forms a
deadlock cycle. Whether or not that specific bug is confirmed here, add
`tracing` instrumentation (span or timed event, `tracing::debug!` or
`tracing::warn!` if it exceeds a threshold — e.g. 1s — so it's visible at
default log levels without flooding normal runs) at:

1. Entry and exit of `pre_commit_prelock` (or wherever the actual
   guard-acquire-order lives now — read the current code, don't assume
   the line numbers above are still accurate), timestamped, so a stuck
   run shows which guard/lock it entered and never exited.
2. The `rename_table` DDL path's own lock/drain acquisition sequence
   (`crates/shamir-db/src/shamir_db/shamir_db/table_management.rs`'s
   `rename_table`, and whatever engine-side function it calls into —
   trace the call chain via `crates/shamir-engine/src/table/
   table_manager_index_mgmt.rs` and `crates/shamir-engine/src/repo/
   repo_instance.rs`, both of which reference rename).
3. Any background reaper/GC task that could be running concurrently with
   this test's own DB instance during the cold-restart cycle (check
   `shamir-tx`'s writer-drain barrier and any scheduled sweep) — a
   timestamped "tick started"/"tick finished" pair around its core loop
   iteration, if one isn't already there.

The goal: if this test hangs again on CI, the log should show EXACTLY
which lock/guard/task was entered last and never returned, with a
timestamp gap that lines up with the 600s kill.

### Target 2 — `shamir-server::observability_http::metrics_exposes_unbounded_sentinel_when_no_byte_budget` (macos-latest, TIMEOUT 600.059s)

New hypothesis from this session's investigation, NOT yet confirmed: read
`crates/shamir-server/src/observability.rs`'s `shutdown()` (around line
102-106) and the `listener_task` (around line 383-391). The listener uses
`axum::serve(listener, app).with_graceful_shutdown(shutdown_signal)` —
axum/hyper's graceful shutdown, BY DEFAULT, waits for ALL open
connections (including idle HTTP/1.1 keep-alive connections from EARLIER
requests in the same test, not just in-flight ones) to close before
`with_graceful_shutdown`'s future resolves. If this test's HTTP client
(check `metrics_exposes_unbounded_sentinel_when_no_byte_budget` itself —
likely a `reqwest`/hyper client hitting `/metrics`) doesn't explicitly
close or drop its connection/client before the test calls
`handle.shutdown().await`, the shutdown could wait on that lingering
keep-alive connection — and if THAT connection is never closed by either
side (e.g. the test's client is a struct field or `static` a longer-lived
part of the harness never dropped, or the OS-level socket teardown is
slow/blocked in some way this reproduces only intermittently on macOS),
`shutdown()` hangs.

Investigate this specific hypothesis:
1. Read the test itself and confirm/refute whether its HTTP client
   connection is explicitly closed/dropped before `shutdown()` is called.
2. Add `tracing` instrumentation around `shutdown()` itself (start/end
   timestamps for `notify_waiters()`, for each of the two awaited
   `JoinHandle`s individually — currently `let _ = self.listener_task.await;
   let _ = self.poller_task.await;` awaits them sequentially with no
   logging; if one specifically hangs, timestamped
   before/after-`listener_task.await` and before/after-`poller_task.await`
   events will show WHICH of the two never returns).
3. If the hypothesis holds (or reasoning otherwise suggests the graceful
   shutdown is waiting on a connection), also add a `tracing::warn!` inside
   `axum::serve`'s shutdown path if there's a way to observe connection
   count / an "N connections still open" signal — check whether hyper or
   axum's server exposes anything like this; if not, don't force it, just
   note it as unavailable and rely on the coarser listener_task/poller_task
   timing split above.

## Definition of done

- Both instrumentation additions are `tracing`-based, cost effectively
  nothing on the hot/happy path (no behavior change, no new blocking
  calls, no allocation on every request — a `tracing::debug!`/`warn!` call
  behind the crate's existing tracing setup is fine).
- `cargo fmt -p shamir-db -p shamir-engine -p shamir-server -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-db -p shamir-engine -p shamir-server --full`
  green (the instrumented tests themselves must still pass normally — this
  changes observability, not behavior).
- Commit message states exactly what was instrumented and why, referencing
  this brief and the two hypotheses (lock-order-inversion candidate for
  Target 1, graceful-shutdown-waiting-on-a-lingering-connection candidate
  for Target 2) so whoever reads the CI log next knows what to look for.
- Do NOT attempt to "fix" anything found — if the investigation reveals
  something that looks like the actual bug, name it clearly in the commit
  message and STOP; the orchestrator will decide whether to fix it in this
  task or file it against #897/#896 (which already own the lock-order
  correctness work), since a single investigation session finding a real
  concurrency bug deserves its own careful, separately-verified fix, not
  a same-commit patch bundled with instrumentation.
