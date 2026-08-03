# Brief — rename the `legacy` module to `base_index` (repo-wide)

Task: #973 in the session TaskList. Explicit user request (2026-08-03): the name "legacy" wrongly implies "old code from a previous release, safe to delete" — there has been NO release yet, and this module (`crates/shamir-index/src/legacy/`) is the actively-used FIRST-generation index implementation (hash/regular indexes, unique-constraint indexes, sorted indexes), distinct from `index2` (the second-generation fts/functional/vector system). The user has approved the new name: **`base_index`**.

This task runs AFTER #957/#958/#959/#960/#972 (all already committed, all touched files inside `crates/shamir-index/src/legacy/`) specifically so this rename doesn't race their edits. Verify `git log --oneline -10` shows those as already landed before you start.

## Scope: what to rename, what to leave alone

**Rename** (all of these currently say "legacy" meaning THIS module):

1. Directory: `crates/shamir-index/src/legacy/` → `crates/shamir-index/src/base_index/`.
2. `crates/shamir-index/src/lib.rs`: `pub mod legacy;` → `pub mod base_index;` (check for a re-export alias too, if one exists).
3. Every `use crate::legacy::...` (inside `shamir-index` itself) and every `use shamir_index::legacy::...` / `shamir_index::legacy::*` (from OTHER crates — `shamir-engine`, `shamir-db`, and check `shamir-server`/`shamir-client`/`shamir-tx` too, do not assume it's confined to `shamir-engine`) → `crate::base_index::...` / `shamir_index::base_index::...`.
4. Field names, local variables, function names, and test names that reference "legacy" AS A NAME FOR THIS MODULE'S CONTENT — e.g. (found during investigation, verify current names before renaming — some may already have shifted after #957-#972's edits):
   - `pre_commit.rs`'s `rederive_legacy_ops_post_stage` (from task #958) → `rederive_base_index_ops_post_stage` (or similar — keep it readable, this is a judgment call, just don't leave "legacy" in it).
   - `TxContext::legacy_stage_gens` / `note_legacy_stage_gen` (from #958) → `base_index_stage_gens` / `note_base_index_stage_gen`.
   - Comments like "P0-2 (#958): legacy `IndexManager` (regular + unique) generation..." → reword to "base_index `IndexManager`..." (the TASK NUMBER references like `#958`/`#959`/`#972` stay as-is — those are real, permanent task-history references, do not remove them).
   - Test file names / test function names containing "legacy" that describe THIS module (grep the `tests/` directories).
5. Doc comments throughout `shamir-index`/`shamir-engine`/`shamir-tx` that say things like "the legacy index manager", "legacy regular/unique", "legacy family" MEANING this module — reword to "base_index" / "the base_index family" / etc.

**Do NOT rename** (these are a DIFFERENT, legitimate meaning of "legacy" — about on-disk data FORMAT backward-compatibility, unrelated to this module's name):

- `IndexInfo::decode_bytes`'s "pre-`state` legacy shape" / "legacy shape" fallback-decoding language (in `index_info.rs` and referenced from `index_manager.rs`'s doc comments, per F-72/F-83's fixes) — this is about an OLD SERIALIZED BYTE FORMAT on disk that the decoder still needs to read for backward compatibility, nothing to do with the module's name. Leave every such reference exactly as-is.
- Similarly, `sorted_index_manager.rs`'s `persist_defs`/`load`'s "three-tier bincode forward-compat fallback chain (current-shape → pre-`state` → V1)" language, if it uses the word "legacy" anywhere to describe an old ON-DISK SHAPE (not this module) — leave it.
- Any OTHER crate's own, unrelated use of the word "legacy" that has nothing to do with `shamir-index`'s module (do a sanity grep first — `legacy` is a common English word and might appear in truly unrelated comments elsewhere in the workspace; leave those alone too).
- Do NOT touch the review document (`docs/dev-artifacts/research/2026-08-03-new-wave-readonly-review.md`) or any of the committed `docs/dev-artifacts/prompts/release-blockers/*.md` briefs — these are historical records of what was true when written; rewriting history in them would be actively misleading. Confirm with `git log` that these are already committed before you start, and simply do not touch them.

**The judgment call to get right**: read each match of the string "legacy" in context before deciding. If unsure whether a specific comment means "this module" or "an old disk-format shape", err on the side of NOT renaming it and note the ambiguous case in your report so the orchestrator can look at it.

## Mechanical approach

1. `git mv crates/shamir-index/src/legacy crates/shamir-index/src/base_index` (or the tool-appropriate equivalent — a plain file move, not `git mv` itself since you must not run git commands; just move/recreate the directory contents at the new path using your file tools, and leave the old path deleted).
2. Update `lib.rs`'s `pub mod` declaration.
3. Grep the ENTIRE workspace (not just `shamir-index`/`shamir-engine`) for `::legacy::` and `crate::legacy` and `shamir_index::legacy` to find every import site; fix each one.
4. Grep for the word `legacy` case-insensitively across `crates/` and manually triage each hit per the scope rules above (this is the labor-intensive, judgment-heavy part — do not skip it or rename blindly via a global search-replace across the whole word "legacy", since that WILL wrongly rename the on-disk-format references).
5. After the rename, run a full workspace build to catch anything missed — do not rely on grep alone (a renamed-wrong or missed import shows up as a compile error, which is the reliable final check).

## Required verification (this is a workspace-wide mechanical refactor — verify at that scope, not narrowly)

- `cargo check --workspace --all-targets` (full workspace, catches every crate that imports the renamed module).
- `cargo fmt --all -- --check` (the diff spans multiple crates).
- `cargo clippy --workspace --all-targets -- -D warnings`.
- `./scripts/test.sh` with NO `-p` scoping (full workspace lib tests) — this refactor touches public import paths across crate boundaries, a narrow `-p` run could miss a break in an unrelated consumer crate. If the full run is too slow for your session, at minimum run `./scripts/test.sh -p shamir-index -p shamir-engine -p shamir-tx -p shamir-db -p shamir-server -p shamir-client` (every crate that could plausibly import `shamir-index`) and say clearly if you didn't run the truly full workspace suite and why.

## Scope discipline

- This is a RENAME, not a refactor — do not restructure, merge, split, or "clean up" any code while you're in there. Resist the urge to fix unrelated things you notice.
- Do not touch anything in `docs/` except what this brief explicitly allows (nothing, in fact — leave all `docs/` untouched; even the CHANGELOG should not be touched by you, the orchestrator will handle documenting this rename after your diff is verified).

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index (including `git mv` — use your file-move tools instead). Do NOT run `git commit` or `git add` — the orchestrator verifies your diff and the test run, then commits. Only edit/move files and run read-only/build/test commands.

## What to report back

List every file touched (there will be many — a summary table by crate is fine, e.g. "shamir-index: N files, shamir-engine: N files, ..."), call out any ambiguous "legacy" occurrences you deliberately left alone (and why), and give the exact `cargo check`/`fmt`/`clippy`/`./scripts/test.sh` commands you ran with real pass/fail counts and exit codes. If the full workspace test run didn't fit in your session, say so explicitly and name exactly which crates you verified vs. which remain unverified.
