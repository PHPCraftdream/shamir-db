Task #537 — implement the keyset-pagination record-id tie-breaker, per the
user's explicit sign-off on `docs/dev-artifacts/design/keyset-pagination-tiebreaker-533-decision.md`
(task #533, commit `cb45ad22`). The user was asked (implement / document-as-
limitation / defer) and chose "завести отдельную задачу на реализацию"
(open a separate implementation task).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Confirmed root cause (from #533's investigation — re-verify line numbers,
## code may have shifted since)

`crates/shamir-index/src/legacy/sorted_index_manager.rs::lookup_range_first_k_page`
unconditionally skips every physical index entry whose ORDER BY value equals
the seek value:
```rust
if key_value_slice(kb, value_start) == seek_encoded {
    continue;
}
```
Correct only for the ONE row that established the seek boundary; every OTHER
row sharing that ORDER BY value is legitimately un-returned and gets silently
and PERMANENTLY dropped — every future page re-seeks on the same bare value
and hits the same skip. Root cause: `Pagination::After.key: Vec<QueryValue>`
(`crates/shamir-query-types/src/read/limit.rs:43-49`) carries only ORDER BY
values, never a row identity — the server cannot distinguish "the row the
client already has" from "a different row tied on the same value."

Also confirmed: `QueryRecord::Direct` (`crates/shamir-query-types/src/read/
query_record.rs`) — the wire variant every SELECT response uses — carries NO
`RecordId` today. `RecordId` is only injected as a synthetic `_id` field on
the WRITE-result variant (`InsertedRecord`). Exposing a row identity in read
results is itself part of this task's wire-protocol surface.

## The approved shape (backward-compatible, additive)

1. Expose a stable per-row identity in read results — extend
   `QueryRecord::Direct` (or add a sibling field) to optionally carry the
   row's `RecordId`, mirroring the existing `_id`-injection pattern already
   used for `InsertedRecord`.
2. Extend `Pagination::After` with an optional tie-breaker: `after_id:
   Option<RecordId>` (or an equivalent — e.g. a trailing element of `key`;
   pick whichever integrates more cleanly with the existing `keyset()`/
   `resolve()` accessors and the `PartialEq` impl's `key_bytes` canonical-
   encoding comparison). MUST default to `None` for backward compatibility —
   old clients that don't send it get today's exact skip-all-ties behavior
   (no regression, just the pre-existing known limitation persisting for
   them specifically).
3. Change `lookup_range_first_k_page`'s skip condition from "value ==
   seek_encoded → always skip" to "value == seek_encoded AND (row's
   record_id compares not-strictly-past `after_id`, or `after_id` is absent)
   → skip" — bound the physical-key range scan by `(seek_encoded,
   after_id)` instead of an approximate value-only filter. The physical key
   layout `[tag|name|encoded_value|record_id]` already supports this
   without a storage-format change — it's purely a comparison-logic fix
   once the tie-breaker is available.
4. Update every consumer: `shamir-query-types` (type definitions),
   `shamir-index` (scan logic), `shamir-engine` (`read_index_scan.rs::
   read_keyset_seek`'s continuation loop), `shamir-query-builder` (Rust
   query builder must build the tie-breaker into `Pagination::after(...)`),
   `shamir-client` (Rust client), `shamir-client-ts` (TS client's
   `.after(key, limit?)` builder needs a way to accept/pass the
   tie-breaker).

## TDD

1. A regression test proving the CURRENT bug exists before the fix (a
   table with several rows sharing one ORDER BY value straddling a keyset
   page boundary; red/green as appropriate for your working order).
2. A test proving old clients (no `after_id` sent) get byte-identical
   behavior to today — no regression for callers that don't opt in.
3. A test proving a client that DOES echo back the tie-breaker (id from a
   previous page's last row) correctly receives every remaining tied row
   across as many pages as needed.
4. Wire/serde round-trip tests for the extended `Pagination::After` and the
   extended `QueryRecord::Direct` (or wherever `RecordId` lands) in both
   Rust and TS test suites.

## Test scope

```
./scripts/test.sh -p shamir-query-types -p shamir-index -p shamir-engine -p shamir-query-builder -p shamir-client
```
Plus the TS client's own test runner for `crates/shamir-client-ts` if that's
how this repo's overall gate exercises it (check the TS project's test
script/package.json).

## Verification (lighter per-task gate, agreed this session)

```
cargo check --workspace --all-targets
./scripts/test.sh -p shamir-query-types -p shamir-index -p shamir-engine -p shamir-query-builder -p shamir-client
```
Do NOT run the full fmt/clippy/test --full gate — that's FINAL-GATE's
(#529's) job.

## Scope discipline

This is a genuine, deliberate, client-visible wire-protocol change (unlike
most of this campaign's internal-only work). Stay strictly additive —
no existing field removed or renamed, no existing behavior changed for a
client that doesn't opt in to the new `after_id`. If you find the change
needs to be BIGGER than this brief describes to work correctly (e.g. the
`RecordId`-in-read-results piece turns out to ripple further than
expected), STOP and document the specific blocker rather than forcing a
larger, riskier change than scoped here.

## Report format

```
[Implementation] Status: fixed / scoped-down-with-followup
  > Wire shape chosen for the tie-breaker (after_id field vs trailing key
    element) and why
  > Where RecordId now surfaces in read results, confirmed additive
  > Every consumer crate updated, confirmed old-client behavior unchanged
  > Semver/versioning note: this is a client-visible wire change — DO NOT
    bump any crate/package version yourself; flag this explicitly in your
    report so the orchestrator can raise it with the user before committing
[Verification]
  cargo check --workspace --all-targets: pass/fail
  ./scripts/test.sh -p shamir-query-types -p shamir-index -p shamir-engine -p shamir-query-builder -p shamir-client: pass/fail
```
