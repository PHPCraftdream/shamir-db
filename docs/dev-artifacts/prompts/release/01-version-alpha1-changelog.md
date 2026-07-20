# RI-3: Version everything as 0.1.0-alpha.1, add publish=false, start CHANGELOG

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

This is task RI-3 of a release-infrastructure campaign, following an
external release-readiness review (2026-07-20/21). The user has EXPLICITLY
CONFIRMED the versioning scheme: **v0.1.0-alpha.1**, with a `CHANGELOG.md`
going forward. This decision is settled — do not re-litigate it.

The review found: every crate independently declares `version = "0.1.0"`
(verify: `grep -n "^version" crates/*/Cargo.toml` — 24 crates, no
`[workspace.package]` inheritance exists in the root `Cargo.toml`), while
`README.md:16` says "Version: 0.1.0 (Alpha)" and
`crates/shamir-client-ts/package.json` says `"version": "0.1.0"`. No crate
has `publish = false`, so every one of the 24 internal crates could
currently be accidentally published to crates.io with `cargo publish`.

**Note on "v0.10"**: `docs/dev-artifacts/research/2026-07-17-release-audit/`
uses "v0.10" as an internal project-milestone CODENAME for this audit round
(e.g. "release-readiness v0.10", "funclib top-up v0.10") — this is NOT a
package version and is unrelated to the `0.1.0-alpha.1` scheme. Do not
touch those internal audit docs; they are historical records of a planning
round, not a version claim about the shipped software.

`CHANGELOG.md` already EXISTS at the repo root (9 lines, minimal Keep-a-
Changelog-style with an `[Unreleased]` section) — read it first. Extend it,
don't replace its structure.

## The task

### 1. Bump every crate version

In all 24 `crates/*/Cargo.toml` files (verify the exact count/list via
`grep -rl "^version = \"0.1.0\"" crates/*/Cargo.toml`), change:

```toml
version = "0.1.0"
```
to:
```toml
version = "0.1.0-alpha.1"
```

This is valid Cargo/semver (pre-release identifier). Do NOT touch
`crates/shamir-client-node` or `crates/shamir-client-ts` differently from
the rest just because they're workspace-excluded — `shamir-client-node` has
its own `Cargo.toml` (bump it the same way); `shamir-client-ts` has no
`Cargo.toml` (handle its `package.json` separately, step 3).

### 2. Add `publish = false` to every crate

Same 24 `Cargo.toml` files: add a `publish = false` line to each `[package]`
section (near `version`). This is a deliberate safety net — nothing in this
workspace is ready for crates.io publication yet, and an accidental
`cargo publish -p <any-crate>` should refuse rather than succeed. This is
NOT a permanent restriction — when the project later decides a specific
crate (e.g. `shamir-client` or `shamir-sdk`) is ready for crates.io, that
crate's `publish = false` gets removed in a dedicated future change. For
now, ALL 24 get it.

### 3. Sync `crates/shamir-client-ts/package.json`

Change `"version": "0.1.0"` to `"version": "0.1.0-alpha.1"` (valid npm
semver pre-release form). Leave `"private": true` UNCHANGED — the TS
package stays unpublished for this alpha; that's a separate decision for
later, not something to revisit here.

### 4. Sync `README.md`

Line ~16 (`**Version:** 0.1.0 (Alpha)`) → `**Version:** 0.1.0-alpha.1`.
Grep the rest of `README.md` for any other bare `0.1.0` version mentions
and sync them too (don't touch the Rust-1.93.0 badge or unrelated version
numbers — only THIS project's own version string).

### 5. Extend `CHANGELOG.md`

Read the existing file first (it's short). Add, under (or restructuring)
the existing `[Unreleased]` section, a clear statement of:

a. **The versioning scheme**: this project uses `MAJOR.MINOR.PATCH-alpha.N`
   during the alpha phase; `alpha.N` increments do not imply any
   compatibility guarantee between them.

b. **Compatibility statement** (explicit, in plain language): storage
   format, wire protocol, and public API MAY change incompatibly between
   any two `0.1.0-alpha.N` releases. Upgrading between alpha versions may
   require an export/import cycle rather than an in-place upgrade — there
   is no supported in-place migration path yet during alpha. This
   statement should be prominent (not buried) since it's the single most
   important operational fact for an early adopter.

c. Keep the existing `[Unreleased]` bullet list of work-in-progress areas
   (don't delete it — this versioning/changelog setup itself is one more
   bullet under Unreleased, not a tagged release yet: no git tag is being
   created by this task, that's a separate later step gated on the user's
   explicit go-ahead per this repo's CLAUDE.md).

Follow Keep a Changelog (https://keepachangelog.com) conventions loosely
(the existing file already does) — Added/Changed/Fixed groupings are fine
if it helps organize, but don't over-engineer a 9-line file into a rigid
template it doesn't need yet.

### 6. Cross-check for stray version mentions

Grep `docs/guide-docs/` (the PUBLIC-facing guide docs, not
`docs/dev-artifacts/`) for any hardcoded `0.1.0` version strings that
should track the new scheme (e.g. install instructions, quickstart
snippets). Fix only clear, unambiguous version-string mentions — do not
rewrite prose around them.

## Out of scope

- Do NOT create a git tag. Do NOT run `cargo publish` for anything (real or
  dry-run against the registry). Tagging/publishing is task RI-13, gated on
  the user's explicit go-ahead.
- Do NOT touch `docs/dev-artifacts/research/2026-07-17-release-audit/*`
  (the "v0.10" codename files) — see the Context section above.
- Do NOT change `crates/shamir-client-ts/package.json`'s `"private": true`.
- Do NOT introduce a `[workspace.package]` + `version.workspace = true`
  refactor — that's a bigger structural change than this task's scope
  (bump the 24 files individually, as they already are individually
  declared).

## Verification (MANDATORY before you report done)

- `grep -rn "^version = \"0.1.0-alpha.1\"" crates/*/Cargo.toml | wc -l`
  should equal the count of crates you touched (list them in your
  summary).
- `grep -rln "publish = false" crates/*/Cargo.toml | wc -l` — same count.
- `cargo build --workspace` clean (version-only Cargo.toml edits shouldn't
  break anything, but confirm — this also validates the TOML syntax of
  every edited file).
- `./scripts/test.sh` (lib tests, all crates) green.
- `cargo fmt --all -- --check` clean.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above, plus the final
  `CHANGELOG.md` content and README.md's new version line in your summary.
