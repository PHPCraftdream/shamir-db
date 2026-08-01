# F-83 (#911) — fix silent unique-constraint loss on a corrupt IndexInfo blob

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Background

This is a finding from an `@oh` adversarial review of the F-69..F-81
remediation wave (see `docs/checkpoints/p0-p1-wave-complete.md` and task
#911). `IndexInfo::decode_bytes` (`crates/shamir-index/src/legacy/index_info.rs:183-216`,
added this wave for F-72, #899) already correctly distinguishes three
cases and documents them in its own doc comment (lines 168-182):

1. Current on-disk shape decodes fine → `Ok`.
2. Pre-F-72 legacy shape (missing the `state` field) decodes via a
   fallback, lifted to `state = Ready` → `Ok`.
3. Neither shape decodes — **genuine corruption** → `Err(bincode::Error)`,
   which the doc comment explicitly says is "surfaced as an error rather
   than silently discarding the caller's existing index definitions."

But its ONLY two production call sites contradict that exact promise —
`crates/shamir-index/src/legacy/index_manager.rs:172` and `:180`:

```rust
let indexes = match info_store.get(indexes_key.clone().into()).await {
    Ok(bytes) => {
        // ... At error we begin with an empty set (matches this call
        // site's pre-existing best-effort recovery policy).
        IndexInfo::decode_bytes(&bytes).unwrap_or_else(|_| IndexInfo::new())
    }
    Err(shamir_storage::error::DbError::NotFound(_)) => IndexInfo::new(),
    Err(e) => return Err(e),
};
// ... identical shape at :180 for indexes_unique_key
```

Note the `NotFound` case (blob never written — brand-new table) is
ALREADY handled separately, one match arm up, and correctly yields a
fresh empty `IndexInfo` — that is the legitimate "no info yet" case. The
`Ok(bytes) => ... unwrap_or_else(...)` arm only runs when bytes WERE
actually read from storage, so a `decode_bytes` failure inside that arm
is NECESSARILY case 3 above (genuine corruption) — it can no longer be
case 1 or 2 (`decode_bytes` already handles those internally and returns
`Ok`). There is no ambiguity left to preserve; the `unwrap_or_else` here
is pure data loss.

## Concrete failure scenario

A table has one unique index. Its `system:indexes_unique` blob on disk
gets corrupted (e.g. a torn/partial write survives a crash — this
crate's own docs elsewhere acknowledge storage backends can produce
partial writes across a crash). On next open:

1. `info_store.get(indexes_unique_key)` succeeds (bytes exist, just
   corrupt).
2. `IndexInfo::decode_bytes(&bytes)` correctly returns `Err` (matches
   neither shape).
3. `.unwrap_or_else(|_| IndexInfo::new())` silently substitutes an EMPTY
   `IndexInfo` — the unique index definition is gone from memory.
4. `has_indexes_unique_flag = indexes_unique.is_enabled()` → `false` →
   `WriteBarrierFlags::with_unique_index_exists(false)` — every writer
   now takes the fast path and skips unique validation entirely
   (`index_manager_unique.rs:37,93` gate on this exact flag).
5. Duplicate values are now silently accepted into a column the schema
   still nominally treats as unique.
6. The NEXT `save_index_info_unique` (or equivalent persist call) writes
   the now-empty `IndexInfo` back to disk — the constraint is
   permanently, silently gone. No error was ever surfaced to an operator.

This is exactly the swallow class F-73 (#900) was chartered to eliminate
in the commit path — the same bug pattern, one crate over, on the
index-metadata LOAD path instead.

## What to build

Both call sites (`index_manager.rs:172` and `:180`) must propagate a
genuine `decode_bytes` failure as an error from `IndexManager::new`,
instead of silently substituting an empty `IndexInfo`. `IndexManager::new`
already returns `Result<Self, shamir_storage::error::DbError>`, and its
caller (`TableManager`'s constructor, `table_manager.rs:285`) already
propagates via `?` — so fixing these two sites is sufficient; no signature
changes ripple further.

Convert the `bincode::Error` into a `DbError` — `DbError::Codec(String)`
already exists exactly for this (`crates/shamir-storage/src/error.rs:36-37`,
`impl From<CodecError> for DbError` exists but `bincode::Error` isn't
`CodecError` — construct `DbError::Codec(format!("..."))` directly, or add
a `From<bincode::Error>` impl if that's cleaner and doesn't collide with
an existing one; check first). Include the table/key context in the
message (which key: `indexes` vs `indexes_unique`) so an operator seeing
this error in a log/panic knows exactly which blob is corrupt.

**Do not touch `decode_bytes` itself** — it is already correct; this is a
call-site fix only. Do not add any new "best-effort recovery" fallback —
the doc comment's promise is that this SHOULD be a hard failure, not a
softened one.

**Preserve exact behavior for the two `Ok` cases inside `decode_bytes`**
(current shape, legacy pre-F-72 shape) — those must continue to succeed
silently (aside from `decode_bytes`'s existing `log::warn!` on the legacy
fallback path, which stays as-is). Only the genuine-corruption `Err` path
changes from silently-discarded to propagated.

## Tests

Write a test that constructs a genuinely-corrupt blob (e.g. random bytes,
or a valid-looking-but-truncated bincode buffer that decodes as neither
`IndexInfo`'s current shape nor `IndexDefinitionNoState`'s legacy shape —
check `index_info.rs`'s existing test fixtures for the closest precedent
of how a "genuinely corrupt" blob is constructed elsewhere in this crate's
test suite, e.g. `crates/shamir-index/src/legacy/tests/f72_legacy_state_compat_tests.rs`
may have useful patterns to adapt), writes it directly to the info store
under the `indexes_unique` key, and asserts `IndexManager::new(...)`
returns `Err` (RED-then-GREEN: this must currently fail — confirm the
test would pass silently with the OLD `unwrap_or_else` code, by
temporarily reverting your own fix locally and re-running, before
finalizing — this IS the sabotage-then-restore proof for this task, do it
yourself and report the result in the commit message).

Also confirm the pre-existing "NotFound → fresh empty IndexInfo" path is
UNCHANGED by adding/checking a test that a brand-new table (no blob ever
written) still opens cleanly with an empty index set — this must keep
passing, proving the fix is scoped to genuine corruption only, not to the
legitimate "no info yet" case.

## Definition of done

- Both call sites propagate a genuine `decode_bytes` error via `DbError`
  instead of `unwrap_or_else(|_| IndexInfo::new())`.
- `decode_bytes` itself unchanged.
- New test(s) proving: (a) genuine corruption on either key now fails
  `IndexManager::new` instead of silently losing the index/unique-index
  set; (b) a brand-new table (missing blob) still opens cleanly.
- `cargo fmt -p shamir-index -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-index -p shamir-engine --full` green.
- Commit message states the sabotage-then-restore result (old code passed
  the new corruption test silently / new code correctly fails closed).

Commit with `Co-Authored-By: Claude Sonnet 5 <noreply@anthropic.com>` per
repo convention.
