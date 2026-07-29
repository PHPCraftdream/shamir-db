# Brief for F-54 (#865, P2) — remove group_commit.rs's unreachable group-batching path

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. **User decision (explicit, this
session): remove/feature-gate the dead code — do not wire group-commit
batching into production.** `crates/shamir-engine/src/tx/group_commit.rs`
(705 lines) has zero production call sites — confirmed by grep across
`crates/shamir-engine/src` (excluding `tests/`): the only references to
`run_leader` / group-batching are the module itself, a comment in
`finalize.rs`, and its own test file. `commit_tx_inner`
(`commit.rs:~514,533`) always routes to either
`commit_tx_inner_legacy_async` or `commit_tx_lockfree` — never to
`group_commit.rs::run_leader`.

**Why this matters beyond dead code hygiene**: this session's F-46
(commit `57382bab`) extended `group_commit.rs`'s inter-batch phantom check
with RI-barrier logic, as if it were a live commit route. It is not — the
correctness logic added there is currently duplicated maintenance burden
with zero runtime benefit, and future correctness work (F-46-style fixes)
would need to remember to touch THIS unreachable file too, for no reason.
Removing it eliminates that drift risk.

## What to do

1. **Delete `crates/shamir-engine/src/tx/group_commit.rs`** (705 lines) and
   its `pub(crate) mod group_commit;` declaration in `crates/shamir-engine/src/tx/mod.rs`.
2. **Delete `crates/shamir-engine/src/tx/tests/group_commit_tests.rs`**
   (82 lines) and its `pub mod group_commit_tests;` registration in
   `crates/shamir-engine/src/tx/tests/mod.rs`.
3. **Fix `finalize.rs`'s module doc comment** (`:1-15`) — it currently
   describes `post_publish_cleanup`'s shared tail as serving THREE commit
   paths: `commit_tx_lockfree`, `run_single_tx`, and "the `run_leader`
   batch loop". Since the latter two only exist in the file you're
   deleting, correct this comment to accurately describe what actually
   calls `finalize`'s shared tail today (confirm the real caller set by
   reading `finalize.rs` in full and grepping its actual call sites — do
   not just delete the stale names, replace them with what's true).
   Preserve the surrounding "Why `commit_tx_inner_legacy_async` is NOT a
   caller" explanation if it's still accurate (check).
4. **Correct `docs/guide-docs/KNOWN_LIMITATIONS.md:181`** — it currently
   lists `group_commit.rs`'s inter-batch phantom check alongside
   `pre_commit_locked_validate`/`pre_commit_locked` as one of the RI
   barrier's "commit-pipeline Phase 2-bis guard sites", implying all three
   are live. Since `group_commit.rs` is being removed, correct this to
   name only the two ACTUALLY-live guard sites, and if useful, note (one
   line) that a third, unreachable guard site existed in dead code and was
   removed along with that code (F-54) rather than silently dropping the
   count from 3 to 2 with no explanation.
5. **Do NOT touch `docs/dev-artifacts/research/f40b-ri-barrier-spike.md`**
   — that is a point-in-time spike memo (historical record of what was
   investigated/decided at the time), not a living doc; it accurately
   described the state of the code AS IT WAS when F-40b landed. Do not
   edit historical research artifacts to reflect later changes.
6. **Check for any other stray references** to `group_commit`, `run_leader`,
   or `run_single_tx` across the whole workspace (`grep -rn` from repo
   root, excluding `docs/dev-artifacts/research/` and `docs/checkpoints/`
   which are historical) that would need updating — report anything found
   and handle it, or note why it's fine to leave (e.g. another historical
   research doc).

## What NOT to do

- Do NOT feature-gate the code behind a Cargo feature flag as an
  alternative to deletion — the user's explicit choice was
  removal/feature-gate, and full removal is simpler and was the
  recommended option; only fall back to feature-gating if deletion turns
  out to be unexpectedly entangled with something still-needed (investigate
  first, but default to clean removal).
- Do NOT touch `commit_tx_lockfree`, `commit_tx_inner_legacy_async`, or any
  of the LIVE commit-pipeline code in `commit.rs`/`pre_commit.rs` — this
  task removes ONLY the unreachable `group_commit.rs` module and its own
  tests, plus the stale doc references pointing at it.
- Do NOT touch F-46/F-47/F-48/F-48b/F-49/F-50/F-51/F-52/F-53's landed code
  from earlier this session.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean —
  in particular, confirm removing the module doesn't leave any now-dead
  imports or now-unused `pub(crate)` items elsewhere that clippy would
  flag.
- This is a deletion-only task — no new tests are expected, but run the
  full suite to confirm nothing outside `group_commit.rs`/its own test
  file depended on it (should be a clean, silent removal given the
  confirmed zero production call sites).
- Clean up any scratch/debug log files you create in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
```

When done, give your final summary as plain text: confirmation of what
was deleted (files + line counts), the corrected `finalize.rs` doc
comment's new wording, the corrected `KNOWN_LIMITATIONS.md` wording, any
other stray references found and how you handled them, and confirmation
fmt/clippy/tests are clean.
