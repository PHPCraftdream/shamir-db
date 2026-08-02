# Checkpoint — 2026-08-02 12:XX [ci-followup-in-flight]

## Session summary

This session continued directly from the completed F-69..F-87 remediation wave (all 18 tasks done, verified, committed — see `docs/checkpoints/p0-p1-wave-complete.md`). The user then gave a sequence of new, distinct instructions: (1) "удали вообще все задачи, делай коммит и пшу, наладь ci" (delete all tasks, commit, push, fix CI). All 34 old tasks were deleted, the working tree was already clean (nothing to commit), and `git push origin master` ran — **this was the first push of the entire session/history of local work to origin**, revealing that `origin/master` had been stuck 45 commits behind local master this whole time (every nightly/scheduled CI run for the past 10+ days had been running against stale, pre-fix code).

Investigating "наладь ci" surfaced four distinct issues, handled in order: (a) `supply-chain` (cargo-deny) failed on two freshly-disclosed RUSTSEC advisories — `event-listener` 5.4.1 (unsound, fixed by `cargo update` to 5.4.2) and `wasmtime` 45.0.0 (security vulnerability, but its ONLY patched versions require Rust 1.94.0 while this workspace is pinned to 1.93.0 — after confirming this via a real attempted bump + `cargo check` MSRV failure, the user approved deferring it via a triaged `deny.toml` ignore entry mirroring the existing bincode-advisory precedent). Committed and pushed as `89a0ca24`. (b) A `truncation_ceiling_blocks_segment_removal_when_a5_gate_unsafe` test failure on the first real CI run turned out to be the exact known convergence-timing flake the test's own comments already document (~1/4 rate) — confirmed by a clean local pass, no code change needed. (c) `ts-e2e-nightly` has failed on EVERY scheduled run for 10+ consecutive days (confirmed via `gh run list`, unrelated to this session's work) — two distinct root causes found: a "connection closed" cascade in `e2e-permissions.test.ts`'s first test (CI-1, #916, in progress) and a `ReplHello`-as-plain-user wrong-error-code mismatch (`hmac_required` instead of expected `bad_role`) in `tests/e2e`'s `09-errors.test.js` (CI-2, #917, not started). (d) The ORIGINAL F-68b ask (two 600s CI hangs) did NOT reproduce on the first real CI run after the push — plausible the session's own F-70 lock-order-inversion fix already resolved it, but not confirmed with certainty from one run (CI-3, #918, not started, P2).

For CI-1, I personally investigated (via an Explore sub-agent plus my own direct code reading) before writing a brief: found that an EARLIER related fix (`05c2028f`, F-68 cluster C, `panic = "abort"` → `"unwind"`) is a strong-looking candidate explanation for "connection closed" (a server crash under panic=abort would look exactly like this to a client) — but personally VERIFIED this is NOT the actual explanation for the CURRENT failure, since the failing CI run (`30727972168`) ran against commit `ed8bba00`, which already includes `05c2028f`. I also found a concrete, actionable gap: `e2e-harness.ts`'s `ServerHandle.logs()` accessor exists (captures the spawned server's full stdout+stderr) but `e2e-permissions.test.ts` only calls it in the `beforeAll`'s connection-failure handler — NOT on any later test's failure — so today's CI logs give zero visibility into whether/where the server actually crashed during the `B-setup` test. The brief (`docs/dev-artifacts/prompts/post-alpha/142-ci1-e2e-permissions-connection-closed.md`, committed as `06eeccc3`) directs the implementer to add that missing diagnostic FIRST (mirroring the "instrument, observe on real CI, then fix" workflow this repo already used successfully for F-68 cluster D), then use the newly-visible log to find and fix the real root cause.

The first `crush run` attempt for CI-1 failed immediately with `provider zai is in peak hours (08:00–12:00), refusing until 12:00` (no work done, zero tool calls). I initially armed a recurring 30-minute retry cron; the user then asked me to delete that and set a precise one-shot alarm instead, and confirmed "12:00" means their own local time (not UTC, not Beijing time — I had asked to disambiguate since the error carried no timezone). I deleted the recurring cron and armed a ONE-SHOT cron (`026c8ce5`, `3 12 2 8 *`, local time, session-only) that fires once, retries the exact same `crush run` command for CI-1, and — if it succeeds — is instructed to perform this session's full zero-trust verification (diff read, fmt/clippy/tests, and critically a REAL `gh workflow run ts-e2e-nightly.yml` trigger + `gh run watch` to confirm the fix actually works on GitHub's runners, not just locally) before committing and marking #916 done, then proceeding to #917 if there's room. If the alarm fires and crush is STILL blocked (wrong timezone guess), the tick is instructed to stop and report back to the user rather than guess again or self-schedule another retry.

No `/goal` Stop-hook is active this session (babysit-tick pattern from the earlier wave was fully torn down along with the old TaskList and its cron). This is the immediate context right before the user ran `/checkpoint`.

## Active goal

None (no `/goal` Stop-hook armed). The operative standing instruction is the user's own most recent message sequence: "наладь ci" (fix CI), followed by "заведи таски исправить оставшееся" (create tasks for what's left) — both satisfied by the TaskList state below. No hard Stop-hook condition is enforcing continuation; the one-shot alarm (`026c8ce5`) is the only automation currently armed to resume work.

## TaskList

### in_progress
- #916 CI-1: fix TS client e2e "connection closed" cascade in e2e-permissions.test.ts — brief committed (`06eeccc3`), first crush attempt blocked by zai peak-hours, one-shot retry alarm armed for 12:03 local time today (`026c8ce5`, id `2026-08-02`).

### pending
- #917 CI-2: fix node napi e2e ReplHello wrong error code (hmac_required vs bad_role) — investigation not yet started; brief not yet written (planned path: `docs/dev-artifacts/prompts/post-alpha/143-*.md`). Should not start until #916 is fully verified+committed (both may touch shamir-server).
- #918 CI-3: confirm F-68b's two 600s CI hangs are resolved (or investigate if not) — P2, lower priority; plan is to trigger CI 2-3 more times and watch for recurrence, not yet started.

### recently completed (this session, since the F-69..F-87 wave closed)
- #915 (umbrella investigation, now closed/decomposed into #916-#918): supply-chain RUSTSEC fix committed+pushed (`89a0ca24`); truncation flake diagnosed as pre-existing/accepted, no fix needed; original F-68b hangs did not reproduce on one real CI run (inconclusive, tracked as #918).

## Decisions

- Deferred the wasmtime RUSTSEC-2026-0222 fix via a triaged `deny.toml` ignore (not a silent ignore — full reasoning comment mirroring the existing bincode precedent) rather than bumping the Rust toolchain, because the toolchain bump is a separate, much larger, invasive change (every crate's clippy needs re-verification, every `dtolnay/rust-toolchain@1.93.0` CI ref needs updating) that the user had not asked for — confirmed via `AskUserQuestion` before proceeding down the smaller (approved) path.
- For CI-1, ruled out the obvious-looking candidate explanation (the already-fixed `panic=abort` bug) with hard evidence (commit-ancestry check) BEFORE writing the brief, rather than assuming crush would need to re-discover this — the brief explicitly tells the implementer NOT to re-investigate that dead end.
- Chose "add the missing `server.logs()` diagnostic first, then observe on real CI, then fix" over guessing at the root cause blind, mirroring this repo's own established F-68-cluster-D precedent for exactly this class of CI-only, hard-to-reproduce-locally bug.
- When crush hit the zai peak-hours block, initially armed a recurring 30-min retry cron; the user explicitly asked to replace it with a precise one-shot alarm instead, and specified the reopen time should be interpreted as their own local time (not UTC, not the provider's likely Beijing/China Standard Time) — deleted the recurring cron, armed a one-shot in its place per that instruction.
- The one-shot alarm's own prompt explicitly forbids self-scheduling another retry if it fires and crush is STILL blocked (wrong timezone assumption) — it must surface that back to the user rather than keep guessing, since a second wrong guess would burn another multi-hour wait silently.

## Open questions

- None outstanding requiring immediate user input beyond what the armed alarm will surface if the 12:00-local-time assumption turns out wrong.

## Repo state

```
(clean — nothing to commit)
```
```
06eeccc3 docs(prompts): brief for CI-1 -- diagnose+fix e2e-permissions connection-closed cascade (#916)
89a0ca24 fix(deps): resolve event-listener RUSTSEC-2026-0221, defer wasmtime RUSTSEC-2026-0222 (cargo-deny)
ed8bba00 docs: F-87 -- doc-accuracy cleanup batch (RAII drop order, CHANGELOG, try_build scope) (#915)
72dd6357 docs(engine,index): F-86 -- fix stale F-70-contradicting lock-order docs + relocate inline tests (#914)
8c0ddc9b docs(prompts): brief for F-86 -- stale lock-order docs + relocate inline tests (#914)
```

Note: `master` is currently pushed and in sync with `origin/master` as of commit `89a0ca24` (the `06eeccc3` brief commit may or may not have been pushed yet — check `git status -sb` / `git log origin/master..master` before assuming parity).
