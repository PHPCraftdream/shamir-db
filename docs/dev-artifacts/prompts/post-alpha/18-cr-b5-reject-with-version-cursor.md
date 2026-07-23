# Brief: CR-B5 — reject `with_version=true` at `CreateCursor` (#771)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem — silent CAS-contour breakage, verified against the current tree 2026-07-23

A cursor's every internal read (both `create_cursor`'s first page and
every `fetch_next`) rewrites the query's `temporal` field to
`Temporal::AsOf { at: At::Version(pinned_version) }`
(`crates/shamir-server/src/db_handler/cursor_handlers.rs`, search for
`Temporal::AsOf` — several sites already do this). The `AsOf` read path
(`crates/shamir-engine/src/table/read_temporal.rs`, the `QueryResult` it
returns) hard-codes `versions: None` (~line 146,
`read_temporal.rs`'s final `QueryResult` construction — verify the exact
line before citing it in your diff). This means `ReadQuery.with_version =
true` — the flag that makes a plain (non-cursor) read attach a per-record
version for later optimistic-CAS use (the FG-2 contour, see
`docs/guide-docs/client-server-protocol-spec/OPTIMISTIC_CONCURRENCY.md`) —
silently produces NO versions when the SAME query is run through a
cursor. A client that built a `.withVersion()` read expecting to later
CAS-update a record, then discovered the query needed to be paginated and
switched it to a cursor, would get no error and no versions — a correctness-
relevant feature quietly stops working.

## Fix — reject explicitly at `CreateCursor`, mirroring the existing temporal scope-cut

`create_cursor`
(`crates/shamir-server/src/db_handler/cursor_handlers.rs`, ~line 707-711)
already has an analogous rejection for the SAME class of problem:

```rust
// Scope cut (FG-5b): only Temporal::Latest cursors are supported.
// AsOf/History are rejected outright — never silently downgraded.
if !matches!(query.temporal, Temporal::Latest) {
    return error_response(&BatchError::CursorTemporalNotSupported);
}
```

Add a sibling check immediately after (or before — whichever reads more
naturally next to the existing one) for `query.with_version`:

```rust
if query.with_version {
    return error_response(&BatchError::CursorWithVersionNotSupported);
}
```

(Exact variant name is a suggestion — follow whatever naming convention
`CursorTemporalNotSupported`/`CursorPageTooLarge` already established if
you land on something slightly different; keep the `Cursor*NotSupported`
family consistent.)

### New `BatchError` variant

In `crates/shamir-query-types/src/batch/batch_error.rs`, add
`CursorWithVersionNotSupported` (a unit variant, no fields — mirrors
`CursorTemporalNotSupported`'s shape) right next to
`CursorTemporalNotSupported` (~line 176). Doc comment should explain:
`with_version=true` requests a per-record version stamp for later
optimistic-CAS use; the `AsOf` read path a cursor's every internal fetch
uses hard-codes `versions: None` (cite the `read_temporal.rs` line once
you've confirmed it), so honoring `with_version` through a cursor would
either silently produce no versions (today's bug) or require threading
real historical per-record versions through the whole `AsOf` pipeline —
out of scope here. Note as a code-comment aside that returning REAL
historical versions through a cursor is the better long-term fix, tracked
as a possible follow-up, not attempted in this task. End the doc comment
with the wire error code line, matching the sibling variants' convention:
`/// Wire error code: `cursor_with_version_not_supported`.`

Wire the `Display` impl (`batch_error.rs`, the `impl std::fmt::Display for
BatchError` block, right next to `CursorTemporalNotSupported`'s arm) and
`error_code()` (`crates/shamir-server/src/db_handler/handler.rs`, ~line
739, right next to `CursorTemporalNotSupported => "cursor_temporal_not_supported"`)
— both exactly mirroring the sibling variant's pattern.

Add coverage in `crates/shamir-query-types/src/batch/tests/batch_error_tests.rs`
(check the existing test naming/structure for `CursorTemporalNotSupported`/
`CursorPageTooLarge` and follow the same pattern exactly — likely a
Display-text assertion and an `error_code()`-mapping assertion, possibly a
"each variant produces a DISTINCT code" exhaustiveness-style test if one
already exists for the cursor error family).

## Docs (do NOT skip)

- `docs/guide-docs/client-server-protocol-spec/CURSORS.md`: add a
  one-line limitation entry (in whichever section already documents the
  `Temporal::Latest`-only scope cut — likely right next to
  `cursor_temporal_not_supported` in the errors table, §6) for
  `cursor_with_version_not_supported`, describing the condition and the
  "why" briefly (references the AsOf-path `versions: None` limitation).
- `docs/guide-docs/KNOWN_LIMITATIONS.md`: this file already has a bullet
  (added by CR-A7) reading "`CreateCursor`/`FetchNext` do not yet reject
  `with_version: true`" (search for that exact text under "## 6. Results")
  — THIS task is what that bullet was foreshadowing. Update it now that
  the rejection is real: either remove the bullet (the limitation it
  described — "unsupported/unverified, avoid combining" — is now
  enforced, not just advised) or reword it to state the enforced fact
  (`with_version: true` is rejected outright at `CreateCursor` with
  `cursor_with_version_not_supported`) if you judge a still-documented
  entry is useful context. Your call, but do not leave the OLD
  "not yet rejected" wording in place once this task lands — that would
  be a new doc/code mismatch of exactly the kind CR-A7 was created to
  clean up.

## Tests (TDD — write failing tests first)

In `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`:

- `CreateCursor` with `query.with_version = true` (check how `ReadQuery`
  sets this flag — likely a builder method like `.with_version()` or a
  struct field set directly, follow whatever convention this codebase's
  `ReadQuery` construction already uses elsewhere in this test file or in
  `shamir-query-builder`) → must return the new distinct error code, NOT a
  `CursorPage` with silently-missing versions.
- **Regression guard**: a PLAIN (non-cursor) read with `with_version = true`
  still returns real per-record versions — proves this task didn't touch
  the working, non-cursor path (find or write a minimal test using the
  normal batch `Execute`/`Read` path, not `CreateCursor`).
- Error-variant `Display`/`error_code()` tests in
  `batch_error_tests.rs` per the existing conventions (see above).

## Gate

```
cargo fmt -p shamir-server -p shamir-query-types -- --check
cargo clippy -p shamir-server -p shamir-query-types --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -p shamir-query-types --full
```

All must pass before returning. Primary code area: `shamir-query-types`
(`batch_error.rs` + its tests), `shamir-server`
(`db_handler/cursor_handlers.rs`, `db_handler/handler.rs`, tests), plus
the two docs files named above. Do NOT touch the `AsOf` pipeline itself
(`shamir-engine`'s `read_temporal.rs`) — this task rejects the combination
outright rather than threading versions through the temporal read path
(that's explicitly out of scope, noted as a possible follow-up).
