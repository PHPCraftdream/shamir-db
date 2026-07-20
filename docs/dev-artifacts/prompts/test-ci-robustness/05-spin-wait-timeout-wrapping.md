# Test/CI Robustness 7f — wrap unbounded spin-waits in `tokio::time::timeout`

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Sixth item of "Этап 7 — Test / CI robustness"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 08 (`docs/dev-artifacts/research/2026-07-17-release-audit/
08-test-coverage-ci-robustness.md`, §3). **This is a REAL TEST-FILE
change** — read report 08's §3 in full first.

**The problem**: several tests poll a shared condition in a loop with no
LOCAL timeout, relying solely on nextest's global 180s slow-timeout kill
(`.config/nextest.toml`'s `[profile.default]`) to eventually terminate them
if the awaited condition never becomes true. When it fires, the failure
looks like an anonymous `TIMEOUT` with no diagnostic message — "an
undiagnosable flaky TIMEOUT" per report 08 — instead of a clear assertion
naming what specifically failed to happen in time.

**The correct pattern already exists in this codebase** —
`crates/shamir-index/src/vector/tests/quantized_graph_tests.rs:1629-1638`:
```rust
let reached = tokio::time::timeout(std::time::Duration::from_secs(5), async {
    while !gate.arrived.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
})
.await;
assert!(
    reached.is_ok(),
    "co-filter search task did not reach the f32-gate within 5s"
);
```
Use THIS as the template: wrap the polling loop itself in
`tokio::time::timeout(Duration, async { ... })`, then assert
`.is_ok()` with a message naming exactly what condition should have
become true. A genuine hang now fails FAST (at your chosen bound,
seconds not 180s) with a clear message, instead of an anonymous kill.

## The five sites report 08 names — VERIFY EACH YOURSELF, don't apply the
## same fix mechanically to all five; some are already partially bounded
## by a different mechanism

1. **`crates/shamir-tx/src/tests/mvcc_store_tests/overlay_ordering_tests.rs`**
   (lines ~140, ~237) — `while !stop.load(Ordering::Relaxed) { ...
   yield_now().await }` reader loops. **Confirmed genuinely unbounded**:
   `stop` is only set via `stop.store(true, ...)` by the MAIN test body
   AFTER the writer-side work completes (not on a wall-clock timer) — if
   the writer ever deadlocks (the exact bug class this test exists to
   catch, per report 08's framing "the known pair... racing a writer
   through the real ack path"), these reader loops spin forever with
   ZERO local bound. **This is the highest-priority site — fix it.**
2. **`crates/shamir-engine/src/table/tests/doctor_tests.rs`** (~lines
   214-251, two similar loops) — `while table.is_background_verify_running()
   && tries < 100 { yield_now().await; tries += 1; }` (and a sibling with
   `tries < 200`). **These ARE already iteration-bounded** (they terminate
   after N loop iterations regardless of wall-clock time), which is a
   weaker-but-real bound — NOT the same risk class as #1. Decide whether
   converting these to the `tokio::time::timeout` pattern is still worth
   it for consistency/better diagnostics (a wall-clock bound is more
   predictable across variable CI-runner speed than a fixed iteration
   count), or whether you judge the existing iteration cap adequate and
   leave it. State your reasoning either way.
3. **`crates/shamir-engine/src/tx/tests/backpressure_gc_tests.rs`**
   (~line 284) — a `loop { let n = drainer_bg.drain_step(...).await...; if
   n == 0 { break } }` inside a spawned background task. **Verify**: this
   loop terminates by its OWN logic (drain until nothing's left, bounded
   by the finite seeded WAL data) — it is not spinning on an external
   condition. The test's actual synchronization point
   (`apply_backpressure`) is ALREADY wrapped in a 30s
   `tokio::time::timeout` a few lines below, with a comment explaining the
   30s bound was chosen deliberately relative to a 5s internal deadlock
   guard. This citation looks like it may NOT need the same fix — confirm
   this reasoning yourself and either leave it with a comment explaining
   why it's already adequately bounded, or wrap it if you find a real gap
   I'm missing.
4. **`crates/shamir-tx/src/tests/mvcc_store_tests/version_tests.rs`**
   (~line 606) — a single `tokio::task::yield_now().await` used to "increase
   interleaving odds" before a subsequent `.await` on `mvcc_r.get_at(...)`.
   **This is NOT a spin-wait loop** — it's one yield, bounded by
   definition. The real question is whether the SURROUNDING test (the
   whole `read_handle`/similar task, or the outer test function) has any
   overall wrapping timeout on the `.await` chain that follows — if
   `get_at` (or whatever it awaits) could itself hang with no local bound,
   THAT'S the real risk, not the yield. Investigate the actual risk here
   (read the whole test function) before deciding what, if anything, needs
   wrapping — the report's one-line citation may be imprecise about
   exactly where the risk is.
5. **`crates/shamir-tx/src/tests/mvcc_store_tests/lock_tests.rs`** (~lines
   165, 178) — same pattern as #4: single `yield_now().await` calls before
   `lock_key(...).await` calls in a wound-wait deadlock-prevention test.
   Same investigation approach as #4: the real risk (if any) is whether
   `lock_key` itself could hang with no bound in this specific test's
   race scenario, not the yield itself.

## The task

1. For site #1 (`overlay_ordering_tests.rs`), apply the
   `quantized_graph_tests.rs` pattern: wrap each reader loop in
   `tokio::time::timeout` with a reasonable bound (shorter than nextest's
   180s kill, long enough for legitimate completion — look at how long
   the test's writer-side work realistically takes and give meaningful
   headroom; the `quantized_graph_tests.rs` template uses 5s for a much
   smaller operation, so this test's bound may need to be larger — use
   your judgment and justify the number in a comment). Assert `.is_ok()`
   with a message naming the specific condition.
2. For sites #2-#5, investigate each per the notes above and either (a)
   apply the same wrap with justification, or (b) leave as-is with a
   comment explaining why it's already adequately bounded by a different
   mechanism. Do NOT mechanically copy-paste the same fix onto every site
   without this judgment call — the brief and my own investigation
   found real differences between these five citations' actual risk
   profiles.
3. Do a broader grep pass yourself for any OTHER `while` loop containing
   only a `yield_now().await` (or similar spin pattern) with NO local
   timeout ANYWHERE in the test suite that report 08 might have missed —
   report 08's own count was "~60 files use multi_thread", not all of
   which were individually audited for this specific pattern. If you find
   additional genuine instances of site-#1's risk class (external-flag-
   gated unbounded loop, no local bound), fix them too and report them as
   additions beyond this brief's original list.

## Out of scope

- Do NOT change any non-test source code — this is entirely test-file
  hardening.
- Do NOT touch report 08's other sections or the nightly stress lane
  (task 7e, already done) — this brief is scoped to spin-wait wrapping.
- Do NOT touch anything from the already-completed Этапы 1-6 or tasks
  7a/7b/7c/7d/7e — scoped to this one hardening pass.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-tx -p shamir-engine --full` green,
  including every test you touched, run at LEAST twice to catch any
  timing sensitivity your new timeout bound might introduce (a bound set
  too tight could turn a legitimately-slower-but-fine run into a false
  failure — this is exactly the failure mode this whole campaign has
  been triaging all session, don't introduce a new instance of it).
- `cargo fmt -p shamir-tx -p shamir-engine -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- For each of the five cited sites, state explicitly what you did (wrapped
  / left as-is) and why, per the investigation notes above.
- Report any additional sites you found and fixed beyond the original
  five, or explicitly state you found none.
