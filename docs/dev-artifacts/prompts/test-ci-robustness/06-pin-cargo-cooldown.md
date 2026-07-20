# Test/CI Robustness 7g — pin `cargo-cooldown`'s version in CI

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Seventh (final) item of "Этап 7 — Test / CI robustness"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 08 (Category 5, item (d): "`cargo-cooldown` itself is
installed unpinned"). Two sub-items were named in the work plan; **the
second is already resolved** — investigated and confirmed no stray
`fix643_test.log` or similar debug-log file exists anywhere in the repo
today (`git status --short` is clean apart from legitimate, intentional
checkpoint docs; `tmp/*.log` scratch files are already properly
`.gitignore`d, `run.log`/`bench-run.log`/`bench-history.log` likewise). **Do
not spend time re-investigating this** — just confirm it in your own
summary and move on to the real remaining task below.

**The real task**: `.github/workflows/ci.yml`'s `cooldown` job
(~lines 145-158) installs the tool unpinned:
```yaml
- run: cargo install --locked cargo-cooldown
- run: cargo cooldown check
```
`--locked` here only affects `cargo-cooldown`'s OWN internal Cargo.lock
during its build — it does NOT pin which published version of
`cargo-cooldown` gets installed (same misconception this campaign already
corrected for `cargo-nextest` in task 7a — read that task's brief/commit
for the exact reasoning if useful context,
`docs/dev-artifacts/prompts/test-ci-robustness/01-pin-nextest-version-ci.md`
and git log `ci: pin cargo-nextest@0.9.137`).

**Note the difference from 7a**: unlike nextest, there is no pre-existing
documented "intended baseline" version for `cargo-cooldown` anywhere in
this repo — you need to determine a real, current, valid version yourself
(check crates.io for `cargo-cooldown`'s published versions, or run `cargo
install cargo-cooldown --dry-run` / `cargo search cargo-cooldown` if this
sandbox permits it — if `cargo install` itself is blocked in this
environment the way it is in some of this campaign's sandboxes, use
whatever read-only means you have — crates.io index query, `cargo info
cargo-cooldown` if available in your cargo version, or a web fetch of
`https://crates.io/crates/cargo-cooldown` — to find the current
published version).

## The task

1. Determine the current real published version of `cargo-cooldown` on
   crates.io.
2. Pin it in `ci.yml`'s `cooldown` job:
   `cargo install --locked --version <X.Y.Z> cargo-cooldown` (confirm
   `--version` + `--locked` compose correctly for `cargo install`, same
   as verified in task 7a's nextest pin — check the Cargo Book if you
   want to re-confirm rather than trust this brief blindly).
3. Add a short rationale comment near this step (mirroring the style of
   the nextest pin comment added in task 7a) explaining WHY it's pinned:
   an unpinned supply-chain/dependency-freshness tool silently changing
   behavior between CI runs is exactly the kind of drift a "cooldown"
   gate is supposed to prevent for the WORKSPACE's own dependencies — it
   would be inconsistent to leave the gate tool itself unpinned.
4. Confirm no other unpinned `cargo install` calls exist anywhere else in
   `.github/workflows/*.yml` that this campaign hasn't already addressed
   (this campaign already fixed `cargo-nextest` in task 7a; do a final
   grep sweep of all three workflow files —`ci.yml`, `supply-chain.yml`,
   `numa.yml` — for any other unpinned `cargo install`/install-action
   call and report what you find, fixing it too if it's a real gap of
   the same class).

## Out of scope

- Do NOT re-investigate the `fix643_test.log`/stray-log-file cleanup —
  already confirmed resolved, per the Context section above.
- Do NOT touch `cargo-nextest`'s pin (task 7a, already done) or any other
  already-completed Этап 7 task's artifacts.
- Do NOT touch anything from the already-completed Этапы 1-6 — this
  brief is scoped to this one version pin (plus the final grep sweep).

## Verification (MANDATORY before you report done)

- Validate `ci.yml`'s YAML is still well-formed after your edit (same
  pattern used throughout this campaign — `py -c "import yaml;
  yaml.safe_load(open('.github/workflows/ci.yml'))"` or equivalent).
- Report the exact version you pinned and how you determined it's the
  real current published version (a crates.io fetch, a cargo command
  output, etc. — cite your source, don't guess a plausible-looking
  version number).
- Report the final grep sweep's findings across all three workflow
  files for any other unpinned install call.
- Explicitly confirm the stray-log-file item is a non-issue today (one
  sentence, citing your own `git status`/`find` check).
