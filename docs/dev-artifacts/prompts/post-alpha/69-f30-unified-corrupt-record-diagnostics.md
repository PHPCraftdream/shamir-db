# Brief for F-30 (#823, P1) — unified corrupt-record diagnostics across remaining engine read paths

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-10 (#800, already landed) added `QueryResult.corrupt_records:
Vec<CorruptRecordRef>` and populated it at all 14 decode-failure sites in
`crates/shamir-engine/src/table/read_exec.rs`'s OWN scan loops
(`read_collecting`, `read_counting`, `read_streaming`, the index2 fast
path, the filtered-vector-scan trio). It deliberately left several sites
out of scope, explicitly documented in its own code comment
(`read_exec.rs` ~line 2554-2561) and in `KNOWN_LIMITATIONS.md`. This task
closes the REACHABLE parts of that remaining gap.

## Confirmed scope (verified by direct code reading — do not re-derive)

### 1. `try_project_page_only_bytes` / `apply_select_value_bytes` (PRIMARY, reachable, real gap)

`read_exec.rs` ~line 2523-2621 — two free functions (no `&self`, no
`QueryResult` in scope) with `Err(_) => continue` /
`Err(_) => QueryValue::Null` decode-failure fallbacks that silently drop a
malformed row's contribution. Called from **10 real production sites**
across two files:

- `crates/shamir-engine/src/table/read_index_scan.rs` — lines ~337, 358,
  438, 569, 758, 780 (verify exact lines, may have shifted since this
  brief).
- `crates/shamir-engine/src/table/read_temporal.rs` — lines ~150, 178, 353.

**Confirmed feasible**: every call site of `try_project_page_only_bytes`
is immediately followed by a `QueryResult { ..., corrupt_records:
Vec::new(), ... }` literal construction (verified at
`read_index_scan.rs` ~line 340-354) — there IS a `QueryResult` in scope at
the call site, just not inside the free function itself. Thread a
`&mut Vec<CorruptRecordRef>` accumulator parameter through BOTH free
functions (matching F-10's own established pattern in `read_exec.rs`'s
other functions — check how those did it for the exact convention to
mirror) and have each of the ~10 call sites pass a local `let mut corrupt
= Vec::new();`, then populate the final `QueryResult.corrupt_records:
corrupt` instead of the current hardcoded `Vec::new()`.

For `apply_select_value_bytes`'s callers that don't immediately construct
a `QueryResult` (e.g. if a call site is mid-pipeline, feeding into
`apply_order_by_qv`/`apply_pagination` before the eventual `QueryResult`
literal further down — check `read_index_scan.rs` ~line 357-370 pattern),
thread the SAME accumulator through that whole local pipeline to the
final `QueryResult` construction at the bottom of each function — do not
invent a second, disconnected accumulator per call site within the same
function.

### 2. `table_manager_streaming.rs`'s `filter_stream`/`filter_stream_tx` (SECONDARY — verify reachability first)

`crates/shamir-engine/src/table/table_manager_streaming.rs` has two
similar-shaped corrupt-skip sites: `filter_stream`'s closure (~line
162-199, `Err(_) => false, // malformed → skip`) and
`filter_stream_tx`'s overlay-merge closure (~line 271+, `Err(_) => false,
// malformed → exclude`).

**Investigate first**: confirmed by a workspace-wide grep during this
brief's own preparation that NEITHER `filter_stream` NOR `filter_stream_tx`
currently has any PRODUCTION caller anywhere in the workspace (only
referenced in `crates/shamir-engine/src/table/tests/filter_stream_tests.rs`
and one SSI test file, `predicate_capture_tests.rs`). Re-verify this is
still true (grep `\.filter_stream(` and `\.filter_stream_tx(` across
`crates/` excluding `tests/`/`_tests.rs` files) before deciding scope:

- If still unreached by production code: these methods return a raw
  `Stream<Item = DbResult<Vec<(RecordId, RecordCow)>>>`, not a
  `QueryResult` — there's no natural corrupt-records sink without a much
  bigger API change (the stream's item type would need to carry corrupt
  markers alongside matched rows, a real design change, not a small fix).
  Given they're currently dead/unreached, document this precisely in
  `KNOWN_LIMITATIONS.md` as an accepted, currently-moot gap (a future
  caller of these methods would need to add corrupt-tracking then) rather
  than doing speculative work on unreached code. Do NOT touch these two
  methods' implementation.
- If you find a production caller this brief's own investigation missed:
  stop and treat it as a real, reachable gap — bring it into this task's
  scope, matching the design used for point 1 above as closely as the
  stream-based API shape allows (investigate what's actually feasible
  before committing to a design; this may need its own follow-up task if
  it's genuinely a bigger redesign — use judgment and state your
  reasoning clearly in your summary either way).

## Tests

**MANDATORY, test-then-fix in the same commit**, mirroring F-10's own test
conventions (check `crates/shamir-engine/src/table/tests/` for its
existing "malformed record" test fixtures/injection technique first —
reuse it, don't invent a new one):

1. At least 2-3 representative sites across `read_index_scan.rs` and
   `read_temporal.rs` (one hitting `try_project_page_only_bytes`'s
   LIMIT-pushdown path, one hitting `apply_select_value_bytes`'s general
   path) — insert a deliberately malformed record alongside valid rows,
   run a query through that specific code path, and assert: valid rows
   still return correctly (no regression), AND `QueryResult.corrupt_records`
   contains the correct `{table, id}` entry for the malformed row.
2. A regression guard: a query with NO corrupt records still returns
   `corrupt_records: []`/omitted — no false positives introduced.
3. If point 2 above (table_manager_streaming.rs) concludes "still
   unreached, doc-only" — no test needed for those two functions on this
   pass (nothing to exercise). If you instead found a real caller, add a
   test analogous to point 1.

## Docs

Update `docs/guide-docs/KNOWN_LIMITATIONS.md`'s existing corrupt-records
bullet (search for "Corrupt-record reporting covers `read_exec.rs`'s scan
paths only" — already extended once by F-20/#813 and F-22/#815) to
reflect the NEW coverage: `try_project_page_only_bytes`/
`apply_select_value_bytes` and their ~10 call sites are now covered. State
precisely what (if anything) remains uncovered after this task
(`table_manager_streaming.rs`'s two methods, if still unreached — note
they're currently dead code, not a live gap).

## Constraints

- Do NOT change the ROW-SKIP behavior itself (a malformed row is still
  skipped from the result set, unchanged) — only the reporting changes
  (silent → visible via `corrupt_records`), matching F-10's own explicit
  constraint.
- Do NOT touch `read_exec.rs`'s own already-covered 14 sites (F-10's
  scope) — this task is only the byte-level twins' callers.
- Do NOT redesign `filter_stream`/`filter_stream_tx`'s stream-based API
  unless you found a genuine live caller (see point 2's investigation
  step) — speculative redesign of dead code is out of scope.
- Tests only via `./scripts/test.sh` (or `cargo t`/`cargo tl`), never raw
  `cargo test`.
- `cargo fmt -p shamir-engine -- --check` and
  `cargo clippy -p shamir-engine --all-targets -- -D warnings` must be
  clean.

## Verification the orchestrator will run

```
cargo fmt -p shamir-engine -- --check
cargo clippy -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- corrupt
./scripts/test.sh @engine
```
