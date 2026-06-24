IMPLEMENTATION TASK (TDD, engine). Do NOT commit, do NOT push. Tests ONLY via ./scripts/test.sh (raw `cargo test` is blocked by a perimeter guard). Use CARGO_TARGET_DIR=D:\dev\rust\.cargo-target-bench only for benches (not needed here).

Goal: make the engine HONOUR the keyset (`Pagination::After`) DTO added in the previous task — a sorted-index SEEK from a key, instead of offset pagination. Single-column ORDER BY MVP.

Prereq (already landed): `crates/shamir-query-types/src/read/limit.rs` has `Pagination::After { key: Vec<QueryValue>, limit: Option<u64> }` with accessor `pagination.keyset() -> Option<(&[QueryValue], Option<u64>)>`.

Read these FIRST (they are the templates to mirror):
- crates/shamir-engine/src/table/read_planner.rs:403 `try_plan_order_limit_fast_path` — eligibility for ORDER BY + LIMIT K (single order_by item, sorted index covers the field, no where/group_by/distinct/count_total/aggregates). Returns `(index_name, take, skip, direction)`.
- crates/shamir-engine/src/table/read_planner.rs:~340-385 — the sorted-index RANGE scan eligibility returning `(idx_name, lo, hi, residual)` (Between/Gte/Lte). This shows the bounded-range machinery + `encode_filter_value_for_sort` (sort_codec encoding of a bound). The keyset seek is a half-open range: lower bound = seek key (EXCLUSIVE), no upper (asc); upper = seek key (exclusive), no lower (desc).
- crates/shamir-engine/src/table/read_index_scan.rs:360 `read_order_limit_fast` — the EXECUTOR to mirror: pulls ids from the sorted index (`lookup_first_k` asc / `lookup_last_k` desc), loads bytes, projects. Find the sorted-index manager's BOUNDED range-lookup method (in the same manager as lookup_first_k/last_k — read it) for "K entries strictly after a key in direction D".
- crates/shamir-engine/src/table/read_exec.rs:513-530 — the DISPATCH site where `try_plan_order_limit_fast_path` is consumed. Wire the new keyset path into the same dispatch (try keyset BEFORE/alongside the order-limit fast path).
- crates/shamir-engine/src/table/read_planner.rs:445 `encode_filter_value_for_sort` — sort_codec encoder for a scalar. You need the SAME for a `QueryValue` (the keyset key is QueryValue, not FilterValue): either add a `encode_query_value_for_sort` sibling, or convert QueryValue→FilterValue scalar, reusing `sort_codec::encode_{i64,f64,str,bool,null,bytes}`.

What to build:
1. `try_plan_keyset_seek(query, interner) -> Option<(index_name, encoded_key, limit, direction)>` in read_planner.rs: eligibility = single order_by item + sorted index covers field + no where/group_by/distinct/count_total/aggregates + `query.pagination.keyset()` is Some + the key has exactly ONE element (single-column MVP; multi-column → return None = correct fallback to full scan, NOT wrong). Encode the single seek value via sort_codec.
2. `read_keyset_seek(...)` executor in read_index_scan.rs mirroring read_order_limit_fast, but using the bounded range lookup from the encoded key (EXCLUSIVE bound, direction-aware), take = limit. Set `stats.index_used = Some("sorted_idx_<n>_keyset")`.
3. Dispatch wiring in read_exec.rs.

Semantics to get right:
- ASC: return rows whose order key is STRICTLY GREATER than `key`, in ascending order, up to `limit`.
- DESC: strictly LESS than `key`, descending, up to `limit`.
- Exclusive bound (the seek key itself is NOT returned — that's the row the client already saw).
- No offset (keyset replaces offset).

Tests (TDD, engine — follow CLAUDE.md test layout, tests/ dir + mod.rs manifest):
- 🔴 first: build a table with a sorted index, insert rows with known order-key values, query `After { key: [v], limit: k }` asc → expect exactly the k rows strictly after v in order; same desc. Assert `stats.index_used` is the keyset label (proves the index path, not a full scan).
- 🔴 edge: key at/after the last row → empty; limit larger than remaining → all remaining.
- 🟢 implement until green.
- Run: ./scripts/test.sh -p shamir-engine (read FULL output; never pipe|grep). Then ./scripts/test.sh once at the end.

Keep the diff surgical; match surrounding style. Imports at top. End with a final assistant message: the new fn/file names, the bounded-lookup method you reused, and test pass count.

RATE-LIMIT: do this YOURSELF in a single agent — NO sub-agents. Use grep/view directly.
