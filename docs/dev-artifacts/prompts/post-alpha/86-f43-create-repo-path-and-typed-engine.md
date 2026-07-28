# Brief for F-43 (#851, P1) — `CreateRepoOp.path` silent ignore + typed `RepoEngine`

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

A readonly review (`docs/dev-artifacts/research/2026-07-27-new-wave-readonly-review.md`,
finding P1-4, plus query-builder item 1 from the same review's §8) found:
the wire DTO `CreateRepoOp` has a `path: Option<String>` field, the Rust
query builder (`crates/shamir-query-builder/src/ddl/create_repo.rs:34-37`)
publicly exposes `.path(...)`, but
`crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs`'s
`handle_create_repo` **completely ignores `op.path`** — confirmed
yourself by grepping `crates/shamir-db/src` for `op.path` (the only hit
outside this DTO is an unrelated `admin_schema.rs:752` line matching on a
different `path` field entirely). The server always computes the repo's
actual on-disk directory itself (`data_root/<db>/<repo>`), silently
discarding whatever the client passed. A client calling `.path(...)`
believes it chose the storage location; the server returns success and
uses a different path with no error or warning.

## What to build

### 1. `handle_create_repo` — stop silently ignoring `op.path`

**Decision: reject, don't silently apply, and don't add a new
validated-root feature in this task.** Given the security implications of
letting a network client choose an arbitrary filesystem path (path
traversal risk into locations outside the intended data root), and given
no validated-root mechanism exists today to build on safely without its
own design work, the correct fix for THIS P1 is: **if `op.path` is
`Some(_)`, return a clear, typed error** (state your reasoning if you
disagree and choose differently, but this is the recommended default) —
e.g. `err_code("unsupported_field", "CreateRepo.path is not supported: \
the server always resolves the storage location internally (data_root/<db>/<repo>)")`
or whatever this file's existing error-code conventions look like (check
`err`/`err_code` closures already defined in `handle_create_repo`, reuse
the same shape). `op.path == None` (the common, correct case) is
unaffected — no behavior change for callers that never set `.path(...)`.

Do NOT implement path validation/allowlisting as an alternative — that's
explicitly out of scope (a separate, bigger feature with its own security
review needs), not a P1 bug-fix task's job.

### 2. Typed `RepoEngine` for the query builder (additive, non-breaking)

`CreateRepoOp.engine` is a plain `Option<String>` on the wire — keep the
WIRE shape unchanged (do not touch `shamir-query-types`). Add a
`RepoEngine` enum to `crates/shamir-query-builder/src/ddl/create_repo.rs`
(or wherever this crate's convention puts such small helper types — check
for a precedent), covering the actual supported engine strings
(`"in_memory"`, `"fjall"`, `"hybrid"` — confirm these are still the
complete, correct set by checking `crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs`'s
engine-match arms yourself, do not trust this brief's list blindly) plus
an escape hatch for forward-compat (e.g. `RepoEngine::Other(String)`):

```rust
pub enum RepoEngine {
    InMemory,
    Fjall,
    Hybrid,
    /// Escape hatch for an engine value not yet known to this builder
    /// version (forward-compat with a server that supports more engines
    /// than this client library does).
    Other(String),
}
```

Implement `impl From<RepoEngine> for String` (or `Into<String>`
equivalently) mapping each variant to its wire string. Because
`CreateRepo::engine`'s existing signature is `pub fn engine(mut self,
engine: impl Into<String>) -> Self`, adding this `From` impl lets EVERY
existing caller keep passing a plain string unchanged, while NEW callers
can pass `RepoEngine::Hybrid` etc. — verify this compiles and both call
styles work, don't just assume the generic bound resolves correctly.

**Also fix a stale doc comment while touching this file**: `engine`'s doc
comment currently says *"Set the storage engine (e.g. `"in_memory"`,
`"redb"`, `"fjall"`)"* — `"redb"` is not a real supported engine value
(confirm against the actual server match arms) and `"hybrid"` is missing
from the example list. Correct it.

## Tests — MANDATORY, in the same commit

1. A DDL-level test (find this crate's existing `CREATE REPO` test
   pattern — check `crates/shamir-db/src/shamir_db/tests/hybrid_repo_ddl_tests.rs`
   for the query-builder-only style to mirror) proving `CREATE REPO`
   with an explicit `.path(...)` set returns the new typed error, and the
   repo is NOT created (check via `has_repo`/`list_repos` afterward).
2. A regression: `CREATE REPO` WITHOUT `.path(...)` still succeeds exactly
   as before (no behavior change for the common case).
3. A query-builder unit test (find or create the right test module in
   `shamir-query-builder`'s own test tree) confirming `RepoEngine::Hybrid`
   (and the other variants) produce the correct wire string when passed to
   `.engine(...)`, and that a plain string still works unchanged (both call
   styles compile and produce identical `CreateRepoOp.engine` values).

## Constraints

- Do NOT touch `shamir-query-types`'s `CreateRepoOp` wire DTO — keep
  `path`/`engine` as plain `Option<String>` on the wire; this task only
  changes how the SERVER responds to `path` being set, and adds a
  BUILDER-side (not wire-side) typed helper for `engine`.
- Do NOT implement path validation/allowlisting — reject, don't
  validate-and-apply.
- Do NOT remove the `.path()` builder method (removing a public method is
  a breaking API change beyond this task's scope) — it can still be
  called; the SERVER now errors clearly if it's actually used with
  `Some(_)`, which is the fix.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-db -p shamir-query-builder -- --check` and
  `cargo clippy -p shamir-db -p shamir-query-builder --all-targets -- -D warnings`
  must be clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-db -p shamir-query-builder -- --check
cargo clippy -p shamir-db -p shamir-query-builder --all-targets -- -D warnings
./scripts/test.sh -p shamir-db -- create_repo
./scripts/test.sh -p shamir-query-builder --full
./scripts/test.sh -p shamir-db --full
```
