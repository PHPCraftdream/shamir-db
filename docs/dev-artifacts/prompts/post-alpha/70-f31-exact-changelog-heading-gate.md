# Brief for F-31 (#824, P1, small) — changelog gate must require the EXACT tag heading

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

`.github/workflows/release.yml`'s `version-consistency` job has a "Check
CHANGELOG.md has appropriate content" step (~line 352-367) that currently
passes if CHANGELOG.md has EITHER an `[Unreleased]` section OR ANY
`## [x.y.z...]` version heading — not necessarily the heading matching the
tag actually being pushed. The job's own extensive comment (~line 301-314)
explains this was a deliberate looser choice made because, AT THE TIME,
CHANGELOG.md had ONLY `[Unreleased]` (no per-version heading existed yet),
and a strict check would have permanently failed until a human added the
heading in the tagging commit.

**That condition no longer holds.** F-14 (#804, already landed) created a
real `## [0.1.0-alpha.1] - 2026-07-26` section in CHANGELOG.md (verify:
`grep -n '^## \[' CHANGELOG.md` — currently shows both `[Unreleased]` and
`[0.1.0-alpha.1]`). The loose fallback is now a liability: a future
`v0.1.0-alpha.2` tag would satisfy today's check purely because the STALE
`[0.1.0-alpha.1]` heading still exists in the file, even if nobody ever
added a `[0.1.0-alpha.2]` section — the tag could ship with the WRONG
(previous release's) notes silently, and `github-release`'s own "Extract
release notes" step (~line 717+, heading-grep for `## [<version>]` with
the tag's version, leading `v` stripped) would then also silently extract
nothing matching (or worse, fall through to its generic
"Pre-release build" fallback) without the earlier gate ever catching that
the changelog wasn't actually updated for THIS tag.

## The fix

Tighten the check to require the EXACT heading for the tag being pushed:
`## [${TAG_VERSION}]` (reuse the SAME `TAG_VERSION` variable the
`version-consistency` job's first step already computes —
`${GITHUB_REF_NAME#v}` — check whether it needs to be re-derived in this
second step's own `run:` block, since each `run:` step is a separate
shell invocation in GitHub Actions and shell variables don't persist
across steps unless exported via `$GITHUB_ENV` or recomputed).

```bash
TAG_VERSION="${GITHUB_REF_NAME#v}"
if grep -qE "^## \[${TAG_VERSION}\]" CHANGELOG.md; then
  echo "OK: CHANGELOG.md has a heading for ${TAG_VERSION}"
else
  echo "::error::CHANGELOG.md has no '## [${TAG_VERSION}]' heading — add release notes for this tag before pushing it"
  exit 1
fi
```

(Escape `TAG_VERSION` correctly for `grep -E` — a version string like
`0.1.0-alpha.1` contains `.` which is a regex metacharacter matching any
character; check whether the existing codebase's convention elsewhere in
this workflow already handles this via `grep -F`/literal matching, or
whether the imprecision of `.` matching "any character" is acceptable
here given real version strings won't collide with anything unintended —
use judgment, but note your choice explicitly.)

Update the step's own comment (~line 352-357) to describe the NEW
stricter behavior, and update the job-level comment block (~line 301-314)
that currently explains the OLD looser choice — either remove that
rationale entirely (replaced by the new one) or keep a brief historical
note ("previously looser, tightened once F-14/#804 landed a real
per-version heading — see #824") — use judgment on which reads better in
context, but don't leave stale reasoning that no longer matches the code.

## Constraints

- Do NOT touch the `version-consistency` job's FIRST step (crate-version
  ↔ tag consistency check) — only the CHANGELOG check step.
- Do NOT touch `github-release`'s "Extract release notes" step — it
  already has its own fallback logic; this task only makes the EARLIER
  gate (in `version-consistency`) catch the case that step's fallback was
  silently absorbing.
- This is a CI workflow file — there's no `cargo test`/`fmt`/`clippy` gate
  for it. Verify correctness by reading the YAML carefully (shell syntax,
  variable scoping across steps) rather than by running it (this repo has
  no local GitHub Actions runner setup to test against) — if you want an
  extra confidence check, you may manually reason through 2-3 concrete
  scenarios (tag `v0.1.0-alpha.1` with the CURRENT CHANGELOG.md → pass;
  a hypothetical future tag `v0.1.0-alpha.2` with NO new heading added →
  fail; that same future tag WITH a `[0.1.0-alpha.2]` heading added →
  pass) and state the reasoning in your summary.

## Verification the orchestrator will run

Read-through of the diff for shell/YAML correctness — no automated CI
dry-run available locally for this specific workflow file.
