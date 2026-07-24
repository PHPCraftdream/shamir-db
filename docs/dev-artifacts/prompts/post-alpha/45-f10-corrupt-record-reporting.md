# Brief for #800 (F-10) — surface corrupt records instead of silently dropping them

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Scope of THIS task (deliberately bounded)

`crates/shamir-engine/src/table/read_exec.rs` has **14 sites** matching the
pattern `Err(_) => continue` (or `Err(_) => break` — check each) on a
per-record decode/deserialize failure inside a scan loop — a malformed
row is silently skipped, and the result set is simply one row short, with
no indication to the caller that anything was wrong (as opposed to that
row genuinely not matching the filter). Exact current locations (line
numbers as of this brief — re-verify, other tasks in this campaign may
have shifted them slightly):

```
~532, ~535, ~951, ~988, ~1223, ~1346, ~1467, ~1787, ~1832, ~1834, ~2030, ~2086, ~2089, ~2407
```

**Two sibling files have the SAME pattern and are explicitly OUT OF SCOPE
for this task** — `crates/shamir-engine/src/table/table_manager_index_mgmt.rs`
and `crates/shamir-engine/src/table/table_manager_streaming.rs`. Note
them in `docs/guide-docs/KNOWN_LIMITATIONS.md` as a follow-up (same
mechanism, not yet wired into those two files) rather than trying to
cover everything in one task — this task's `read_exec.rs`-only scope is
already substantial (14 sites, plus a new wire-visible struct field).

## Design — a new `QueryResult::corrupt_records` field, not a hard error

A single malformed row today just silently vanishes from the result
count — the review's core objection. The database should surface this:
collect `(table, RecordId)` for every row that failed to decode into a
new field on `QueryResult`, rather than erroring out the whole scan (a
single corrupt row aborting an otherwise-successful query over millions
of good rows would be a worse outcome than a documented gap).

1. **New type** in `crates/shamir-query-types/src/read/query_result.rs`
   (same file as `QueryResult`/`QueryStats`/`ExplainPlan` — this crate's
   convention groups small tightly-related read-result types in one file;
   check whether the "one file = one primary export" rule should instead
   push this into its own sibling file — if `query_result.rs` already
   holds 3 exports today as a cohesive "read result" group, adding a
   4th small one here is consistent with the EXISTING file's own
   precedent; use your judgment and match whichever convention this file
   already demonstrates):
   ```rust
   /// A single record that failed to decode during a scan — reported
   /// instead of silently dropped from the result set.
   #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
   pub struct CorruptRecordRef {
       /// Name of the table the corrupt record belongs to.
       pub table: String,
       /// The record's id (still resolvable even though its VALUE failed
       /// to decode — ids are read independently of the value payload).
       pub id: shamir_types::types::record_id::RecordId,
   }
   ```
   Check `RecordId`'s existing derive list (used elsewhere in wire types
   already) to confirm `Serialize`/`Deserialize`/`PartialEq` are already
   implemented — they should be, since `RecordId` already appears in
   wire-serialized structures.
2. **New field** on `QueryResult`
   (`crates/shamir-query-types/src/read/query_result.rs`, ~line 64-109):
   ```rust
   /// Records that failed to decode during this scan — dropped from
   /// `records`, but reported here as `(table, id)` pairs instead of
   /// silently vanishing. Empty (and omitted from the wire) on the
   /// common case where nothing was corrupt.
   #[serde(default, skip_serializing_if = "Vec::is_empty")]
   pub corrupt_records: Vec<CorruptRecordRef>,
   ```
   Match the EXISTING backward-compatible wire-field convention this
   struct already uses (`#[serde(default, skip_serializing_if = ...)]`)
   — an old peer that doesn't know this field simply never sees it; a
   new peer reading an old response gets an empty `Vec` by default.
3. **Every construction site of `QueryResult` in `read_exec.rs`** needs to
   populate this new field (most can just be `Vec::new()` when nothing
   went wrong — check whether `QueryResult { .. }` struct-update syntax
   with a `Default`-derived base, or an explicit field, is cleaner given
   how many literal `QueryResult { ... }` constructions already exist in
   this file — do NOT use `..Default::default()` if `QueryResult` doesn't
   already derive `Default` and adding that derive would be a bigger
   change than warranted; just add the field explicitly at each existing
   `QueryResult { ... }` literal).
4. **At each of the 14 `Err(_) => continue` sites**: before skipping,
   check whether a `RecordId` is available in the enclosing scope (most
   are inside a loop over `(id, cow)` or similar tuples — grab `id` there)
   and push a `CorruptRecordRef { table: self.name().to_string(), id }`
   into a `corrupt: Vec<CorruptRecordRef>` accumulator that's threaded
   through to the final `QueryResult { corrupt_records: corrupt, ... }`
   construction for that function. Investigate each site individually —
   some may be inside a nested closure/helper where threading the
   accumulator requires a small signature change (e.g. an `&mut
   Vec<CorruptRecordRef>` parameter) — prefer the smallest change that
   correctly threads the accumulator to the right final `QueryResult`,
   and if a specific site genuinely has NO `RecordId` available in scope
   (verify before assuming), skip logging that one site as a corrupt
   record and leave a comment explaining why, rather than inventing a
   fake id.

## Tests

Find or create the test file(s) covering `read_exec.rs`'s scan paths
(check `crates/shamir-engine/src/table/tests/` for an existing
"malformed record" or "corrupt" test convention first) and add:

1. **At least 3-4 REPRESENTATIVE sites** (not necessarily all 14 — pick a
   cross-section covering different scan shapes: e.g. one from the plain
   `read_collecting` loop, one from `read_streaming`, one from an
   index-scan path, one from the aggregate/raw_acc path) — insert a
   deliberately malformed record (corrupt msgpack bytes for that row,
   written directly to the underlying store bypassing normal
   insert-path validation — check how existing tests in this crate
   already construct a malformed record for a similar purpose, e.g.
   search for existing "malformed" test fixtures before inventing a new
   injection technique) alongside valid rows, run a query that would
   scan past it, and assert:
   - the valid rows still return correctly (no regression to today's
     "skip and continue" recovery behavior),
   - `QueryResult::corrupt_records` contains exactly one entry with the
     correct `table` name and the correct `RecordId` of the malformed
     row.
2. **The common case is unaffected**: a query with NO corrupt records
   still returns `corrupt_records: []` (or omitted from the wire,
   depending on how the differential/wire test checks this) — a pure
   regression guard that this task's plumbing doesn't accidentally
   surface false positives.
3. Update `docs/guide-docs/KNOWN_LIMITATIONS.md` with an entry describing
   the new `corrupt_records` field, its coverage (read_exec.rs's 14
   sites), and the two sibling files (`table_manager_index_mgmt.rs`,
   `table_manager_streaming.rs`) that still silently skip corrupt records
   without reporting them — a documented, honest scope boundary, not a
   silent gap.
4. Update `docs/guide-docs/client-server-protocol-spec/` if it documents
   the `QueryResult` wire shape (check first; only touch if it actually
   enumerates this struct's fields).

## Constraints

- Do NOT turn any of these 14 sites into a hard error — the row is still
  skipped from `records` (this behavior is UNCHANGED), only the
  reporting changes (silent → visible via `corrupt_records`).
- Do NOT touch `table_manager_index_mgmt.rs` or
  `table_manager_streaming.rs` — explicitly out of scope, documented as
  follow-up instead.
- Do NOT add `corrupt_records` handling to the query-builder (Rust or
  TS) — this is a response-shape addition a client can choose to inspect
  or ignore; no builder-side change is needed.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -p shamir-query-types` and
  `cargo clippy -p shamir-engine -p shamir-query-types --all-targets --
  -D warnings` must be clean.
- Follow workspace conventions: `use` at file top, one primary export per
  file (or match `query_result.rs`'s own existing multi-small-type
  precedent, per the design section above), surgical diff.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -p shamir-query-types -- --check
cargo clippy -p shamir-engine -p shamir-query-types --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -p shamir-query-types
```
