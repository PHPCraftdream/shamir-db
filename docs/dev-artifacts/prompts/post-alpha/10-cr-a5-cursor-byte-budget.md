# Brief: CR-A5 — route cursor responses through byte budget + per-page result-size cap (#763)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem — RESOURCE BYPASS, verified against the current tree 2026-07-23

The RI-15 byte-budget acquire lives ONLY inside
`ShamirDbHandler::execute` (`crates/shamir-server/src/db_handler/handler.rs`,
~lines 585-589 — inside the `DbRequest::Execute` handling path). The
`CreateCursor`/`FetchNext` dispatch arms in the same `match` (calling
`self.create_cursor(...)`/`self.fetch_next(...)` in
`db_handler/cursor_handlers.rs`) never acquire anything, and nothing
clamps a cursor page against `query_limits.max_result_size_bytes` either
(that clamp only applies to `batch.limits.max_result_size`, set upfront
for `Execute`'s planner — `handler.rs` ~lines 472-476 — which the cursor's
`TableManager::read_with_encoding` call never goes through). CR-A3 (just
landed, `f048b218`) added a ROW-COUNT cap (`max_cursor_page_size`, default
10,000) which substantially bounds worst-case page size already, but a
byte-size cap and the global budget are independent, complementary
protections and both are still missing.

## Fix — mirror `execute()`'s existing mechanism exactly, do not invent a new one

### 1. Byte budget acquire (the must-do part)

Study `execute()`'s exact block (`handler.rs` ~lines 585-589):
```rust
if self.byte_budget.cap().is_some() {
    if let Ok(bytes) = rmp_serde::to_vec_named(&response) {
        let guard = self.byte_budget.acquire(bytes.len()).await;
        stash_guard(guard);
    }
}
```
This works because `request_loop.rs` already wraps the ENTIRE dispatch
(every `DbRequest` variant, not just `Execute`) in
`byte_budget::run_with_guard_slot` before calling into
`RequestHandler::handle` — the task-local slot is live for cursor
requests too. **No changes needed in `request_loop.rs` or `handler.rs`'s
dispatch wrapping** — `stash_guard`/`take_stashed_guard` already work for
any response path, cursor included.

Add the IDENTICAL block in both `create_cursor` and `fetch_next`
(`crates/shamir-server/src/db_handler/cursor_handlers.rs`), right before
each function's final successful return (`DbResponse::CursorPage {
cursor_id, page, has_more }`) — measure `page`'s (or the whole response's,
match whatever `execute()` measures — check: does it measure the
`BatchResponse` payload alone, or the full `DbResponse` envelope? Mirror
that choice exactly for consistency) serialized size, acquire from
`self.byte_budget`, `stash_guard(guard)`. Do this on EVERY successful
`CursorPage` return in both functions — including CR-A2's early-return
"exhausted first page, not registered" branch in `create_cursor` (that
response still goes out over the wire and its bytes still occupy memory
on the write path — the budget must see it too).

Do NOT add this to `cancel_cursor` (its `CursorClosed` response is tiny
and fixed-size — not worth the complexity; skip it, note why in a
one-line comment if you want).

### 2. Per-page byte-size cap (secondary, cheap addition)

After measuring the serialized size (you're already doing this for the
budget acquire — reuse the same measurement, don't serialize twice), if
it exceeds `self.query_limits.max_result_size_bytes`, REJECT (do not
truncate — truncating a page would corrupt `has_more`/bookmark semantics,
same reasoning CR-A3 used for page_size). Use a clear, distinct error
(check whether an existing `BatchError` variant fits, e.g. something
already used for the batch-level result-size cap, or add
`CursorPageTooLarge`-style if nothing fits — follow CR-A3's
`InvalidPageSize` precedent for how to add a new cursor-specific variant
cleanly: `batch_error.rs` new variant + `Display` + `error_code()` mapping
in `handler.rs`).

Note the ordering interaction with the byte budget: if the page is
rejected for being too large, do NOT acquire budget for it (there's
nothing to write). Measure once, decide (reject vs. proceed), then only
acquire on the "proceed" path.

## Tests (TDD — write failing tests first)

Mirror `crates/shamir-server/src/db_handler/tests/byte_budget_exhaustion_tests.rs`'s
style (real handler, real bounded `ByteBudget`, `tokio::spawn` + timeout to
prove blocking/unblocking) but exercised through cursor calls instead of
`Execute`:

- A bounded budget saturated by two large cursor pages blocks a third
  cursor fetch (either a `CreateCursor` or a `FetchNext`) until one guard
  releases — same shape as `exhaustion_blocks_until_release`.
- A guard acquired for a cursor page is released after the simulated
  writer-task write-error path (mirror
  `release_on_simulated_write_error_recovers_budget`).
- A cursor page whose serialized size exceeds a configured
  `max_result_size_bytes` is rejected with the new/chosen error — and no
  budget is acquired for the rejected attempt (assert `budget.used()`
  stays at its pre-attempt value).
- Regression guard: the existing `byte_budget_exhaustion_tests.rs` (which
  use `ByteBudget::unbounded()` by default) and CR-A1/A2/A3's cursor tests
  must all stay green — this task must not change behavior when the
  budget is unbounded or the page is within size limits.

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All must pass before returning. Stay inside `shamir-server`
(`cursor_handlers.rs`, possibly `batch_error.rs`/`handler.rs` if a new
error variant is needed, tests). Do NOT restructure the byte-budget
acquire timing (upfront-reserve-then-shrink) or eliminate the
measure-then-serialize-again double cost — those are a SEPARATE, already
-queued follow-up task (CR-B2) that lands specifically on top of this one;
keep this task's scope to "wire the EXISTING mechanism into the cursor
path," matching `execute()`'s current (not-yet-optimized) shape exactly.
