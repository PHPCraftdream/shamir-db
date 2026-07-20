# RI-5: Wire TypeScript client tests + Node napi e2e into CI

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

`ci.yml:106-111` explicitly documents that the Node.js e2e suite (under
`tests/e2e/`) is intentionally NOT wired into the per-PR gate ("too heavy...
Run it manually per `tests/e2e/README.md`; promoting it to a scheduled/
nightly workflow is a follow-up"). This task IS that follow-up — but the
investigation for this brief found the actual gap is BROADER than that one
comment suggests: read `crates/shamir-client-ts/package.json`'s `"test":
"vitest run"` script and `crates/shamir-client-ts/src/__tests__/` — there
are TWO distinct TS test surfaces, currently BOTH entirely absent from CI:

1. **Pure unit tests** (e.g. `src/core/builders/__tests__/*.test.ts`,
   `src/core/__tests__/framing.test.ts`, `hmac.test.ts`, etc.) — no server
   needed, run in milliseconds, cheap enough for the per-PR gate.
2. **TS client e2e tests** (`src/__tests__/e2e-*.test.ts`, ~25 files, plus
   the shared harness `src/__tests__/e2e-harness.ts`) — these spawn a REAL
   `shamir-server` subprocess and drive it over TCP/WS using the pure-TS
   client (no napi/native binding involved — read `e2e-harness.ts`'s
   `serverBinPath()`/`SERVER_AVAILABLE` logic). They self-skip cleanly via
   `describe.skipIf(!SERVER_AVAILABLE)` when no server binary is resolvable
   (`SHAMIR_SERVER_BIN` env → `CARGO_TARGET_DIR/release/<exe>` →
   `<repo>/target/release/<exe>`), so `npm test` is always GREEN even
   without a built server — it just skips those suites silently. This
   means these tests are currently not just "not run in CI" — a developer
   running `npm test` locally without first building the server gets a
   false-green signal for the whole e2e surface.

Separately, `tests/e2e/` (a DIFFERENT package, `tests/e2e/package.json`,
`"name": "shamir-e2e"`) drives the Node **native binding**
(`shamir-client-node`, the napi crate) through the same kind of flow. This
one genuinely needs the MSVC-only `shamir-client-node` build (`napi build
--platform --release`), which is why `ci.yml`'s comment calls it heavy.

Read `.github/workflows/stress-nightly.yml` in full — it's the house
convention for a scheduled workflow (`on.schedule` cron at an off-hour
minute, `workflow_dispatch: {}` for manual trigger, matching `env:` block,
heavy explanatory comments). Also read `.github/workflows/ci.yml`'s `test`
job for the toolchain-pin / `Swatinem/rust-cache@v2` conventions used
everywhere else in this repo's CI.

## The task

### 1. Add TS unit tests to the per-PR gate (`ci.yml`)

Add a new job to `.github/workflows/ci.yml` (e.g. `ts-unit`) that:
- Runs on `ubuntu-latest` only (pure TS, no OS-specific behavior expected —
  but check if any test relies on OS-specific paths and matrix if so).
- `actions/setup-node@v4` with a pinned LTS Node version (check
  `crates/shamir-client-ts/package.json`'s `engines` field if present for a
  version floor; otherwise pick a current LTS and pin it, e.g. `22`).
- `cd crates/shamir-client-ts && npm ci && npm run typecheck && npm test`.
- Since no `SHAMIR_SERVER_BIN` / server binary will exist in this job, the
  e2e-* suites self-skip automatically (per `SERVER_AVAILABLE` above) — this
  job exercises ONLY the pure unit tests, cheaply, on every PR. Confirm this
  behavior empirically (run it yourself locally first — see Verification).

### 2. Add TS e2e to a scheduled/nightly workflow

Create `.github/workflows/ts-e2e-nightly.yml` (new file, following
`stress-nightly.yml`'s conventions for the `on:` block and comment style):
- `on.schedule`: pick an off-hour cron slot distinct from
  `stress-nightly.yml`'s `23 4 * * *` and `supply-chain.yml`'s Sunday slot
  (check both files' cron expressions and choose a non-colliding time).
  `workflow_dispatch: {}` too, for on-demand runs.
- Steps: checkout, pinned Rust toolchain (`dtolnay/rust-toolchain@1.93.0`),
  `Swatinem/rust-cache@v2`, `cargo build --release -p shamir-server`
  (produces the binary `e2e-harness.ts` will auto-discover at
  `target/release/<exe>` — no `SHAMIR_SERVER_BIN` override needed if the
  default build path is used, but set `CARGO_TARGET_DIR` explicitly if this
  repo's CI convention does so elsewhere — check `ci.yml`/other workflows
  for whether `CARGO_TARGET_DIR` is set as an env anywhere and match that
  convention for consistency, otherwise rely on the default `target/release`
  path `e2e-harness.ts` already falls back to).
- Node setup (same version as step 1), `cd crates/shamir-client-ts && npm ci`.
- Run `npm test` — this time `SERVER_AVAILABLE` will be true (the binary
  exists), so the full e2e-* suite actually executes. Confirm by checking
  `e2e.test.ts`'s "skip reason" describe block (around line 1018) — it
  itself asserts `SERVER_AVAILABLE` — read it to understand the mechanism
  fully before wiring the workflow.
- Fail the job if any test fails (standard `vitest run` non-zero exit).

### 3. Add the Node napi e2e (`tests/e2e/`) to the SAME nightly workflow, OR a separate one

This is the heavier suite `ci.yml`'s comment already called out. Investigate
whether `shamir-client-node`'s napi build genuinely requires
`windows-latest` (MSVC) as the repo's other docs claim (`CLAUDE.md`
Workspace section: "napi-rs binding, MSVC-only on Windows — built
separately"; check `crates/shamir-client-node/rust-toolchain.toml` for a
pinned MSVC toolchain) or whether it can build on `ubuntu-latest`/
`macos-latest` too (napi bindings are typically cross-platform; the
MSVC constraint may be Windows-specific, not "Windows only ever"). Decide:
- If it can run on ubuntu-latest — add it as a second job in the SAME
  nightly workflow (simpler, one file).
- If it genuinely needs a Windows/MSVC runner — either add a
  `windows-latest` job to the same workflow (GitHub Actions supports mixed
  runners across jobs in one workflow file) or a separate
  `node-e2e-nightly.yml`. Follow `tests/e2e/README.md`'s documented steps
  (`npm run build` → `npm test`) exactly.

Whichever path: report your reasoning for the decision in your summary —
don't silently pick one without explaining why.

### 4. Cross-check `docs/guide-docs/` / `CONTRIBUTING.md` for stale claims

If any doc says "TS tests are not run in CI" or similar, sync it. Do not
rewrite unrelated prose.

## Out of scope

- Do NOT change `ci.yml`'s existing triggers (still push/PR to master) or
  remove the informational comment about `tests/e2e/` — update it to point
  at the new nightly workflow instead of describing an unaddressed gap.
- Do NOT touch `.github/workflows/release.yml` (task RI-4, already landed) —
  RI-4's brief explicitly scoped Node/TS e2e out of the release workflow;
  keep it that way.
- Do NOT attempt to fix any TS test that turns out to be failing/flaky when
  first wired in — if you discover a genuinely broken or flaky test while
  verifying locally, report it in your summary as a finding (with the exact
  failure) rather than silently fixing or skipping it; that's a separate
  follow-up decision for the orchestrator/user.

## Verification (MANDATORY before you report done)

- Locally run `cd crates/shamir-client-ts && npm ci && npm run typecheck &&
  npm test` WITHOUT a built server binary present, and confirm the e2e-*
  suites report as skipped (not failed) and the overall run is green. Report
  the literal test summary output (pass/skip counts).
- Build `shamir-server` release (`cargo build --release -p shamir-server`),
  then re-run `npm test` in `crates/shamir-client-ts` and confirm the e2e-*
  suites now actually execute (report the pass count difference vs the
  no-server run — this proves the self-skip mechanism and the real
  execution path both work as documented).
- Validate every new/modified YAML workflow file: parse as YAML (Node +
  `js-yaml` is available in this environment if not globally installed,
  `npm install js-yaml` locally is fine for a one-off check) and run
  `actionlint` if installed (it may need `go install
  github.com/rhysd/actionlint/cmd/actionlint@latest` — Go is available in
  this environment). Report literal actionlint output for `ci.yml` and any
  new/touched workflow file.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` clean (sanity check — these edits shouldn't touch Rust
  code, but confirm nothing else regressed).
- Report literal command output for all of the above.
