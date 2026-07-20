# RI-4: Tag-based release workflow — gate, multi-OS binaries, checksums, SBOM, GitHub Release

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits. **Never create or push a git tag** — that
is explicitly reserved for the user's own future action.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Task RI-4 of a release-infrastructure campaign. The workspace now versions
everything as `0.1.0-alpha.1` (task RI-3, already landed) and has a real
`CHANGELOG.md`. There is currently NO workflow that runs on a git tag push —
`.github/workflows/` has `ci.yml` (per-PR gate), `numa.yml`, `stress-nightly.yml`,
and `supply-chain.yml` (SBOM is `workflow_dispatch`-only today).

Read `.github/workflows/ci.yml` in full first — it's the house style to
match: `dtolnay/rust-toolchain@1.93.0` pinned toolchain, `env:
CARGO_TERM_COLOR: always` / `RUST_BACKTRACE: 1`, `actions/checkout@v6.0.3`,
`Swatinem/rust-cache@v2` for caching, matrixed OS jobs
(`ubuntu-latest, windows-latest, macos-latest`), and heavy explanatory
comments above each job (this repo's convention — every non-obvious choice
gets a comment explaining why, not just what).

Also read `.github/workflows/supply-chain.yml`'s `sbom` job (near the
bottom) — it ALREADY generates a CycloneDX SBOM per crate via
`cargo cyclonedx -f json --spec-version 1.5` and its own top-of-file
comment literally says: *"Cadence: workflow_dispatch-only today because
this project has not yet published a tagged release... When a real
release-tagging convention is established, add a tag trigger here"*. This
task IS that release-tagging convention — reuse that job's exact steps
(pinned `cargo-cyclonedx` version, output paths) rather than re-inventing
SBOM generation in the new workflow. Decide whether to (a) add a
`v*` tag trigger directly to `supply-chain.yml`'s existing `sbom` job, or
(b) call it from the new release workflow via a separate SBOM step that
mirrors it — prefer (a), triggering the existing job, if `workflow_call` /
tag-trigger composition is straightforward; otherwise duplicate the
job body into the new workflow and note in a comment why.

## The task

Create `.github/workflows/release.yml`, triggered on tag push matching
`v*` (e.g. `v0.1.0-alpha.1`):

```yaml
on:
  push:
    tags:
      - 'v*'
```

### 1. Gate job (must pass before any artifact builds)

Re-run the same checks as `ci.yml`'s `fmt` + `clippy` + `test` jobs (you can
either duplicate them inline in `release.yml`, or investigate GitHub
Actions' `workflow_call` reusable-workflow pattern to invoke `ci.yml`'s jobs
directly — prefer reuse if it's not significantly more complex; document
whichever choice you make and why). All downstream jobs (`build`, `docker`,
`sbom`, `github-release`) must `needs:` this gate so a red gate blocks the
whole release.

### 2. Multi-OS binary build

Matrix job building `shamir-server` release binaries for:
- `x86_64-unknown-linux-gnu` (ubuntu-latest)
- `aarch64-apple-darwin` (macos-latest — check what runner arch GitHub
  Actions currently provides; cross-compile via `--target` if the runner
  itself is a different arch)
- `x86_64-pc-windows-msvc` (windows-latest)

Use `dtolnay/rust-toolchain@1.93.0` (matching `ci.yml`'s pin) +
`Swatinem/rust-cache@v2`. Build with `cargo build --release -p shamir-server
--locked`. Package each binary into a `tar.gz` (Unix) / `.zip` (Windows)
archive named `shamir-server-<tag>-<target>.{tar.gz,zip}`, alongside a
`sha256sum`-generated `.sha256` checksum file per archive. Upload each as a
build artifact (`actions/upload-artifact@v4`) so a later job can collect
them.

### 3. Docker image build + smoke test

Uses the Dockerfile fixed in task RI-2 (`deploy/Dockerfile`, now pinned to
`rust:1.93-bookworm`, `COPY src ./src` removed — verify these fixes are
present, they should already be committed). On `ubuntu-latest` (Docker is
native there):

- `docker build -f deploy/Dockerfile -t shamir-db:${{ github.ref_name }} .`
- Start a container from it (mount a scratch volume for `/var/lib/shamir-db`,
  a minimal generated `server.ktav` config for `/etc/shamir/server.ktav` —
  check `deploy/server.example.ktav` for the minimal required fields).
- Poll `http://127.0.0.1:9090/healthz` (the same endpoint the Dockerfile's
  own `HEALTHCHECK` uses) until it returns 200 or a timeout (e.g. 30s).
- Stop the container gracefully (`docker stop`, allowing its SIGTERM
  graceful-shutdown path — check `main.rs`/service code for how shutdown is
  triggered) and check the exit code / logs for a clean shutdown, not a
  forced kill.
- Fail the job if any step fails. This is the P0#2 Docker-smoke-test gap
  the release review called out — implement it for real, not as a stub.

### 4. SBOM (reuse/trigger the existing job — see Context above)

### 5. GitHub Release

A final job, `needs:` the gate + build + docker + sbom jobs, that:
- Downloads all the binary-archive + checksum artifacts.
- Creates a GitHub Release for the pushed tag (`softprops/action-gh-release`
  or the `gh release create` CLI — pick whichever is simpler / already used
  elsewhere in this org's workflows if you find a precedent, otherwise
  `softprops/action-gh-release@v2` is a common, well-maintained choice).
- Attaches: all binary archives + their `.sha256` files, and the SBOM
  `.cdx.json` files.
- Release notes: extract the relevant `CHANGELOG.md` section for this
  version (or link to the compatibility statement / CHANGELOG.md directly
  if automatic section-extraction is fragile — don't over-engineer a
  changelog-parser; a simple heading-grep is fine, and falling back to "see
  CHANGELOG.md" is an acceptable degradation).
- Marks the release as a **pre-release** (GitHub's `prerelease: true` flag)
  since this is an alpha — do not let it default to a "latest" full release.

### Artifact signing (nice-to-have, not blocking)

If `cosign`/sigstore keyless signing (via GitHub OIDC) is straightforward
to wire in for the binary archives, add it as an additional step. If it
adds meaningful complexity (key management, extra secrets), skip it and
leave a `TODO` comment in the workflow explaining what's missing and why —
do not block the rest of the task on this.

## Out of scope

- Do NOT create or push any git tag (`git tag`, `git push --tags`) —
  verify no such command appears anywhere in your session, this is purely
  authoring the workflow file.
- Do NOT modify `ci.yml`'s triggers (still push/PR to master only).
- Do NOT attempt to actually trigger this workflow (no tag exists yet to
  fire it) — validate it via YAML linting / `actionlint` if available
  locally, or careful manual review against `ci.yml`'s proven patterns.
- Node/TS e2e in CI is a SEPARATE task (RI-5) — do not wire it into this
  release workflow.

## Verification (MANDATORY before you report done)

- The new `.github/workflows/release.yml` file is valid YAML (parse it,
  e.g. with `python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/release.yml'))"`
  if Python+PyYAML is available, or any equivalent local YAML validator —
  report what you used).
- If `actionlint` is installed or easily installable without heavy setup,
  run it against the new file and report results; if not available, say so
  plainly rather than skipping verification silently.
- Cross-check every job's `runs-on` / action versions / toolchain pin
  against `ci.yml`'s existing conventions for consistency (pinned action
  versions, not floating tags like `@latest`, matching this repo's
  supply-chain discipline).
- If you touched `supply-chain.yml` to add a tag trigger, run
  `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` as a sanity check that nothing else regressed (workflow
  YAML changes shouldn't affect these, but confirm).
- Report literal output for everything above, plus a summary of every job
  in the new workflow and what it does.
