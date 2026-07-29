# Brief for F-55 (#881, P0) — fail-closed FK reverse-cache discovery + read-error propagation

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

This is the shamir-db Rust workspace. An independent readonly review of
snapshot `e145b1d3` (`docs/dev-artifacts/research/2026-07-29-new-wave-readonly-review.md`,
section P0-1) found a fail-open correctness bug in the repo-wide FK
reverse-cache discovery scan.

`build_reverse_fk_entries` (`crates/shamir-engine/src/repo/fk_reverse_cache.rs:486-516`)
is documented as an **authoritative, repo-wide** scan: for every table in
the repo, resolve it and collect every FK it declares. But at line 496-499:

```rust
let child_table = match resolver.resolve(&child_ref).await {
    Ok(t) => t,
    Err(_) => continue,
};
```

Any `resolve` failure on one child table — transient I/O error, storage
hiccup, whatever — is silently treated as "this table declares no foreign
keys" instead of failing the whole discovery scan. The caller,
`FkReverseCache::get_or_build_by_parent` (`fk_reverse_cache.rs:308-356`),
is generic over the build closure's `Result<Vec<TaggedReverseFkEntry>, E>`
and does `let all_entries = build().await?;` (line 345) — **if
`build_reverse_fk_entries` actually returned `Err`, that `?` would already
propagate it correctly and the CAS-publish would never run.** The bug is
entirely that `build_reverse_fk_entries` swallows the error itself instead
of returning it, so `get_or_build_by_parent` always sees `Ok(partial_list)`
and happily CAS-publishes an incomplete cache. This also defeats
`query_runner.rs`'s `require_footprint_if_fk_child` fail-closed branch,
which only widens a tx's footprint when `get_or_build_by_parent` returns
`Err` — an inner-swallowed error means that branch never fires.

The exact same pattern is duplicated, uncached, in
`discover_on_update_refs` (`crates/shamir-engine/src/query/batch/fk_on_update.rs:725-764`,
bug at line 743-746):

```rust
let child_table = match resolver.resolve(&child_ref).await {
    Ok(t) => t,
    Err(_) => continue,
};
```

Concrete failure scenario: repo has `parent` and FK-child `child`. Cache
is cold. `resolver.resolve(child)` transiently errors (e.g. a storage
hiccup). The scan finishes "successfully" with `child` absent from the
FK graph, and that **incomplete** cache is published and served warm until
the next invalidation. A subsequent `parent` UPDATE/DELETE never discovers
`child` as a dependent and can violate referential integrity.

## What to do

1. **`crates/shamir-engine/src/repo/fk_reverse_cache.rs`, `build_reverse_fk_entries`**:
   change the `Err(_) => continue` arm so any resolve failure aborts the
   whole scan and propagates the error — the function already returns
   `shamir_storage::error::DbResult<Vec<TaggedReverseFkEntry>>` and
   `resolver.resolve(...)` already returns that same `DbResult` type, so
   the simplest correct fix is replacing the `match` with `?`
   (`let child_table = resolver.resolve(&child_ref).await?;`). Confirm via
   `get_or_build_by_parent`'s `build().await?` (line 345) that this error
   now genuinely aborts the cache build and prevents any publish — read
   that function fully before changing anything to make sure no other
   caller expects `build_reverse_fk_entries` to be infallible-in-practice.

2. **`crates/shamir-engine/src/query/batch/fk_on_update.rs`, `discover_on_update_refs`**:
   this function returns `Result<Vec<OnUpdateRef>, BatchError>`, not
   `DbResult`, and a few lines above (the `resolve_repo` call) already
   shows the established pattern for mapping a `DbError` into
   `BatchError::QueryError` with a `message`/`code`. Apply the same
   mapping to the `resolve` failure instead of swallowing it — do not use
   a bare `?` here since the error type differs; mirror the existing
   `.map_err(|e| BatchError::QueryError { alias: String::new(), message:
   format!("fk_on_update: resolve(...): {e}"), code: Some("fk_on_update".to_string())
   })?` shape used just above for `resolve_repo`.

3. **Do not try to distinguish "concurrent DROP of a table mid-scan" from
   "a real I/O/catalogue error"** — the review explicitly notes there is
   no catalogue epoch today that would let you safely tell these apart, so
   the correct, safe behavior for both this task's scope is: fail the
   whole scan closed. (A future task could add a checked epoch to allow a
   narrower distinction; that is explicitly out of scope here.)

4. **Add a fault-injection test** for each fixed function: construct a
   `TableResolver` test double (check whether one already exists in
   `crates/shamir-engine/src/repo/tests/` or `fk_reverse_cache`'s own test
   module — reuse it if so) where `resolve` returns `Err` for exactly one
   table name, and assert:
   - `build_reverse_fk_entries` returns `Err`, not a partial `Ok(..)` list.
   - `discover_on_update_refs` returns `Err` (a `BatchError`), not a
     partial `Ok(..)` list.
   - (if feasible without excessive new scaffolding) a
     `get_or_build_by_parent` call wrapping a resolver with one
     poison-table returns `Err` and does NOT publish any cache state (the
     `ArcSwap` generation must not advance) — this is the actual
     user-visible fail-closed guarantee the review is asking for.

## What NOT to do

- Do NOT change `get_or_build_by_parent`'s own retry/CAS-publish logic
  (`fk_reverse_cache.rs:308-356`) — it already does the right thing once
  the inner build function actually returns `Err`; the bug is entirely in
  the two functions named above, not in the caller.
- Do NOT touch `fk_restrict.rs` / `fk_actions.rs`'s own scan-and-classify
  logic beyond what's needed to keep them compiling against the (now
  correctly fallible) reverse cache — this task is scoped to the discovery
  functions, not the RESTRICT/CASCADE/SET NULL consumers.
- Do NOT touch F-56/F-57/F-58/F-59/F-60/F-61 (other tasks from the same
  review) — this brief is F-55 only.

## Constraints

- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy --workspace --all-targets -- -D warnings` must be clean.
- Follow the TDD protocol: write the fault-injection test(s) first, confirm
  they fail against the current (buggy) code, then make the minimal fix,
  confirm green.
- Clean up any scratch/debug files created in the repo root before
  finishing.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine --full
```

When done, give your final summary as plain text: the exact diff for both
functions, the new test(s) added and their file paths, and confirmation
fmt/clippy/tests are clean.
