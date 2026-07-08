Task: MEDIUM-HIGH compliance — add automated supply-chain gating
(`cargo-deny` + `cargo-audit`) to CI, add a `SECURITY.md`
vulnerability-disclosure policy, and fix the non-reproducible absolute
local path pin on the `captrack` dependency (audit findings C1 + C4,
`docs/audits/2026-07-06-security-compliance-supplychain.md`).

## Where

- Workspace root `Cargo.toml:17`: `captrack = { path =
  "D:/dev/rust/captrack", features = ["telemetry"] }` — an ABSOLUTE
  LOCAL PATH that only resolves on the audit author's machine. Any
  other checkout (CI, another contributor, a release build) cannot
  build the workspace at all. `crates/shamir-db/Cargo.toml:72` and
  `crates/shamir-server/Cargo.toml:129` both reference it via
  `captrack = { workspace = true }`.
- `.github/workflows/ci.yml` (existing CI — confirm current structure
  before adding a new job) — no `cargo-deny`/`cargo-audit` step exists.
- No `deny.toml` (or `audit.toml`/`.cargo/audit.toml`) exists anywhere
  in the repo.
- No `SECURITY.md` exists (root, `.github/`, or `docs/`).

## Why this matters (audit context, do not re-litigate — just fix)

- **C1 (HIGH)**: no automated gate catches new RUSTSEC advisories on
  transitive dependencies, license drift (200+ transitive deps
  unchecked for copyleft/incompatible licenses — relevant for a
  product shipped as a single binary), or yanked/duplicate-version
  crates. The existing pre-commit/pre-push gate (fmt/clippy/test) does
  not cover any of this.
- **C4 (MEDIUM)**: no `SECURITY.md` / vulnerability-disclosure policy —
  an industry-baseline expectation for a database product.
- The `captrack` path dependency is a standalone reproducibility bug:
  the workspace literally cannot build outside the audit author's
  machine as currently configured.

## Fix

### 1. Fix the `captrack` path dependency (do this FIRST — it may block
   `cargo deny`/`cargo audit` from even running if the workspace can't
   resolve)

1. Determine what `captrack` actually is — check if it's:
   - Published on crates.io (if so, pin an exact version:
     `captrack = { version = "=X.Y.Z", features = ["telemetry"] }`).
   - A git dependency the author intends to keep private/internal (if
     so, pin to a git URL + specific rev/tag:
     `captrack = { git = "https://...", rev = "<sha>", features =
     ["telemetry"] }`).
   - Genuinely only available as a local path in this environment (if
     you cannot determine a git URL or crates.io publication and there
     is no way to resolve this without guessing — per this repo's
     CLAUDE.md rule on never adding an unverified dependency without
     confirming its existence, **DO NOT GUESS a URL**. If the actual
     source cannot be determined from the repo's own history/docs
     (check `git log` for when `captrack` was added, check for any
     README/comment mentioning its origin), STOP and report this as a
     blocker in your final report rather than inventing a path/URL —
     ask the orchestrator to confirm captrack's real location before
     you proceed with this specific sub-item. Do NOT skip the REST of
     this task (deny.toml/CI/SECURITY.md) because of this blocker —
     complete everything else and clearly flag the captrack pin as
     unresolved/needs-human-input in your report.

### 2. Add `deny.toml` at the workspace root

Create a `deny.toml` with (at minimum) these sections:
- `[advisories]`: fail on any known RUSTSEC advisory affecting a
  workspace dependency (deny, not just warn, for the CI gate to be
  meaningful) — check `cargo-deny`'s current schema (this tool's
  config format has evolved across versions; use whatever the
  currently-pinnable `cargo-deny` version's schema actually expects —
  do not blindly copy an outdated example).
- `[licenses]`: an allow-list covering `MIT`, `Apache-2.0`, `BSD-2-Clause`,
  `BSD-3-Clause`, `Unicode-DFS-2016` (or whatever the current
  Unicode-license identifier is called in recent `cargo-deny`/SPDX
  data — verify), `ISC`, `Zlib` — per the audit's suggested allow-list.
  Run `cargo deny check licenses` locally against the ACTUAL current
  dependency tree to discover what licenses are genuinely present
  before finalizing the allow-list — don't just copy the audit's
  suggested list blindly if the real tree needs something else (e.g.
  `CC0-1.0`, `MPL-2.0` for some common crates) — if you find
  additional licenses in use that are permissive/compatible, add them;
  if you find anything genuinely concerning (copyleft, unclear), flag
  it in your report rather than silently allow-listing it.
- `[bans]`: flag duplicate versions of the same crate (a common
  `cargo-deny` check) per the audit's suggestion — use `cargo-deny`'s
  standard bans-duplicates config; decide (and report) whether to set
  this to `deny` or `warn` for CI purposes — a `deny` here can be
  noisy in large dependency trees with legitimate duplicate major
  versions, so use judgment (the audit doesn't mandate a specific
  strictness here, just that duplicates are "flagged" — a `warn`
  level that's visible in CI output but doesn't fail the build may be
  the pragmatic choice; state your reasoning).
- `[sources]`: standard `cargo-deny` unknown-registry/unknown-git
  bans, allowing crates.io as the sole registry (plus explicit
  allow-listing of `captrack`'s git source if you resolved it to a git
  dependency in step 1, or crates.io if published there).

Run `cargo deny check` locally against the actual current dependency
tree ONCE the `captrack` fix (or its documented blocker) is in place,
and iterate the `deny.toml` config until it passes cleanly (or until
remaining failures are genuine, real issues worth flagging rather than
config mistakes) — report the final `cargo deny check` output.

### 3. Add a CI step running `cargo deny check` + periodic `cargo audit`

1. In `.github/workflows/ci.yml` (or a new dedicated workflow file if
   that fits the existing structure better — check how other checks
   are organized in the current CI setup and match the established
   pattern), add a job/step that:
   - Installs `cargo-deny` (via `cargo install cargo-deny --locked` or
     a maintained GitHub Action if one is already idiomatically used
     elsewhere in this CI setup for similar tool installs — check
     existing patterns in `ci.yml`/`numa.yml` first).
   - Runs `cargo deny check` and fails the job on any violation.
2. Add `cargo-audit` too, per the audit's explicit ask ("периодический
   `cargo audit`") — this can be a SEPARATE, less-frequent trigger
   (e.g. a scheduled/cron GitHub Actions workflow, since RUSTSEC
   advisories change over time independent of code pushes) rather than
   running on every single push/PR — use your judgment on the
   appropriate trigger (a nightly/weekly schedule is the audit's
   apparent intent given the word "периодический"/periodic) and
   document your choice in the report.

### 4. Add `SECURITY.md`

Create `SECURITY.md` at the repo root with, at minimum:
- A vulnerability-disclosure contact (use whatever contact mechanism
  is appropriate for this project — check if there's an existing
  contact convention elsewhere in the repo's docs, e.g. an email or a
  GitHub issue-based process; if genuinely nothing exists to reference,
  use a placeholder like "please open a private security advisory via
  GitHub" — a standard, low-friction default for a project hosted on
  GitHub — and note in your report that this may need a real contact
  address from the project owner).
- A supported-versions table/section (even a minimal one stating "only
  the latest commit on `master` is supported" is acceptable for a
  pre-1.0/internal-stage project — match the project's actual release
  maturity; check if there are version tags/releases to reference, or
  state there are none yet).
- A brief statement of scope (what counts as a security issue for this
  project — e.g. the WASM sandbox escape class, auth/ACL bypasses,
  memory-safety issues in `unsafe` code, supply-chain concerns).

## Verification requirement (no TDD — this is CI/config/docs)

This task changes NO application logic. Instead of Red/Green:
1. Run `cargo deny check` locally and show the full output (should be
   clean, or show exactly what genuine issues remain and why they're
   acceptable/tracked).
2. Confirm the workspace still builds cleanly after the `captrack` fix
   (`cargo build --workspace` or equivalent) — UNLESS you hit the
   captrack-source-unknown blocker, in which case state clearly that
   this verification step could not be completed and why.
3. Show the new CI workflow YAML diff and confirm it's syntactically
   valid (e.g. via `actionlint` if available, or at minimum confirm
   the YAML parses and matches the existing file's structural
   conventions).
4. Run the existing full gate (fmt/clippy/test) to confirm nothing
   else broke:
   ```
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets -- -D warnings
   ```
   (Skip `cargo t`/full test suite unless the `captrack` fix could
   plausibly affect runtime behavior — it shouldn't, since it's purely
   a dependency-resolution change, but note if you have any reason to
   run tests anyway.)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits. Do NOT add any dependency whose
existence/location you have not verified — per the captrack
instructions above, STOP and flag rather than guess if its true source
is undeterminable.

## Report format

When done, report exactly:
- What you determined `captrack`'s real source to be (git URL + rev,
  or crates.io version), OR a clear statement that this could not be
  determined and needs human input, with what you checked (git log,
  any docs) before concluding this.
- The final `deny.toml` content and the `cargo deny check` output.
- The exact CI workflow diff added (which file, what job/step).
- Your reasoning for `cargo-audit`'s trigger schedule (periodic vs.
  every-push) and the `[bans]` duplicate-version strictness choice.
- The `SECURITY.md` content added.
- Full verification results (build, deny check, fmt/clippy).
