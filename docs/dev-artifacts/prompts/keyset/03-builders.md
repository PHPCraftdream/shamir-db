IMPLEMENTATION TASK (TDD). Do NOT commit, do NOT push. Tests ONLY via ./scripts/test.sh for Rust (raw `cargo test` is blocked); TS tests via `npx vitest run` inside crates/shamir-client-ts.

Goal: expose KEYSET (seek) pagination in the Rust AND TS query builders, with unit wire-shape tests + a TS e2e against a real server. The DTO + engine already land: `Pagination::After { key: Vec<QueryValue>, limit: Option<u64> }`, wire tag `"After"` (PascalCase, consistent with `LimitOffset`/`Page`), engine seek path proven.

### Part 1 — Rust builder
File: crates/shamir-query-builder/src/query/query.rs (mirror the existing `.limit()` / `.offset()` / `.page()` setters at ~line 148-185).
- Add `pub fn after(mut self, key: Vec<X>, limit: Option<u64>) -> Self` that sets `self.pagination = shamir_query_types::read::Pagination::after(key_as_query_values, limit)`.
- The DTO key is `Vec<QueryValue>`. Pick the most ergonomic builder signature consistent with this crate's conventions (the builder uses `FilterValue` / `val::*` elsewhere; if you accept `Vec<FilterValue>` or `impl Into<QueryValue>` items, convert to `QueryValue` — find how the crate already converts FilterValue↔QueryValue, e.g. in val/ or wire/). State your chosen signature.
- Unit test (follow CLAUDE.md test layout): `Query::from("t").order_by_asc("score").after(vec![...], Some(2)).build()` → assert the resulting `ReadQuery.pagination == Pagination::After { key, limit: Some(2) }`.
- Run: ./scripts/test.sh -p shamir-query-builder

### Part 2 — TS builder
Files: crates/shamir-client-ts/src/core/builders/query.ts (mirror `.limit()`/`.page()` ~line 174) + crates/shamir-client-ts/src/core/types/query.ts (the `Pagination` union ~line 111).
- Extend the `Pagination` type union with: `{ mode: 'After'; key: WireValue[]; limit?: number }` (PascalCase `'After'` to match the existing `'LimitOffset'`/`'Page'`). Use the project's `WireValue` type for key elements (check types/write.ts).
- Add a `PaginationMode` `'after'` internal state + an `after(key: WireValue[], limit?: number): this` method that switches the mode and stores key+limit. `build()` must emit `{ mode: 'After', key, ...(limit!=null?{limit}:{}) }`.
- Unit test (src/core/builders/__tests__/query.test.ts — extend it): `Query.from('t').orderByAsc('score').after([...], 2).build()` → `pagination` deep-equals `{ mode: 'After', key: [...], limit: 2 }`; and without limit → no `limit` key.
- Run: npx vitest run src/core/builders/__tests__/query.test.ts

### Part 3 — TS e2e (real server)
File: crates/shamir-client-ts/src/__tests__/e2e-keyset.test.ts (NEW). Use the shared harness (e2e-harness.ts: startServer / connectAdmin / uniqueDbName / setupDb / br). Pattern (mirror e2e-data.test.ts pagination cases):
1. createTable + a SORTED index on `score` (createIndex with `{sorted:true}` — check the exact ddl builder option).
2. Seed ~8 rows with distinct increasing `score`.
3. Page 1: `Query.from('users').orderByAsc('score').limit(3)` → first 3.
4. Take the last row's `score` → Page 2: `.orderByAsc('score').after([lastScore], 3)` → next 3, strictly after, no overlap with page 1, correct order.
5. Assert: page2 scores are all > page1's last, contiguous, length 3.
- Run: npx vitest run src/__tests__/e2e-keyset.test.ts (the harness skips if the server binary is absent — it IS built; resolve via CARGO_TARGET_DIR-aware path the harness already handles).

Keep diffs surgical, imports at top, match surrounding style. End with a final assistant message: chosen Rust signature, TS shape, and pass counts for all three parts.

RATE-LIMIT: do this YOURSELF in a single agent — NO sub-agents. Use grep/view directly.
