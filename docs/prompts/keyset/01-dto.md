IMPLEMENTATION TASK (TDD). Do NOT commit, do NOT push. Tests ONLY via ./scripts/test.sh (raw `cargo test` is blocked by a perimeter guard).

Goal: add a KEYSET (seek) pagination DTO variant to OQL — the wire type only, plus serde + a failing-then-passing test. NO planner/engine work in this task (that's the next task).

Target: crates/shamir-query-types/src/read/limit.rs — the `Pagination` enum.

Current state (read it first):
- `Pagination` is `#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]` with `#[serde(tag = "mode")]` (internally tagged), variants `LimitOffset { limit: Option<u64>, offset: u64 }`, `Page { page, page_size }`, `None` (default).
- `resolve(&self) -> (u64, Option<u64>)` maps to (skip, take).

What to add:
- A new variant `After { key: Vec<QueryValue>, limit: Option<u64> }` — seek-pagination: "return up to `limit` rows ordered after the tuple `key`". `key` is the ordered tuple of values matching the query's ORDER BY columns. Use `shamir_types::types::value::QueryValue` (the wire value type — check the exact import path used elsewhere in this crate; do NOT use serde_json::Value, it is BANNED).
- Serde shape (internally tagged): `{ "mode": "after", "key": [...], "limit": <n> }`.

CRITICAL non-obvious finding (verified): `Pagination` currently derives `Copy`. `Vec<QueryValue>` is NOT `Copy`, so adding `After` will BREAK the `Copy` derive. You must:
  1. PREFER: remove `Copy` from the derive and fix every call site that relied on Copy (deref-copies like `*offset`/`*limit`, by-value passes). Grep the workspace for `Pagination` uses; the ripple is mechanical (add `.clone()` / borrow where needed). This is the honest model — keyset IS a pagination mode.
  2. FALLBACK (only if the Copy-removal ripple is large — many sites or hot paths): keep `Pagination: Copy` and instead model keyset as a separate `after: Option<Vec<QueryValue>>` field on `ReadQuery` (read_query.rs) used together with a limit, NOT a Pagination variant. If you take this path, document WHY in a comment and in your final message.
  Decide based on the actual ripple size you find; state which path you took and the site count.

- `resolve()` returns (skip, take) which does NOT fit seek semantics. For `After`, do NOT shoehorn into (skip, take). Either return `(0, *limit)` with a doc note that seek is handled by the planner (next task), OR add a dedicated accessor `keyset(&self) -> Option<(&[QueryValue], Option<u64>)>`. Prefer the dedicated accessor; keep `resolve()` correct for the offset modes.
- Add a constructor `Pagination::after(key: Vec<QueryValue>, limit: Option<u64>) -> Self` (or the ReadQuery field setter in the fallback design).

Tests (TDD):
- 🔴 Write a failing test first (serde round-trip of the `After` variant → exact `{mode:"after",...}` shape; constructor; accessor) in the crate's test module (follow the existing test layout — tests/ dir, mod.rs manifest; see CLAUDE.md test organisation).
- 🟢 Implement until green.
- Run: ./scripts/test.sh -p shamir-query-types   (read the FULL output; never pipe|grep it).
- Also run ./scripts/test.sh once at the end to confirm no cross-crate breakage from the Copy change.

Keep the diff surgical. One primary export per file (CLAUDE.md). Imports at top of file. End with a final assistant message: which Copy-path you chose + site count, the serde shape, and test pass count.

RATE-LIMIT: do this YOURSELF in a single agent — do NOT spawn sub-agents (the service 429s on nested agents). Use grep/view directly.
