# Documentation Accuracy 6b — replace raw `cargo test` with the nextest wrapper in README/CONTRIBUTING/CLAUDE.md

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

Second item of "Этап 6 — Documentation accuracy"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 09. This project migrated to a `cargo-nextest`
wrapper (`./scripts/test.sh` / `cargo t` / `cargo tl` aliases) and BLOCKED
raw `cargo test` outright via a perimeter guard in `.cargo/config.toml`
(gated on the `$NEXTEST` env var that only `cargo nextest` sets). This is
extensively documented in `CLAUDE.md`'s own "🧪 Centralised test entry
point — MANDATORY" section (read it in full for the authoritative
explanation of why, and the exact wrapper command forms).

**The problem**: three files still show literal `cargo test ...` example
commands that predate this migration and will now FAIL if copy-pasted
verbatim (the guard refuses them with a printed error, not a silent
no-op):

1. `README.md` lines ~92-98 (the "🧪 Testing" section):
   ```
   # Full workspace test sweep (1178+ tests, ~90s)
   bash scripts/test-all.sh

   # Specific crate
   cargo test -p shamir-engine
   cargo test -p shamir-server
   ```
   The first line (`bash scripts/test-all.sh`) also needs verification —
   check whether `scripts/test-all.sh` still exists or was superseded by
   `scripts/test.sh` (search the repo). The "1178+ tests, ~90s" figure is
   also almost certainly stale given how much this campaign alone has
   added — decide whether to keep an approximate figure, update it, or
   drop the specific number in favor of prose (your call, but don't leave
   a number you haven't at least sanity-checked against a real recent
   test run count).

2. `CONTRIBUTING.md` lines ~5-9 (the "TL;DR — run this before every push"
   block, described as "the exact four jobs in `.github/workflows/ci.yml`"):
   ```
   cargo fmt --all -- --check                              # 1. formatting
   cargo clippy --workspace --all-targets -- -D warnings   # 2. lints
   cargo test  --workspace --lib                           # 3. unit tests
   cargo test  --workspace --test '*'                      # 4. integration tests
   ```
   **Verify this "exact four jobs" claim against `.github/workflows/ci.yml`
   as it exists TODAY** before rewriting — the actual CI `test`/
   `integration` jobs already run `./scripts/test.sh --locked` and
   `./scripts/test.sh --full --locked -E 'kind(test)'` respectively (NOT
   raw `cargo test`), so this block is doubly stale: wrong commands AND
   arguably no longer "exactly" the CI jobs (CI has more jobs today --
   `fmt`, `clippy` (3-OS matrix), `test` (3-OS matrix), `integration`
   (3-OS matrix), `cooldown`, plus whatever else `ci.yml` has grown to —
   read the whole file). Rewrite this block to show the real local-gate
   commands using the wrapper, and correct the "exact four jobs" framing
   to match reality (don't just swap the command text and leave a
   claim that's still wrong in a different way).

3. `CLAUDE.md`'s own "🧹 Code quality (MANDATORY)" → "Pre-commit gate"
   section:
   ```
   cargo fmt --all -- --check                            # formatting drift
   cargo clippy --workspace --all-targets -- -D warnings # lint regressions
   cargo test  --workspace --lib                         # behavioural tests
   ```
   This is the MOST important of the three fixes: `CLAUDE.md` is this
   project's own checked-in instructions, read by every future agent
   (including whichever agent picks up this exact task) as authoritative.
   Right now `CLAUDE.md` **contradicts itself** — its own later "🧪
   Centralised test entry point — MANDATORY" section says bare
   `cargo test` is "BLOCKED outright" with "NO escape flag", while its
   earlier "Pre-commit gate" section's own example command is exactly the
   thing that section says is blocked. Fix this self-contradiction.

## The task

1. In `README.md`'s "🧪 Testing" section: replace
   `cargo test -p shamir-engine` / `cargo test -p shamir-server` with the
   wrapper equivalents (`./scripts/test.sh -p shamir-engine` / `./scripts/test.sh
   -p shamir-server`, or `cargo tl -p <crate>` if that reads better in
   context — check which alias this project's own docs prefer in
   example blocks elsewhere, for consistency). Verify/fix the
   `scripts/test-all.sh` reference and the test-count figure as described
   above.
2. In `CONTRIBUTING.md`'s TL;DR block: replace both `cargo test` lines
   with the wrapper form (`./scripts/test.sh --lib` / `./scripts/test.sh
   --full`, or `cargo tl` / `cargo t` — pick whichever pairing most
   directly maps to "unit tests" vs "integration tests" per
   `CLAUDE.md`'s own description of what each alias does). Correct the
   "exact four jobs in `.github/workflows/ci.yml`" framing against the
   file's REAL current job list (read the whole file, don't guess).
3. In `CLAUDE.md`'s "Pre-commit gate" section: replace the
   `cargo test --workspace --lib` line with the wrapper form. This is the
   file most other sections of `CLAUDE.md` itself already treat as
   authoritative (e.g. "For sub-agents: every test step in an Agent brief
   MUST point at `./scripts/test.sh`... NEVER raw `cargo test`" appears
   later in the same file) — make the Pre-commit gate section consistent
   with that existing rule rather than inventing new phrasing.
4. While in each file, do a final grep pass for any OTHER stray raw
   `cargo test` example command you may have missed (the three sites
   above were found via `grep -n "cargo test"` across README.md,
   CONTRIBUTING.md, CLAUDE.md — re-run that grep yourself after your
   edits and confirm the only remaining `cargo test` mentions are
   EXPLANATORY PROSE about the ban itself, not copy-pasteable example
   commands).

## Out of scope

- Do NOT touch `.github/workflows/ci.yml`'s actual `run:` steps — they
  already correctly use the wrapper (`./scripts/test.sh --locked`,
  `./scripts/test.sh --full --locked -E 'kind(test)'`). You MAY note in
  your summary if you spot a stale comment/job-name mismatch there, but
  do not edit that file — it's out of scope for this brief.
- Do NOT touch `.cargo/config.toml` or `.config/nextest.toml` — the guard
  itself and its config are correct; only the DOCUMENTED example commands
  that reference the now-blocked raw form need fixing.
- Do NOT touch anything from the already-completed Этапы 1-5 or task 6a
  — this brief is scoped to the three raw-`cargo test`-reference files.

## Verification (MANDATORY before you report done)

- No `cargo test`/`clippy`/`fmt` gate applies in the traditional sense
  (docs-only change) — but DO actually run the wrapper commands you're
  now recommending (e.g. `./scripts/test.sh -p shamir-engine`, a quick
  `./scripts/test.sh --lib` or whatever subset you cite) to confirm they
  work as documented before you commit to that exact phrasing in three
  separate files — don't propose a command form you haven't verified
  actually runs.
- Report the final `grep -n "cargo test" README.md CONTRIBUTING.md
  CLAUDE.md` output and confirm every remaining hit is explanatory prose,
  not an executable example.
- Confirm what you found when checking `.github/workflows/ci.yml`'s
  actual current job list against CONTRIBUTING.md's "exact four jobs"
  claim, and how you resolved the discrepancy.
- Confirm what you found for `scripts/test-all.sh` (exists / doesn't
  exist / superseded) and how you resolved the README.md reference to it.
