# Test/CI Robustness 7c — add `[[profile.ci.overrides]]` for SCRAM/WASM tests

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Third item of "Этап 7 — Test / CI robustness"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 08. This is a `.config/nextest.toml`-only change —
read the WHOLE file first, it's short and already well-commented.

**Investigation already done (verify yourself too):**

- `[profile.ci]` already exists. It has its OWN `slow-timeout = { period
  = "60s", terminate-after = 10 }` (kill at 600s — deliberately looser
  than `[profile.default]`'s 180s, because CI runners are assumed slower/
  more contended than the dev box the default timeouts were tuned for)
  and `test-threads = 4` (task #589's oversubscription fix). It has
  exactly ONE `[[profile.ci.overrides]]` entry today: the
  `functions_lifecycle` / `wasm-heavy` test-group assignment.
- `[profile.default.overrides]` has TWO additional per-test overrides
  that are NOT mirrored into `[profile.ci.overrides]`:
  ```toml
  [[profile.default.overrides]]
  filter = "test(wasm_function_inserts_and_queries)"
  slow-timeout = { period = "120s", terminate-after = 2 }  # ~99 s legit, kill at 240 s.

  [[profile.default.overrides]]
  filter = "package(shamir-connect) and test(/.*scram.*/)"
  slow-timeout = { period = "10s", terminate-after = 6 }   # Argon2-bound; kill at 60 s.
  ```
- **Confirmed via `scripts/test.sh` (~lines 184-197): CI actually runs
  with `--profile ci`** (auto-selected when `CI=true`, unless the caller
  already passed an explicit `--profile`). This means: **nextest profile
  overrides do NOT merge across profiles** — `[profile.ci]` does NOT
  automatically inherit `[profile.default]`'s `[[overrides]]` list. So in
  a real CI run today, `wasm_function_inserts_and_queries` and every SCRAM
  test fall through to `[profile.ci]`'s own blanket `60s/10` (kill at
  600s) instead of their dev-tuned per-test values — **verify this
  nextest semantic yourself** (check nextest's own documentation on
  config-file profile inheritance / override merging — this is the
  crux of the whole task, don't proceed on my say-so alone).

**The actual gap, once verified**: this is NOT quite "dev-box kills leak
into CI" (the literal work-plan phrasing) — it's closer to the opposite:
CI currently has NO per-test override for these two cases at all, falling
back to a MUCH LOOSER blanket (600s) than either test's dev-tuned intent
(240s for the WASM test, 60s for SCRAM). For the WASM test, 600s is still
a safe upper bound (huge margin over the ~99s legit runtime) so this is
low-severity there. For SCRAM tests specifically, falling back to 600s
instead of a tight, Argon2-cost-derived bound means a genuinely hung SCRAM
test in CI would run 10× longer than intended before nextest kills it —
a real regression-detection-latency gap.

## The task

1. Add `[[profile.ci.overrides]]` entries mirroring both missing
   dev-profile overrides, but do NOT just copy the dev-tuned values
   verbatim — CI runners are plausibly slower, so a straight copy of the
   TIGHT dev value (e.g. SCRAM's `10s/6` = 60s kill) risks the exact
   "dev-box kill leaking into CI" failure mode the work-plan item names:
   false-positive timeouts on legitimately-slower-but-not-hung CI
   hardware. Pick CI-appropriate values with real headroom over the dev
   numbers, and say explicitly why you chose the multiplier you did
   (there's no real CI-timing data available in this repo to calibrate
   against — say so, and mirror this file's own existing convention of
   labeling a new value as a "starting point, not a measured optimum" the
   same way the `test-threads = 4` comment already does for its own
   value).
2. Suggested starting point (adjust with your own reasoning, this is not
   a mandate): SCRAM → something like `20s/6` (120s kill — 2× the dev
   bound, still much tighter than the 600s blanket) rather than reusing
   `10s/6` verbatim. WASM `wasm_function_inserts_and_queries` → decide
   whether it needs its own CI override at all, given the existing
   600s blanket already covers the ~99s legit runtime with 6× headroom —
   if you conclude the blanket is already sufficient, don't add a
   redundant override; explain that reasoning in your summary instead of
   silently skipping it.
3. Update the file's own top-of-file-ish commentary (near the
   `[profile.ci]` section or the `[[profile.default.overrides]]` section)
   to note that CI overrides must be maintained SEPARATELY from default
   overrides (since they don't merge) — this is exactly the kind of
   "verify this yourself" nextest semantic that bit this campaign once
   already (per `scripts/test.sh`'s own comment about the profile
   never having been selected before); make it impossible for a future
   editor to add a new `[[profile.default.overrides]]` entry and assume
   it also applies in CI.

## Out of scope

- Do NOT touch `[profile.default]` or its existing overrides.
- Do NOT touch `test-threads` or `slow-timeout` at the `[profile.ci]`
  top level (the blanket CI timeout) — this brief only adds per-test
  overrides underneath it.
- Do NOT touch anything from the already-completed Этапы 1-6 or tasks
  7a/7b — this brief is scoped to `.config/nextest.toml` only.

## Verification (MANDATORY before you report done)

- This is a nextest TOML config change — validate it parses correctly
  by actually running nextest against it with the `ci` profile selected,
  e.g. `cargo nextest list --profile ci -p shamir-connect` (or similar —
  pick a command that exercises config parsing without running the full
  suite) and confirm it doesn't error.
- If feasible, run the actual SCRAM test subset under `--profile ci` to
  confirm it completes well within your new override's bound (e.g.
  `./scripts/test.sh -p shamir-connect --full -- scram` with `CI=true`
  set, or the direct nextest equivalent) and report the real timing —
  this gives you actual local evidence for whether your chosen multiplier
  is sane, even though it can't fully substitute for real CI-hardware
  data.
- Report literal command output for all of the above.
- Explicitly state your citation/verification for the "profiles don't
  merge overrides" nextest semantic this whole task hinges on (a doc
  link, a `cargo nextest` help output, or an empirical test where you
  add a temporary log/marker override and confirm it does/doesn't fire
  under a different profile) — don't just assert it.
