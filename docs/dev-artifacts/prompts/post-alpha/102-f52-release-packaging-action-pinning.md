# Brief for F-52 (#863, P1) — release archive self-containment, GitHub Actions SHA-pinning, Node package version-scope documentation

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. A 2026-07-28 review flagged three
release-infrastructure findings (P1-7/8/9), independently confirmed this
session against the current tree. This brief covers all three — they are
independent and can be done in any order within one session.

**Standing constraint (user's own global rule, non-negotiable):** this task
must NOT bump any crate/package version number anywhere in the repo. Where
the review's original wording suggested "synchronize the Node package
version," the correct resolution instead is to explicitly document why it
is out of scope — see finding 3 below. Do not touch any `version = "..."`
field in any `Cargo.toml`, nor `crates/shamir-client-node/package.json`'s
`"version"` field.

## Finding 1 — release archives contain only the binary, nothing else

`.github/workflows/release.yml`'s `build` job (~lines 428-480) packages
`shamir-server-<tag>-<target>.{tar.gz,zip}` containing ONLY the
`shamir-server` binary. Confirmed: `LICENSE-MIT` and `LICENSE-APACHE` exist
at the repo root; `README.md` exists at the repo root; a `.sha256` checksum
file is already generated per-archive (so checksum-verification
instructions are the missing piece, not the checksum itself).

**What to add to the archive** (stage them into the same `stage/` dir
before the `tar`/`Compress-Archive` step, so they end up inside the
archive alongside the binary):
- `LICENSE-MIT` and `LICENSE-APACHE` (repo root).
- `README.md` (repo root) — or, if it's large/dev-oriented, consider
  whether a smaller `deploy/README.md` (already a self-contained deploy
  quickstart) is more appropriate for an operator unpacking a release
  archive; use your judgement and state which you chose and why.
- A sample safe config: `deploy/server.example.ktav` (the base profile —
  see F-51, commit `883af6dc`, which just brought this into a correctly
  documented, conservatively-sized state).
- Checksum-verification instructions: a short `VERIFY.md` (or a section
  appended to whichever README you include) explaining `sha256sum -c
  <archive>.sha256` (Unix) / `Get-FileHash` comparison (Windows) — keep
  this SHORT, a few lines, not a full security doc.
- Third-party notices: check whether `cargo-about` or similar is already
  used anywhere in the workspace's CI (grep `.github/workflows/` and
  `Cargo.toml` for `about`/`license` generation tooling) before deciding
  whether to generate a THIRD_PARTY_LICENSES file at release time or
  whether one already exists to include as-is. If generating one from
  scratch would require adding a new tool/dependency, treat that as a
  separate, larger task — note it in your summary instead of implementing
  it, rather than scope-creeping this fix.

Update each OS's packaging step (`tar -czf`, `Compress-Archive`) to include
the new staged files. Keep the archive's internal layout flat and simple
(operator unzips and finds the binary + docs side by side, not buried in
subdirectories) unless there's a clear reason to nest.

## Finding 2 — GitHub Actions workflows use mutable version tags, not immutable commit SHAs

Confirmed current state (grep across `.github/workflows/*.yml`):
third-party actions referenced by mutable tag, appearing across
`ci.yml`, `release.yml`, `supply-chain.yml`, `stress-nightly.yml`,
`ts-e2e-nightly.yml`, `numa.yml`:
- `actions/checkout` — **inconsistently versioned across files**:
  `@v6.0.3` in `ci.yml`/`release.yml`, `@v5` in `supply-chain.yml`.
- `actions/setup-node@v4`
- `Swatinem/rust-cache@v2`
- `actions/upload-artifact@v4`
- `actions/download-artifact@v4`
- `taiki-e/install-action@v2`
- `sigstore/cosign-installer@v3`
- `softprops/action-gh-release@v2`

(`dtolnay/rust-toolchain@1.93.0` is NOT a supply-chain action-version
concern — that "version" is a Rust toolchain version input to a
first-party-maintained action, not a mutable action release tag; leave it
as-is.)

**What to do:** for every third-party action use-site listed above, pin to
the exact commit SHA that the CURRENTLY-referenced tag resolves to (you
have network/tool access to resolve `owner/repo@vX.Y.Z` → its commit SHA —
use `gh api repos/<owner>/<repo>/git/refs/tags/<tag>` or equivalent, or the
GitHub UI's "copy full SHA" for that tag's commit, whichever tool you have
available). Format each pin as:

```yaml
- uses: actions/checkout@<40-char-sha>  # v6.0.3
```

— i.e. the SHA is what's authoritative, with a trailing comment naming the
human-readable version it corresponds to (this is the standard
SHA-pinning convention GitHub itself documents, and lets Dependabot's
existing `github-actions` ecosystem entry (`.github/dependabot.yml`,
already configured with a 30-day cooldown) continue to propose version
bumps as SHA-updating PRs with the comment kept in sync).

**Normalize `actions/checkout` to ONE version** across all workflow files
before pinning it (pick whichever is newer/more current between `v6.0.3`
and `v5` — confirm via `gh` or the action's release notes which is
actually more recent; do not assume by number alone if there's any
ambiguity in how that action versions its major series).

**Permissions audit:** `release.yml` already declares explicit
`permissions:` blocks (3 jobs). `ci.yml`, `supply-chain.yml`,
`stress-nightly.yml`, `ts-e2e-nightly.yml`, and `numa.yml` currently have
**none at all** — confirm this with a fresh grep before changing anything.
For each of those files, add a top-level `permissions: contents: read`
(the GitHub-recommended least-privilege default for a workflow that only
reads the repo and runs tests/builds — do NOT grant `write` scopes unless
a specific job in that file genuinely needs one, e.g. anything that
pushes a commit, comments on a PR, or uploads to a registry; audit each
job's actual steps before deciding, and if a job needs a broader scope,
declare it at the JOB level, not blanket workflow-level, to keep the
default narrow). State in your final summary which files got a blanket
workflow-level permission and which (if any) needed a job-level exception,
and why.

## Finding 3 — release-state documentation: Node package version is out of the workspace version-consistency contract

Confirmed current state: `.github/workflows/release.yml`'s
`version-consistency` job (~line 321-352) greps `crates/*/Cargo.toml` for
`version = "..."` and requires every workspace crate to agree with each
other AND with the pushed tag. `crates/shamir-client-node/package.json`'s
`"version": "0.1.0"` is a DIFFERENT manifest format (Node, not Cargo) and
is never touched by this grep — it is currently, silently, outside this
consistency contract, and does not match the workspace's `0.1.0-alpha.1`.

**Do NOT bump `shamir-client-node/package.json`'s version to match** (see
the standing constraint above). Instead:
1. Add a comment inside the `version-consistency` job's script (right
   after the existing crate-version check, before the CHANGELOG check)
   explicitly noting that `shamir-client-node`'s `package.json` version is
   OUT OF SCOPE for this check — because that crate is excluded from the
   default workspace (`Cargo.toml`'s `exclude`, MSVC-only, built
   separately per the project's own `CLAUDE.md`) and is versioned/released
   independently on its own cadence (confirm this is actually true by
   checking whether there's any existing separate publish workflow for it,
   e.g. an napi-specific release job — grep for "napi" or
   "client-node" across `.github/workflows/`; if no separate release
   process exists for it AT ALL today, say so honestly in your summary
   rather than inventing one — documenting the current gap is enough for
   this task, building a new node-publish pipeline is out of scope here).
2. Add the same explanation as a short note in `CHANGELOG.md` or
   `RELEASE.md` if one exists (check first) — wherever release-process
   documentation already lives — so a future contributor doesn't
   rediscover this as a "bug."

## What NOT to do

- Do NOT bump ANY version number anywhere (Cargo.toml, package.json,
  CHANGELOG.md's Unreleased heading) — this task is documentation +
  workflow hygiene only.
- Do NOT create a git tag or trigger a release.
- Do NOT implement third-party-license generation from scratch if no
  existing tool/precedent is found in the workspace — note it as a
  follow-up instead.
- Do NOT redesign the release workflow's overall structure — this is a
  targeted hardening pass on the 3 findings above, not a rewrite.

## Constraints

- This task touches YAML workflow files and Markdown/config docs, not
  Rust code — there is no `cargo fmt`/`clippy`/`cargo t` gate that applies
  directly, but if you touch anything in `crates/` (you should not need
  to), run the normal gates on it.
- Validate every YAML file you touch is syntactically valid (e.g. `python3
  -c "import yaml, sys; yaml.safe_load(open(sys.argv[1]))" <file>` or
  equivalent, if a YAML parser is available in this environment — if not,
  carefully hand-check indentation).
- Do not actually run any GitHub Actions workflow (no way to trigger CI
  from a local session) — your job is to produce a correct, reviewable
  diff; the orchestrator will confirm the workflow files parse and the
  next real push exercises them.

## Verification the orchestrator will run

Manual review of the diff (YAML correctness, SHA-pin correctness spot-check
against a couple of the actions' actual tag→SHA mappings, permissions
sanity) — no automated test suite applies to this task.

When done, give your final summary as plain text: the exact archive
contents added (finding 1), the full list of action pins with their
SHA→version-comment mapping and which files changed (finding 2), the
permissions changes per workflow file and your reasoning for any job-level
exceptions, and the version-consistency job's new documentation comment
plus wherever else you added the Node-package-scope note (finding 3).
