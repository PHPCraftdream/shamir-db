# Brief for F-22 (#815, P2) — surface `corrupt_records` in the TS (and Rust) SDK

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-10 (#800, already landed) added `QueryResult.corrupt_records:
Vec<CorruptRecordRef>` (`crates/shamir-query-types/src/read/query_result.rs`,
~line 62-75, 130-135) so a scan that hits a malformed row reports it
instead of silently vanishing from the result count. That closed the
engine-side gap. This task closes the **SDK-exposure** gap the post-wave
review flagged (narrower than F-30/#823, which is about widening
*coverage* of which engine read paths populate the field — do not touch
that scope here, this task is purely "does the field reach SDK
consumers in a well-typed, ergonomic way").

### Investigated finding #1 — Rust SDK already has it, nothing to add there

`crates/shamir-client` (the lower-level Rust client, which `shamir-sdk`
builds on) imports and returns `shamir_query_types::read::QueryResult`
directly (see `crates/shamir-client/src/interner_cache_ops.rs:28`,
`use shamir_query_types::read::{QueryRecord, QueryResult};`) — there is
NO separate, parallel "SDK response" struct that needs its own
`corrupt_records` field added. Any Rust consumer of `shamir-client`/
`shamir-sdk` already has direct field access to `result.corrupt_records`
today, since F-10 landed. **Confirm this is still true** (re-check for any
newer Rust-SDK-facing response wrapper that might have appeared since) —
if so, this task's Rust-side work is documentation only (see below), not
a new field/struct.

### Investigated finding #2 — a genuine wire-shape inconsistency to fix

`CorruptRecordRef.id` (`crates/shamir-query-types/src/read/query_result.rs`
~line 69-75) is `pub id: shamir_types::types::record_id::RecordId`, using
`RecordId`'s own derived `Serialize` impl
(`crates/shamir-types/src/types/record_id.rs` ~line 158-165), which emits
**raw msgpack `bin`** (16 bytes) — `serializer.serialize_bytes(&self.0)`.

But EVERY OTHER place a `RecordId` reaches the wire in this codebase uses
its **base58 string** form instead:
- `InsertedRecord` (`crates/shamir-query-types/src/write/inserted_record.rs`
  ~line 32, `let id_str = id.as_ref().map(|r| r.to_string());`) — a
  written row's `_id` is a base58 string on the wire.
- A read result row's own `_id` field is likewise documented as "the
  base58 `_id` string of the last row" (see
  `crates/shamir-client-ts/src/core/builders/query.ts` ~line 305-306,
  `after()`'s `afterId` doc comment).

So `CorruptRecordRef.id` is the ONE place a `RecordId` leaks onto the wire
as opaque bytes instead of the established base58-string convention — an
oversight from F-10, not an intentional design choice (nothing in F-10's
brief or its review discussed wire encoding for this field). Since this
hasn't shipped/tagged yet, fix it now rather than carry the inconsistency
forward: change `CorruptRecordRef` so its `id` serializes as the SAME
base58 string form as every other record id on the wire. Two ways to get
there — pick whichever is the smaller, cleaner diff after checking how
`InsertedRecord` did it:
- Give `CorruptRecordRef` a custom `Serialize`/`Deserialize` pair
  (mirroring `InsertedRecord`'s approach) that converts `id` to/from a
  base58 string at the wire boundary while keeping the Rust-side field
  typed as `RecordId` (nicer for Rust callers — no `FromStr` parsing
  needed at every call site).
- Or serialize `id` as `String` directly (`id.to_string()`), same
  external shape, simpler impl, but Rust callers get a `String` instead
  of a typed `RecordId` (check how `CorruptRecordRef` is actually
  constructed at its ~14 call sites in `read_exec.rs` — F-10's own scope
  — to judge which is more ergonomic there).

Verify no existing test currently pins the raw-bytes wire shape as
"correct" before changing it (checked: `crates/shamir-query-types/src/wire/
tests/db_message_tests.rs`'s only `corrupt_records` reference constructs an
EMPTY `vec![]`, so it does not exercise `CorruptRecordRef`'s actual byte
shape — safe to change).

## What to do

1. **Fix `CorruptRecordRef`'s wire encoding** (finding #2 above) — base58
   string, matching the rest of the codebase's `RecordId`-on-the-wire
   convention. Update its doc comment to state the wire shape explicitly.
2. **Rust SDK**: confirm finding #1 still holds (no separate struct to
   touch). Add a short doc-comment note on `QueryResult.corrupt_records`
   and/or `CorruptRecordRef` (if not already clear) confirming this is
   the SAME type Rust SDK consumers see directly — no wrapper needed.
3. **TS SDK type surface** — add the missing type in
   `crates/shamir-client-ts/src/core/types/batch.ts`:
   ```ts
   /**
    * A single record that failed to decode during a scan — reported
    * instead of silently dropped from the result set. Mirrors
    * `query_result.rs::CorruptRecordRef`. `id` is the record's base58
    * `_id` string (same form as a normal read result row's `_id` field),
    * not raw bytes.
    */
   export interface CorruptRecordRef {
     table: string;
     id: string;
   }
   ```
   Add `corrupt_records?: CorruptRecordRef[];` to the `QueryResult`
   interface (`batch.ts` ~line 199-235), with a doc comment matching this
   file's existing style for other optional/backward-compatible fields
   (e.g. `versions`' comment right above it) — explain it's omitted from
   the wire (and thus `undefined` here) on the common case of nothing
   corrupt, mirroring `#[serde(default, skip_serializing_if =
   "Vec::is_empty")]` on the Rust side.
4. **TS test** — find this crate's existing convention for asserting a
   decoded `QueryResult`-shaped wire response populates a given field
   correctly (check `crates/shamir-client-ts/src/__tests__/` and
   `core/__tests__/` for how other optional `QueryResult` fields like
   `versions`/`skipped`/`explain` are tested — likely a unit test feeding
   a hand-built msgpack payload through the decode path, or an e2e test
   against a real running server). Add a test confirming a response
   containing `corrupt_records` decodes into the typed
   `CorruptRecordRef[]` shape, AND that the common case (field omitted)
   leaves it `undefined` (regression guard against a false-positive
   default).
5. **Rust test** — add a round-trip test (find or create the right
   location — check whether `crates/shamir-query-types/src/read/` has (or
   should get) a `tests/query_result_tests.rs` following this crate's
   established `tests/` directory convention, or whether extending
   `db_message_tests.rs` is more consistent with how `corrupt_records` is
   already touched there) confirming: `CorruptRecordRef { table, id }`
   round-trips through msgpack serialize→deserialize with the SAME `id`
   value, AND that the serialized bytes contain a msgpack `str` marker for
   the id (not a `bin` marker) — a directly verifiable proof the wire
   shape is now the base58-string form, not raw bytes.
6. **Docs**: add a short entry to `docs/guide-docs/KNOWN_LIMITATIONS.md`
   noting `corrupt_records` is now surfaced with a typed shape in the TS
   SDK (cross-reference the existing F-10 bullet there rather than
   duplicating it — a one-line addendum saying "now typed on the TS side,
   F-22 #815" is enough). If `docs/guide-docs/client-server-protocol-spec/`
   documents `QueryResult`'s wire fields, update it there too (check
   first; only touch if it actually enumerates this struct).

## Constraints

- Do NOT widen which engine read paths populate `corrupt_records` — that
  is F-30 (#823), a separate, broader task. This task only touches the
  wire shape of `CorruptRecordRef` itself and its SDK-side type surface.
- Do NOT add `corrupt_records` handling to the query-BUILDER (Rust or TS)
  — matches F-10's own original constraint; this is a response-shape a
  client can inspect or ignore, not something constructed in a request.
- Rust: `cargo fmt -p shamir-query-types -- --check` and
  `cargo clippy -p shamir-query-types --all-targets -- -D warnings` must
  be clean. Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`),
  never raw `cargo test`.
- TS: run this crate's existing test command (check
  `crates/shamir-client-ts/package.json` for the right script, e.g.
  `npm test` / `npm run test` from that directory) and confirm it passes.
- Follow workspace conventions: `use` at file top, surgical diff.

## Verification the orchestrator will run

```
cargo fmt -p shamir-query-types -- --check
cargo clippy -p shamir-query-types --all-targets -- -D warnings
./scripts/test.sh -p shamir-query-types
```
(plus the TS test command you report having used, re-run by the
orchestrator from `crates/shamir-client-ts`)
