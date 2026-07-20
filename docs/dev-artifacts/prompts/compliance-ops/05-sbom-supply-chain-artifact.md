# Compliance & Ops 5e — add SBOM artifact to the supply-chain CI workflow

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Fifth item of "Этап 5 — Compliance & Ops"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 03
(`docs/dev-artifacts/research/2026-07-17-release-audit/03-compliance-data-governance.md`,
capability-matrix row **7c** and the "P7 — No SBOM release artifact"
section):

> License/advisory *gating* is strong (`deny.toml` + CI), but no
> machine-readable SBOM (CycloneDX/SPDX) or `cargo-about` license report is
> generated or committed; nothing in `docs/` inventories the ~full
> dependency tree for release evidence... **Fix direction:** add a `cargo
> cyclonedx` (or `cargo-about`) step to the supply-chain workflow emitting
> an artifact **per tagged release**.

This is a CI/build-tooling change, not application code. Read
`.github/workflows/supply-chain.yml` in full first — it already
establishes this project's conventions for supply-chain jobs (two jobs
today: `deny` on every push/PR, `audit` on a weekly schedule + manual
dispatch; both documented with a rationale comment block at the top of the
file explaining WHY each cadence was chosen — mirror that documentation
style for your new job's own cadence choice).

## The task

1. **Pick the tool**: use `cargo cyclonedx` (the report's primary
   recommendation, and CycloneDX is the more directly-named format in the
   finding — `cargo-about` is more of a license-notices generator, a
   related but distinct concern already partially served by `deny.toml`'s
   license gate). If you find a compelling reason to prefer `cargo-about`
   or to run both, justify it explicitly in your summary; otherwise default
   to `cargo cyclonedx` alone.
2. **Add a new job** to `.github/workflows/supply-chain.yml` (do NOT
   replace or restructure the existing `deny`/`audit` jobs — add a third,
   following their exact style: `runs-on: ubuntu-latest`, pinned toolchain
   via `dtolnay/rust-toolchain@1.93.0` matching the other two jobs,
   `cargo install --locked <tool>` before running it).
3. **Trigger cadence** — per the report's "emitting an artifact per tagged
   release" guidance: trigger on git tag pushes (a `push: tags: ['v*']` or
   whatever this project's actual release-tag naming convention is — check
   for any existing release/tag workflow or `CHANGELOG.md`/git tag history
   to confirm the naming pattern before guessing) PLUS `workflow_dispatch`
   for manual/on-demand runs (mirroring the `audit` job's own manual-dispatch
   affordance). Do NOT run SBOM generation on every push/PR — that would
   be noisy and doesn't match "per tagged release" framing; if this
   project doesn't yet have a formal release-tagging convention, ask
   yourself (and state in your summary) whether `workflow_dispatch`-only
   (no automatic tag trigger) is the more honest choice until a real
   release process exists — don't invent a tag pattern that doesn't match
   how this project actually cuts releases.
4. **Generate + upload**: run `cargo cyclonedx` (check its actual CLI
   flags for output format/path — likely emits one `.cdx.json`/`.cdx.xml`
   per workspace member by default; decide whether workspace-root-only or
   per-crate output is more useful for "release evidence" of the whole
   product, and document your choice), then upload the result as a GitHub
   Actions build artifact via `actions/upload-artifact@v4` (check the
   `actions/checkout` version already pinned in this file, ~`@v5`, and use
   whatever `upload-artifact` major version is current/compatible — don't
   guess, check GitHub's own docs or an existing project convention if one
   exists elsewhere in `.github/`).
5. Update the workflow file's own top-of-file rationale comment block
   (mirroring the existing `deny`/`audit` explanation) to describe the new
   job's purpose and cadence, consistent with the file's existing
   documentation style.

## Out of scope

- Do NOT implement automatic SBOM attachment to a GitHub Release object
  (i.e. `gh release upload` / release-asset API calls) unless it's a
  trivial, obviously-correct addition once the artifact-upload step
  exists — the core deliverable is the SBOM ARTIFACT existing at all;
  attaching it to a formal release object is a nice-to-have, not required.
- Do NOT touch `deny.toml` or the existing `deny`/`audit` jobs' logic.
- Do NOT touch anything from the already-completed correctness-bug wave,
  concurrency-deadlock sweep, DDL-time-rejection/warn-log fixes, cleanup
  tails A/B/C, Этап 4's funclib top-up, or the already-completed Этап 5
  tasks (5a-5d) — this brief is scoped to the SBOM CI job only.

## Verification (MANDATORY before you report done)

- Since this is a CI-workflow file change, `cargo test`/`clippy`/`fmt`
  don't directly apply to `.yml` — instead, validate the YAML is
  well-formed (e.g. `python3 -c "import yaml; yaml.safe_load(open('.github/workflows/supply-chain.yml'))"`
  or an equivalent linter if one is available in this environment) and, if
  feasible in this sandbox, actually RUN the new job's commands locally
  (`cargo install --locked cargo-cyclonedx` + `cargo cyclonedx` against
  this workspace) to confirm the tool installs and produces real output
  before declaring the workflow step correct — a CI job you've never
  actually executed once is exactly the kind of thing that silently fails
  on first real run.
- Report the actual SBOM output you produced locally (file path, rough
  size/format) as evidence the command works against this real workspace,
  not just that the YAML parses.
- Confirm explicitly: did you find an existing release-tag naming
  convention to trigger on, or did you default to `workflow_dispatch`-only
  because none exists? State which, and why.
