# Brief: CR-B9 — release workflow: full gate before tag release (#775)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

This is a **CI/CD workflow-only** task — no Rust or TS source changes.
Edit `.github/workflows/release.yml` only (read `.github/workflows/ci.yml`
and `.github/workflows/ts-e2e-nightly.yml` for content to copy from, but
do not modify either of those files).

## Problem — verified against the current tree 2026-07-23

`.github/workflows/release.yml`'s tag-triggered pipeline gates every
downstream job (`build`, `docker`, `sbom`, `sign`, `github-release`) on
only three jobs: `fmt`, `clippy`, `test` (lib tests, ~lines 60-100). The
regular `ci.yml` ALSO runs an `integration` job (`kind(test)` — everything
under `crates/*/tests/*.rs`, ~ci.yml lines 119-143) and a `ts-unit` job
(TS builder/type unit tests, ~ci.yml lines 165-176), but NEITHER is a
dependency of the release pipeline. A tag can therefore be pushed and
fully released even when the commit's integration suite or TS unit suite
was never actually green (they might have failed on the corresponding PR
and been ignored, or simply never run against this exact commit).

Additionally, nothing today verifies the pushed tag actually matches the
version the workspace crates declare, or that a corresponding CHANGELOG
entry exists — a tag typo (`v0.1.0-alph.1`) or a forgotten version bump
could release a build whose reported version is silently wrong.

## Fix 1 — add `integration` and `ts-unit` as release gates

Copy `ci.yml`'s `integration` job (~lines 119-143) and `ts-unit` job
(~lines 165-176) into `release.yml` VERBATIM (same matrix, same run
commands, same comments explaining WHY each is shaped the way it is —
these comments are load-bearing documentation, don't drop them). Add both
new job names to the `needs:` list of every job that currently depends on
`[fmt, clippy, test]` in `release.yml` (`build`, `docker`, `sbom`, `sign`,
and the final `github-release`'s explicit list) — a red integration or TS
suite must block the whole release exactly like a red fmt/clippy/test
does today.

**Runner-cost note**: `ci.yml`'s `integration` job matrixes across all
three OSes (ubuntu/windows/macos), same as `test`. Copying it verbatim
means the release gate now also runs integration on all three OSes. This
IS the "reuse verbatim, don't invent variants" instruction taking
precedence over a cost optimization — do not narrow the matrix to save
runner minutes unless you have a strong reason; if you DO decide to
narrow it (e.g. full integration on ubuntu only + lib-only on the other
two, matching what some review drafts floated as "an acceptable
documented tradeoff"), you must add an explicit workflow comment stating
the tradeoff and why you chose it over verbatim reuse. Prefer verbatim
reuse as the default.

## Fix 2 — TS e2e against the built release artifact (do it if cheap; else a TODO)

Per the review, wiring the real TS e2e suite (`e2e-cursors.test.ts`,
`e2e-cursor-lifecycle.test.ts`, etc. — the suites `ts-unit` deliberately
skips because no server binary is present, see `ci.yml`'s comment on
`ts-unit`) against a genuinely-built release binary is preferable to
`ts-unit`'s unit-only coverage, for the artifact that's actually about to
ship. `.github/workflows/ts-e2e-nightly.yml`'s `ts-e2e` job (top of the
file) already does exactly this — a self-contained job that: checks out,
installs the Rust toolchain, `cargo build --release --locked -p
shamir-server` (default target — NOT `--target <triple>`, so the binary
lands at the DEFAULT `target/release/shamir-server` path that
`e2e-harness.ts`'s `serverBinPath()` fallback resolves with no
`SHAMIR_SERVER_BIN` override needed), sets up Node 22, then runs `npm
test` (which now exercises the full e2e suite since `SERVER_AVAILABLE`
becomes true).

Add an analogous job to `release.yml` (e.g. `ts-e2e`), `needs: [fmt,
clippy, test]` (same as the other gates — it doesn't need `integration`/
`ts-unit` to have passed first, they're independent), and add `ts-e2e` to
the downstream `needs:` lists alongside `integration`/`ts-unit`. This is a
SEPARATE, SELF-CONTAINED release build (its own `cargo build --release`
step, default target) — do NOT try to share the matrixed `build` job's
per-target artifacts here; that job builds to `target/<triple>/release/`
paths and downloading+renaming a cross-job artifact into the default path
the harness expects is more complexity than a second straightforward
build step costs. If, after reading `ts-e2e-nightly.yml` in full, you
judge this wiring is NOT actually cheap for some reason you discover
(e.g. a secrets/environment dependency `ts-e2e-nightly.yml` relies on that
isn't available in a tag-push context), leave a clearly-marked `# TODO:
CR-B9 follow-up —` comment explaining exactly what blocks it, rather than
silently omitting the job.

## Fix 3 — tag ↔ version consistency check

Add a new job (e.g. `version-consistency`), `needs: []` (no dependencies
— this is a fast, cheap check that should fail FAST, so don't gate it
behind the slower jobs; but DO add it to the downstream `needs:` lists of
`build`/`docker`/`sbom`/`sign`/`github-release` so a mismatch blocks the
release exactly like the other gates). Verified facts to build this
check against:

- Every crate's `Cargo.toml` in this workspace currently declares the
  SAME `version = "0.1.0-alpha.1"` (confirmed via `grep -rn "^version"
  crates/*/Cargo.toml` — 24 crates, one unique value). There is no
  `[workspace.package] version` field (root `Cargo.toml` has no top-level
  `version` at all — it's a pure `[workspace]` manifest). The check
  should therefore: (a) collect every crate's declared version via
  whatever your script step finds easiest (grep, or `cargo metadata
  --format-version=1 | jq` if `jq` is available on the runner — check
  what other workflow steps in this repo already use for JSON parsing
  before picking a tool), (b) assert they are ALL identical (catches a
  crate that got its version bumped independently by mistake — a
  pre-existing-but-latent bug class this check should also guard
  against, not just the tag mismatch), (c) assert that shared version
  equals `${GITHUB_REF_NAME#v}` (the tag with its leading `v` stripped,
  same pattern `github-release`'s existing "Extract release notes" step
  already uses, ~near the end of the file).
- `CHANGELOG.md` currently has ONLY an `[Unreleased]` section (no
  per-version `## [x.y.z-alpha.N]` heading exists yet — this will be true
  for the FIRST real tag too, since nothing has cut a release yet). The
  existing `github-release` job's "Extract release notes" step (near the
  end of `release.yml`) already has fallback logic for exactly this case
  (falls back to a generic "Pre-release build of ${GITHUB_REF_NAME}"
  note when no matching heading is found) — **do NOT make this new
  check's CHANGELOG requirement stricter than that existing, intentional
  fallback allows**. Read the task's own wording carefully: it wants "a
  matching CHANGELOG section/mention" — interpret this loosely enough
  that today's `[Unreleased]`-only state does not itself fail the CI
  check (a hard requirement for a `## [<version>]` heading to exist would
  make CI unable to ever pass until someone manually adds that heading in
  the SAME commit being tagged — reasonable, but decide deliberately: if
  you want to REQUIRE the heading exists (stricter, catches "forgot to
  cut a CHANGELOG section" — arguably the more useful check), state that
  choice explicitly in a workflow comment and accept that the first real
  tag push will need a preparatory commit with the heading already in
  place. If you prefer the looser "at minimum the string `[Unreleased]` OR
  a version heading exists somewhere in CHANGELOG.md" check, that's
  acceptable too — just be explicit and document the choice; this is a
  judgment call the brief deliberately leaves to you rather than
  over-specifying.
- Fail LOUDLY (`echo "::error::..."` plus a non-zero exit) on any
  mismatch — mirror the existing style already used in this file's
  `docker` job's failure paths (`echo "::error::..."` before `exit 1`).

## Fix 4 — leave everything else unchanged

Do NOT touch: the existing `fmt`/`clippy`/`test` gate, the Docker smoke
test, the SBOM job, artifact packaging/signing, or the final
`github-release` job's release-notes extraction / `softprops/action-gh-release`
step (beyond adding the new jobs to its `needs:` list). Do NOT add a
cursor-ACL-specific regression job — CR-A1's tests are ordinary
lib/integration tests already covered by the `test`/`integration` jobs
you're wiring in.

## Verification (this cannot be fully tested without pushing a tag)

- Validate the YAML is well-formed: run `actionlint` if it's available in
  this environment (check `which actionlint` or similar); if not
  available, do a careful manual review of indentation/job graph
  consistency instead — GitHub Actions YAML errors are otherwise silent
  until a real tag push.
- Confirm every new job is correctly wired into every downstream `needs:`
  list — grep the whole file for `needs:` after your edit and manually
  verify the graph: `fmt`/`clippy`/`test`/`integration`/`ts-unit`/`ts-e2e`/
  `version-consistency` should all feed into `build`, `docker`, `sbom`;
  `sign` needs those plus `build`; `github-release` needs all of the
  above plus `build`/`docker`/`sbom`/`sign`.
- In your final report, write out the dry-run reasoning explicitly: what
  would happen on the next real tag push, given the current repo state
  (all crate versions consistent at `0.1.0-alpha.1`, no per-version
  CHANGELOG heading yet) — would `version-consistency` pass or fail
  against a hypothetical `v0.1.0-alpha.1` tag, given the choice you made
  in Fix 3 above? State it plainly.

## Gate

No Rust/TS code changes, so `cargo fmt`/`clippy`/`./scripts/test.sh` are
not applicable — just confirm (via `git status`/`git diff`) that ONLY
`.github/workflows/release.yml` was touched, nothing else in the repo.
