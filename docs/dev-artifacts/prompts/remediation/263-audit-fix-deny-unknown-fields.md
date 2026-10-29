# 263 — audit-fix-deny-unknown-fields (P1 audit follow-up: H5 stage C, reject unknown request keys)

Source: post-merge audit of PR #323 + design decision by @oh (2026-10-29, binding). Plan row H5: "BatchOp dispatch is unambiguous and rejects unknown fields". Crate `shamir-query-types` (+ explicit known-key checks where serde attributes cannot work). Read first: `CLAUDE.md`, `crates/shamir-query-types/src/wire/db_message.rs:26-31` (v4: unknown BatchRequest keys are rejected), `ops/batch/req/batch_request.rs:51` (existing `deny_unknown_fields`), precedent `ops/**/repo_ops.rs` + tests `repo_ops_tests.rs:6-49`.

## Policy (decided)
Reject unknown keys in EVERY request-side type reachable from `BatchOp` / `BatchRequest` under v4: no strict mode, no `query_version` bump. Response types stay lenient so older clients can read newer servers. Open user-data maps stay open: `QueryValue` payloads (`values`, `set`, `key`, `value`), the `queries` map, `interner_epochs`, `bind`.

## Change
1. Add `#[serde(deny_unknown_fields)]` to the plain derived structs reachable from `BatchOp`: all `*Op` types (read/write/admin/ddl/subscribe/replication/call/sub-batch/for-each), `ReadQuery` and its clause structs (order/group/having/pagination/join/select options), the `*Select` option structs, `BatchLimits`, `SubBatchOp`, `ForEachOp`, `CallOp`, `FilterExpr`, `Cond`, the struct form of `FnCall`. Partial `limits` maps must keep working (missing keys still take defaults).
2. `Filter` (internally tagged enum, `filter_enum.rs`): attribute at enum level `#[serde(tag = "op", deny_unknown_fields)]` (verify this combination is valid for the serde version in the lockfile; otherwise implement an explicit known-key check per variant and say so).
3. `TableRef` has a hand-written `Deserialize` (`ops/table_ref.rs`): add an explicit known-key check in its map form.
4. `QueryEntry` uses `#[serde(flatten)]` (`ops/batch/ops/query_entry.rs`): exempt - its own keys are consumed first and the remainder is handed to the op type, which rejects unknown keys. Do NOT add the attribute there (it is incompatible with flatten).
5. `FieldRuleDto` (`schema_ops.rs`) flattens `ConstraintsDto`: decode it through a non-flattened mirror struct with `deny_unknown_fields` + `#[serde(from = ...)]` (Serialize unchanged, byte-identical). The catalogue does not use serde for it (`schema_codec.rs`), so stored records are unaffected.
6. RULE before adding the attribute to any type: grep the workspace for a RESPONSE decode or STORAGE/catalogue decode of that same type (client crates, `shamir-engine*`, `shamir-db`, recovery/replication paths) and leave lenient any type that is also decoded from data written by an older/newer binary. List every such type you left lenient in the final report.
7. Update the v4 comment in `wire/db_message.rs` to say the rejection covers the whole request tree.
8. Stage 262 (a producer-vs-DTO wire fixture matrix) lands in parallel and lists known producer/DTO gaps: if `crates/shamir-query-builder/tests/fixtures/wire_ops_matrix.json` exists in your worktree after a merge, read its `known_gap` entries; a key a first-party builder emits must be MODELLED (add the field to the DTO only if the engine already supports it; otherwise report it) rather than rejected.

## Tests (must fail on the old code)
For each family one test in a topic file under the existing tests dirs: `{"from":"t","wehre":{...}}` (misspelt `where` - today silently dropped and the read becomes a full scan), `{"update":"t","set":{},"expected_verison":1}`, `{"insert_into":"t","values":[],"return_results":true}`, `pagination` with `limt`, a field rule with `requird`, a nested filter with a stray key, a `TableRef` map form with a stray key, a sub-batch op with a stray key. Positive controls: open maps (`values`, `set`, `bind`) accept arbitrary keys; partial `limits` decode; a `queries` entry with `return_result`/`after`/`when` decodes. Errors must name the unknown key (serde does for the attribute; keep that for the manual checks).

## Rules
Touch only `shamir-query-types` (+ the wire doc comment). No new dependency, no `#[allow]`, no visibility widening, layout limits (<=7 entries/folder, <=1000 lines/file - many DTO files are small; tests go to existing tests dirs, new topic file if needed), imports at top, `mod.rs` re-exports only, JSON literals multi-line. Another stage (261) rewrote `BatchOp` decoding in `ops/batch/ops/batch_op.rs`; do not touch that file.

## Local gates (only these)
`cargo fmt -p shamir-query-types -- --check`; `cargo clippy -p shamir-query-types --all-targets -- -D warnings`; `cargo check -p shamir-query-builder -p shamir-engine-query -p shamir-engine -p shamir-db -p shamir-server -p shamir-client --all-targets`; `python -I scripts/ci/check_layout.py` (ignores untracked files - count direct entries yourself with ls). One cargo process at a time, CARGO_BUILD_JOBS=2. Report real exit codes.
