# Brief 47 — #1063 round 3: the 3 remaining tests skipped in round 2 for false reasons

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any
git command that mutates the working tree or index. Only edit files; the
orchestrator commits.

## Context

Round 2 added `crates/shamir-engine/src/table/tests/p1063_multi_fts_index_stats_tests.rs`
with 2 passing tests (`two_fts_indexes_different_fields_one_insert_doc_count_and_sum_doc_len_correct`,
`two_fts_indexes_abort_staged_tx_neither_backend_stats_change`) and skipped 3
more, citing "API complexity". **Both cited reasons are factually wrong** —
working examples of exactly what was claimed to be unavailable already exist
in this same test directory. Do not re-litigate whether these tests are
possible; they are. Copy the patterns below directly.

Production code is DONE and correct — still do not touch
`crates/shamir-tx/src/index_write_op.rs`, `fts_ranked_backend.rs`'s
`plan_*`/`apply_in_memory`, `write_ops.rs`'s `apply_index_ops_at_commit`, or
`pre_commit.rs`'s `retract_stale_provenance_ops`. Also do not touch the 2
already-passing tests in `p1063_multi_fts_index_stats_tests.rs` — extend the
file, keep them as-is.

## Test 2 — update: `execute_update_tx` is NOT complex, here is the exact call

Round 2's claim: "`FilterContext<'_>` isn't readily available". False — it's
two lines. Copy this EXACT pattern from
`crates/shamir-engine/src/table/tests/insert_tx_tests.rs:258-297`
(`execute_update_tx_stages_via_update_tx`):

```rust
let interner = tbl.interner().get().await.unwrap();
let refs = new_map();                                   // shamir_types::types::common::new_map
let ctx = FilterContext::new(interner, &refs);           // shamir_engine::query::... — check imports in insert_tx_tests.rs

let op = write::update("docs")
    .set(mpack!({ "title": "new title text here" }))
    .build()
    .unwrap();

let result = tbl
    .execute_update_tx(&op, &ctx, &mut tx, None, &shamir_types::access::Actor::System)
    .await
    .unwrap();
```

Write `two_fts_indexes_update_one_field_only_owner_stats_change`: two FTS
indexes (`title_fts` on `title`, `body_fts` on `body`), insert a row with
both fields, commit. Snapshot both backends' `doc_count()`/`sum_doc_len()`.
Stage a NEW tx, `execute_update_tx` that changes ONLY `title` to different
text, commit. Assert: `title_fts`'s `sum_doc_len()` changed to reflect the
new title's length (doc_count net change is 0 for an update — old bump -1,
new bump +1 on the SAME backend); `body_fts`'s `doc_count()` AND
`sum_doc_len()` are BYTE-IDENTICAL to their pre-update snapshot — not
touched by ANY bump.

## Test 4 — delete: same file, same pattern, trivial

`execute_delete_tx` follows the identical shape (see other tests in
`insert_tx_tests.rs` for delete if present, otherwise mirror the update call
above with the delete builder). Write
`two_fts_indexes_delete_row_correct_backend_only_decremented`: two backends,
insert, commit, snapshot stats, delete the row via a fresh tx, commit.
Assert doc_count decremented by exactly 1 on... actually there is only ONE
row touching both fields, so BOTH backends' `doc_count` should decrement by
1 and `sum_doc_len` by that field's length — the point of this test is that
EACH backend gets EXACTLY ONE decrement bump (not two), not that only one
backend changes. Re-read the brief for #1063 (round 1, already committed at
`docs/dev-artifacts/prompts/release-blockers/45-p1063-bump-fts-stats-provenance.md`)
if the exact assertion shape is unclear — the corruption this whole task
fixes is N² application, so the assertion that matters is "decremented by
exactly 1, not 2" on EACH backend independently.

## Test 3 — the ABA case: `drop_index2(name, None)` already exists, no live-tx op_id needed

Round 2's claim: "op_id must come from a live transaction... `make_repo()`
pattern doesn't provide this." False —
`crates/shamir-engine/src/table/tests/p1008_instance_provenance_tests.rs:305`
calls `tbl.drop_index2("lower_name", None).await.unwrap()` directly, with
`None` for the op_id parameter, in exactly this same `make_repo()`/
`TableConfig::new(...)` test harness. This is the base_index equivalent of
the exact test you need, at `p1008_instance_provenance_tests.rs:353-410`
(`regular_aba_drop_create_same_name_no_field_a_contamination`) — read that
function in full, it is your template. The shape:

```rust
let repo = make_repo();
repo.add_table(TableConfig::new("docs"));
let tbl = repo.get_table("docs").await.unwrap();
let title_key = key_id(&tbl, "title").await;

// Instance A: title_fts on "title"
tbl.create_index_v2(&fts_index_op("title_fts", "docs", "title")).await.unwrap();

// Stage an insert against instance A.
let (mut tx, _guard) = repo.begin_tx(IsolationLevel::Snapshot).await.unwrap();
let rid = tbl.insert_tx(&record_with_str(title_key, "hello world"), Some(&mut tx)).await.unwrap();

// ABA: DROP instance A, CREATE a NEW instance B under the SAME name.
tbl.drop_index2("title_fts", None).await.unwrap();
tbl.create_index_v2(&fts_index_op("title_fts", "docs", "title")).await.unwrap();

repo.commit_tx(tx).await.expect("commit must succeed");

// Instance B must NOT have received instance A's stale bump.
let backend_b = tbl.index2_registry().get_by_name(key_id(&tbl, "title_fts").await).await.unwrap();
let fts_b = backend_b.as_any().downcast_ref::<crate::index2::fts_ranked_backend::FtsRankedBackend>().unwrap();
assert_eq!(fts_b.doc_count(), 0, "...");
assert_eq!(fts_b.sum_doc_len(), 0, "...");
```

Name it `stale_bump_from_dropped_and_recreated_fts_index_not_applied_to_new_instance`.
This is the SINGLE MOST IMPORTANT test of the three — it is the exact ABA
corruption scenario named in the original bug report and is what
distinguishes "provenance exists" from "provenance is actually checked at
retraction time". Do not skip it a second time.

## Gate before you report done

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
./scripts/test.sh -p shamir-engine -- p1063
./scripts/test.sh -p shamir-index
```

All 5 tests in `p1063_multi_fts_index_stats_tests.rs` must PASS (2 already
do — do not break them). Report the exact diff and paste the actual
`nextest` output for the `p1063` filter — all 5 test names, all PASS. If
genuinely one of these three still cannot be written after actually trying
the patterns above (not after a cursory attempt), explain exactly what
specific compiler error or runtime assertion failure you hit, quoting it —
"complexity" is not an acceptable reason a third time.
