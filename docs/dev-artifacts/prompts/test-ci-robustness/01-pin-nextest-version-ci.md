# Test/CI Robustness 7a — pin `cargo-nextest@0.9.137` in CI (guard-coupling risk)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

First item of "Этап 7 — Test / CI robustness"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 08. This project's whole test-execution safety model
depends on a coupling documented in `.config/nextest.toml`'s own header
(read it in full):

> **PERIMETER GUARD COUPLING**: The cargo-runner guard in
> `.cargo/config.toml` refuses bare `cargo test` and only allows test
> processes that have the `$NEXTEST` environment variable set.
> cargo-nextest sets `$NEXTEST` in every test process it spawns; that is
> the sole signal the guard relies on.
>
> **RISK**: if a future cargo-nextest release renames or removes
> `$NEXTEST`, the guard will fail-closed — it will refuse ALL test
> invocations, including those launched through `./scripts/test.sh` and
> `cargo t`.
>
> **Pinned baseline: cargo-nextest 0.9.137** (commit 75ddba7e9,
> 2026-05-26). Verify with: `cargo nextest --version`

**Confirmed via investigation: this pin is NOT actually enforced
anywhere.** `.github/workflows/ci.yml` installs nextest via
`- uses: taiki-e/install-action@nextest` at TWO call sites (the `test`
job, ~line 59, and the `integration` job, ~line 109) — this shorthand
form installs whatever version `taiki-e/install-action` currently
resolves as "latest" for the `nextest` tool, with no version pin at all.
Locally, `CLAUDE.md`'s own installation instruction
(`cargo install cargo-nextest --locked`) also does not pin a version —
`--locked` there only means "respect nextest's own internal Cargo.lock
during its build", not "install version X.Y.Z". So the documented
"Pinned baseline: 0.9.137" is currently aspirational, not real, in BOTH
CI and the documented local install path.

## The task

1. **Research `taiki-e/install-action`'s actual version-pinning syntax**
   before writing any YAML — don't guess. Check the action's own
   repository/README (it's a widely-used GitHub Action; you likely have
   access to fetch its documentation, or check for a vendored copy /
   cached action metadata in this environment). The general pattern for
   this action family is a `tool@version` string passed via a `with:`
   block on the full-form invocation (`taiki-e/install-action@v2` +
   `with: { tool: "nextest@X.Y.Z" }`), as opposed to the shorthand-ref
   form currently used (`taiki-e/install-action@nextest`, which has no
   room for a version suffix). Confirm the EXACT correct syntax before
   editing `ci.yml` — a wrong syntax silently installs latest anyway or
   breaks the step outright, exactly the failure mode this task exists
   to prevent.
2. Update BOTH `taiki-e/install-action@nextest` call sites in
   `.github/workflows/ci.yml` to pin `cargo-nextest` to `0.9.137`
   (matching `.config/nextest.toml`'s documented baseline exactly).
3. Update `CLAUDE.md`'s local install instruction
   (`cargo install cargo-nextest --locked`, ~line 232, in the
   "🧪 Centralised test entry point" section) to pin the same version:
   `cargo install cargo-nextest --version 0.9.137 --locked` (verify this
   exact flag combination is valid `cargo install` syntax — `--version`
   and `--locked` are both real `cargo install` flags but confirm they
   compose the way you expect).
4. Update `.config/nextest.toml`'s own header comment if its wording
   ("Pinned baseline: cargo-nextest 0.9.137") needs any adjustment now
   that the pin is ACTUALLY enforced (e.g. it could say "enforced in
   `ci.yml` and `CLAUDE.md`'s install instruction" instead of just
   stating a baseline that wasn't wired anywhere) — your call whether
   this needs wording changes or is already accurate once the two real
   fixes land.

## Out of scope

- Do NOT bump the pinned version to something newer than 0.9.137 — this
  brief is about ENFORCING the already-documented baseline, not choosing
  a new one. If you discover 0.9.137 is somehow no longer available/valid
  (yanked, action doesn't support that old a version, etc.), stop and
  report this rather than silently picking a different version.
- Do NOT touch the `dtolnay/rust-toolchain@1.93.0` pins or any other
  action version in `ci.yml` — scoped to nextest only.
- Do NOT touch anything from the already-completed Этапы 1-6 — this
  brief is scoped to this one CI/tooling pin.

## Verification (MANDATORY before you report done)

- Since this is a CI-workflow + doc-instruction change, no local
  `cargo test`/`clippy`/`fmt` gate directly applies to the YAML/md edits
  — but DO actually run `cargo install cargo-nextest --version 0.9.137
  --locked` (or whatever exact command you land on) in this sandbox to
  confirm the version string and flags are valid and the install
  succeeds (or that this version is already installed and matches,
  which is fine too) — report the literal command and output.
- Validate the final `ci.yml` YAML is well-formed (e.g. `python3 -c
  "import yaml; yaml.safe_load(open('.github/workflows/ci.yml'))"` or
  equivalent — check the earlier SBOM CI task's brief for the exact
  Windows/Python invocation pattern used in this repo if `python3` isn't
  on PATH, e.g. `py -c "..."`).
- Report the exact syntax you researched/used for `taiki-e/install-action`
  version pinning and where you confirmed it's correct (a fetched doc
  page, a comment in the action's own repo, etc.) — do not just assert
  "this should work" without a real citation.
- Confirm both `ci.yml` call sites were updated (not just one).
