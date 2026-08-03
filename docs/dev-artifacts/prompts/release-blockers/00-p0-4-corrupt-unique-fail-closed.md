# Brief — P0-4: corrupt unique posting must be fail-closed, not fail-open

Task: #960 in the session TaskList. Source: `docs/dev-artifacts/research/2026-08-03-new-wave-readonly-review.md` §P0-4, verified against the actual source (not taken on faith) before filing this task.

## Bug

`crates/shamir-index/src/legacy/index_manager_unique.rs`, function `check_unique_key` (around lines 169-185):

```rust
async fn check_unique_key(&self, index_key: &Bytes) -> DbResult<Option<RecordId>> {
    match self.info_store.get(index_key.clone().into()).await {
        Ok(bytes) => {
            if bytes.len() == 16 {
                let arr: [u8; 16] = bytes.as_ref().try_into().unwrap();
                Ok(Some(RecordId(arr)))
            } else {
                // Коррупция данных — считаем что занято
                log::warn!("Invalid unique index value length: {}", bytes.len());
                Ok(None)
            }
        }
        Err(shamir_storage::error::DbError::NotFound(_)) => Ok(None),
        Err(e) => Err(e),
    }
}
```

The comment claims "corruption — treat as occupied" but `Ok(None)` is the exact signal `check_unique_key`'s callers use for "key is free" (it's the SAME return value as the genuine not-found branch two lines below). So corrupted unique-index storage is currently treated as an EMPTY, insertable key — the opposite of what the comment says and the opposite of this codebase's fail-closed policy for corruption (see F83's fix for corrupt `IndexInfo` metadata, same crate, for the precedent this should follow).

Impact: a subsequent write can pass unique-constraint validation and write a duplicate value, or otherwise commit on top of corrupted storage, silently violating the unique constraint.

## Required fix

1. Find (or add) a typed corruption error variant. Look for `DbError` in `crates/shamir-storage/src/error.rs` (or wherever `shamir_storage::error::DbError` is defined) — check whether an existing variant (e.g. `Codec`) fits, or whether F83's fix for corrupt `IndexInfo` metadata (search recent git history / that same unique/index manager area for how it signals corruption) already established a pattern to mirror. Prefer reusing that pattern over inventing a new one.
2. Change the `else` branch to return `Err(...)` with that typed corruption error instead of `Ok(None)`. Update the misleading comment to describe what the code now actually does.
3. Replace `bytes.as_ref().try_into().unwrap()` (line ~174, the 16-byte-length success path) with a `try_into()` that maps to a typed error via `?`, even though the length check right above makes the `unwrap()` currently infallible — this repo's convention (see CLAUDE.md "Error handling") is to avoid `unwrap()` outside true invariant violations, and a length-check-then-unwrap pattern is exactly the kind of thing that silently breaks if the check above it is ever edited.
4. Propagate the new error correctly through every caller of `check_unique_key`/`check_unique_constraint` (grep for both names in `crates/shamir-index` and `crates/shamir-engine`) — the goal is that corruption aborts the write/commit that would have relied on this check, not that it gets swallowed somewhere upstream into another "key is free" interpretation.

## Required tests

Add tests (in this module's existing `tests/` layout if one exists for `index_manager_unique.rs`, otherwise follow the repo's `tests/` directory convention from CLAUDE.md — one `tests/` dir per module, split by topic, `tests/mod.rs` re-exports only) covering:

- Stored value length 0 → corruption error, not `Ok(None)`.
- Stored value length 15 → corruption error.
- Stored value length 17 → corruption error.
- Stored value length much larger (e.g. 64 bytes) → corruption error.
- A genuine `NotFound` (key never written) still returns `Ok(None)` — do NOT break the real "key is free" path.
- An attempted `create`/`update`/tx-commit against a corrupted unique posting must abort the write, not silently succeed.

## Scope discipline

- Touch ONLY what this bug requires: `index_manager_unique.rs`'s `check_unique_key` (and the minimal error-plumbing needed for its callers to compile and propagate correctly), plus new tests. Do NOT touch regular/sorted/index2 index managers, DDL lifecycle, or anything from the other P0 items in the review — those are separate tasks (#957-#959, #961-#963) that will be worked in later, separate `crush` sessions.
- Do NOT rename/refactor unrelated code you encounter while reading nearby functions.
- Run the repo's centralized test entry point ONLY: `./scripts/test.sh -p shamir-index` (or narrower with `-- <filter>` once you know your new test names). Raw `cargo test` is blocked by this repo's perimeter guard — do not attempt to route around it.
- Run `cargo fmt -p shamir-index -- --check` and `cargo clippy -p shamir-index --all-targets -- -D warnings` before declaring done; if clippy has PRE-EXISTING failures unrelated to your change, note them in your final report instead of fixing them inline.

## ⛔ Git discipline (MANDATORY)

NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Do NOT run `git commit` or `git add` either — the orchestrator (a human-supervised session, not you) verifies your diff and the test run, then commits. Only edit files and run read-only/build/test commands.

## What to report back

End your turn with a clear summary: what you changed (file + line ranges), what error type you used and why, what tests you added and what each one proves, and the exact `./scripts/test.sh` / `cargo fmt` / `cargo clippy` commands you ran with their pass/fail outcome. If you hit a design decision not covered by this brief (e.g. no existing corruption error variant fits and you have to add one), state the choice you made and why.
