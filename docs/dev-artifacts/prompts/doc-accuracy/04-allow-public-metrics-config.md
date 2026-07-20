# Documentation Accuracy 6d — implement the `allow_public_metrics` config knob (currently phantom)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Fourth item of "Этап 6 — Documentation accuracy"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 09. This is a **REAL CODE fix**, not docs-only (same
shape as task 5d earlier this campaign — a documented config knob that
doesn't actually exist / isn't wired up).

`docs/guide-docs/guide/07-operations.md` (lines ~247-255) tells operators:

> Loopback-only по умолчанию. Для non-loopback — нужно явно разрешить
> `allow_public_metrics: true` (не рекомендуется).

...and even has its own pre-existing, honest TODO right below it:
`<!-- TODO: verify allow_public_metrics field name in ObservabilityConfig
— see config.rs -->`

**Investigation already done (verify yourself too):**

- The underlying SAFETY MECHANISM is real and tested:
  `crates/shamir-server/src/observability.rs::spawn` takes a real
  `allow_public_metrics: bool` parameter (line ~160) and enforces it
  (line ~166: `if !addr.ip().is_loopback() && !allow_public_metrics {
  return Err(ObservabilityError::NonLoopbackBindRejected(addr)); }`),
  with a real regression test
  (`crates/shamir-server/tests/observability_http.rs::
  refuses_non_loopback_bind_without_opt_in`).
- **But there is no way for an operator to actually set this to `true`
  via config.** `crates/shamir-server/src/config.rs`'s
  `ObservabilityConfig` struct (line ~157) has exactly ONE field —
  `addr: String` — no `allow_public_metrics` field exists at all.
- The one call site, `crates/shamir-server/src/server/
  server_launcher.rs` (~line 665-680), hardcodes the literal `false` at
  BOTH of its two `crate::observability::spawn(...)` calls (the primary
  path and the "recorder already installed" fallback path), with its own
  comment admitting this: *"M-tier audit M5: pass `allow_public_metrics =
  false`. A non-loopback `addr` is rejected up-front. Operators that need
  a public scrape endpoint can promote this to a config flag in a
  follow-up."* — this brief IS that follow-up.

## The task

**Implement the config field** (this brief's default choice, mirroring
5d's precedent — the fix is small and low-risk; investigation above shows
exactly one struct field to add and two call sites to thread it through).
If, after reading the code yourself, you find this assessment wrong (a
real complication exists), fall back to removing/correcting the doc claim
instead and say so explicitly in your summary.

1. Add `allow_public_metrics: bool` to `ObservabilityConfig`
   (`crates/shamir-server/src/config.rs`), `#[serde(default)]` (default
   `false` — preserves today's safe-by-default behavior for every
   existing config file that doesn't mention this field). Document it
   with a doc comment explaining the M5 audit rationale (mirror the
   existing `addr` field's doc-comment style immediately above it).
2. Update `impl Default for ObservabilityConfig` to include the new field
   (`allow_public_metrics: false`).
3. In `server_launcher.rs`, replace both hardcoded `false` literals (the
   5th positional arg to `crate::observability::spawn(...)`, at both call
   sites — success path and the recorder-already-installed fallback) with
   `config.observability.allow_public_metrics` (check the exact variable
   name holding the parsed config at that point in the function — mirror
   how `addr` is already read from the same config struct a few lines
   above).
4. Delete the resolved TODO comment in `07-operations.md` (it's now
   answered — the field exists under exactly the name already documented,
   `allow_public_metrics`) — confirm this and update the surrounding
   prose only if the field's actual serde name/behavior differs from what
   the doc already says (it shouldn't, per the investigation above, but
   verify).

## Tests

1. A config-parsing test confirming `allow_public_metrics: true` in a
   `.ktav` config file round-trips into `ObservabilityConfig.
   allow_public_metrics == true` (check `crates/shamir-server/tests/
   config.rs` — mirror an existing test for a similar boolean/optional
   field in the same file for style).
2. Confirm the DEFAULT (field omitted from config) still parses to
   `false` — regression test, preserves the documented safe-by-default
   behavior (config parsing tests already exist for other
   `#[serde(default)]` fields in the same file — mirror one).
3. An end-to-end-ish test (or extend the existing
   `refuses_non_loopback_bind_without_opt_in` test's sibling coverage in
   `observability_http.rs` if one already tests the ALLOWED path) proving
   that when `allow_public_metrics: true` flows through
   `server_launcher.rs`'s real boot path (not just the low-level `spawn`
   function directly), a non-loopback observability bind actually
   succeeds instead of being rejected. Check whether `observability_http.rs`
   already has such a positive-path test calling `spawn` directly with
   `allow_public_metrics: true` — if so, that's sufficient standalone
   coverage of the enforcement logic itself; what's NEW and needed here is
   coverage that the CONFIG FIELD actually reaches that parameter through
   the real `server_launcher.rs` boot path, not just that the low-level
   function behaves correctly when called directly with the literal
   `true`.

## Out of scope

- Do NOT change the underlying enforcement logic in `observability.rs`
  (`spawn`'s existing loopback check is correct and already tested) — this
  brief only wires a config-file-level knob through to the ALREADY-CORRECT
  existing parameter.
- Do NOT add any other observability config knobs beyond
  `allow_public_metrics` — out of scope.
- Do NOT touch anything from the already-completed Этапы 1-5 or tasks
  6a/6b/6c — this brief is scoped to this one config knob only.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-server --full` green, including all
  new/modified tests.
- `cargo fmt -p shamir-server -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) the default behavior (field omitted from
  config) is unchanged — still rejects non-loopback binds; (b) an
  operator can now actually achieve what `07-operations.md` has always
  told them to do (`allow_public_metrics: true` in their config file
  actually works end-to-end through the real boot path, not just at the
  low-level function); (c) whether you removed 07-operations.md's TODO
  as resolved or needed to correct the doc's wording to match a
  different reality than expected — state which and why.
