# Brief: CR-D4 — cursor path single-serialization fix + upfront-reserve ops docs (#785, N-4 + N-5)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

This brief covers two independent, unrelated findings from the `@fh`
post-Wave-C review (N-4, N-5). Land both; they touch disjoint files.

## Part 1 (N-4, MED) — cursor pages are still serialized twice

### Problem — confirmed by reading the code directly

CR-B2 (#768) gave the plain `Execute` path (`handler.rs::execute`)
single-serialization: `execute` serializes the final `DbResponse` envelope
EXACTLY ONCE (`rmp_serde::to_vec_named(&final_response)`,
`handler.rs:626`), uses those same bytes both to shrink the RI-15 budget
reservation and to stash via `stash_serialized_response` (`byte_budget.rs`),
and `RequestHandler::handle` (`handler.rs:451-455`) reuses the stashed bytes
instead of encoding the response a second time.

The cursor path (`cursor_handlers.rs::create_cursor` / `::fetch_next`) never
got this. `enforce_page_budget` (`cursor_handlers.rs:262-322`) serializes
`page: &QueryResult` ONLY to measure its size for the too-large check and
the RI-15 budget shrink (`rmp_serde::to_vec_named(page)`,
`cursor_handlers.rs:277`) — it discards those bytes immediately. Both
`create_cursor` and `fetch_next` then return `DbResponse::CursorPage {
cursor_id, page, has_more }`, and `RequestHandler::handle`'s
`take_stashed_serialized_response()` call finds nothing stashed (cursor ops
never call `stash_serialized_response`) and falls through to its own fresh
`rmp_serde::to_vec_named(&response)` — a full second encode of essentially
the same payload, on every single cursor page. Confirm this yourself by
re-reading `cursor_handlers.rs:262-322` and `handler.rs:438-456` before
touching anything — the double-encode is real, not a misreading.

**Correction to an existing, misleading doc comment**: `enforce_page_budget`'s
doc comment (`cursor_handlers.rs:241-245`) claims it's "matching `execute()`'s
choice to measure the payload alone... not the full `DbResponse` envelope".
This is WRONG — re-read `handler.rs:626`: `execute()` serializes
`&final_response`, which IS the full `DbResponse` envelope (`DbResponse::Batch
{ response }` or `DbResponse::Error { .. }`), not some inner payload alone.
Fix this comment as part of this task (see "Docs" below) — it materially
mischaracterizes the very pattern this task is asked to replicate.

### Fix — measure and stash the FULL `DbResponse`, not the bare `QueryResult`

The target shape: serialize the complete `DbResponse::CursorPage { cursor_id,
page, has_more }` value exactly once, use those bytes for the too-large
check + RI-15 budget shrink (mirroring `execute()`'s real behavior — the
wire envelope, not just the inner payload, which also means the byte-budget
number now includes the few bytes of `cursor_id`/`has_more`/enum-discriminator
framing, correctly capturing what actually goes out over the socket — this
is what `execute()` already does for the plain path, not a scope expansion),
then stash them via `stash_serialized_response` so `RequestHandler::handle`
reuses them instead of re-encoding.

**The subtlety that makes this non-trivial, read carefully before coding**:
`enforce_page_budget` currently runs BEFORE `cursor_id` is minted in
`create_cursor` (`next_cursor_id()` is called at `cursor_handlers.rs:1104`,
strictly AFTER the `enforce_page_budget` call at line 1072) — the full
`DbResponse::CursorPage` value literally does not exist yet at the point the
budget/size check needs to run. Additionally, `create_cursor`'s SUCCESS path
branches AGAIN after the budget check: the `!has_more` early return
(line 1121) returns `DbResponse::CursorPage{..}` directly, but the `has_more`
branch calls `self.cursor_registry.register(...)` (line 1146) which can
ITSELF fail (`CursorLimitExceeded` / generic `cursor_error`) and return a
COMPLETELY DIFFERENT `DbResponse` variant. If you stash the `CursorPage`
bytes before knowing whether `register()` will succeed, a `register()`
failure would leave STALE stashed bytes for a response that is never
actually returned — `RequestHandler::handle` would then serve the WRONG
payload to the client (the abandoned `CursorPage`, not the actual
`CursorLimitExceeded`/`cursor_error` response). This would be a real wire
corruption bug, not just a missed optimization — treat avoiding it as the
hard constraint on this task, more important than the perf win itself.

Required shape (adapt as needed, but preserve this ordering property —
**stash only once the ACTUAL final response value is fully decided, never
speculatively**):

1. In `create_cursor`: move `let cursor_id = self.next_cursor_id();` to run
   BEFORE the budget/size check instead of after (right after `has_more` is
   computed, before calling into the budget-check function). This is
   harmless: `next_cursor_id()` is a plain counter mint with no side effect
   beyond incrementing — minting it slightly earlier and never using it (the
   too-large-rejection / `register()`-failure paths) simply "burns" an id,
   the same tradeoff the RI-15 upfront-reserve already accepts elsewhere
   (reserved-then-released on a rejected path). Build
   `let response = DbResponse::CursorPage { cursor_id: CursorId(cursor_id),
   page, has_more };` right after.
2. Rename/adapt `enforce_page_budget` to accept `&DbResponse` (the freshly
   built `response` above) instead of `&QueryResult`, serialize IT once, run
   the existing too-large check and RI-15 shrink/acquire logic against those
   bytes exactly as today, and on success RETURN the serialized `Vec<u8>` to
   the caller (do NOT call `stash_serialized_response` inside this function
   — see point 3 for why the stash call must live at the caller, not here).
   On the too-large rejection, behavior is unchanged (`Err`, guard released
   via `Drop`, no stash needed since an error response is small and gets a
   normal fresh encode).
3. Back in `create_cursor`, after the budget-check function returns
   `Ok(bytes)`:
   - `!has_more` branch: this `response` IS the final return value — call
     `stash_serialized_response(bytes)` immediately before `return response;`.
   - `has_more` branch: call `register(...)`. On `Ok(_)`: THIS `response` is
     now confirmed final — call `stash_serialized_response(bytes)` then
     `return response;`. On `Err(..)`: DISCARD `bytes` (do nothing with
     them — let them drop) and return the existing
     `CursorLimitExceeded`/`cursor_error` response exactly as today (that
     rare failure path takes the pre-existing fresh-encode hit at
     `RequestHandler::handle`, same as any other error response in the
     codebase — this is an acceptable, deliberate non-optimization of a
     rare path, not an oversight).
4. `fetch_next` is simpler: `cursor_id` (function parameter) and `has_more`
   are both already known by the time the budget check needs to run (see
   `cursor_handlers.rs:1350-1454` — `has_more` is set inside the dispatch
   match before `enforce_page_budget` is called at line 1451), and there is
   NO further branch between the budget check and the actual return — the
   function unconditionally returns `DbResponse::CursorPage{cursor_id, page,
   has_more}` at line 1473 once the budget check passes and the cursor state
   is updated. Build the full `response` value right before calling the
   (adapted) budget-check function, get back `Ok(bytes)`, call
   `stash_serialized_response(bytes)` once, THEN do the existing state
   mutation (`state.seek_key = ...` etc.) and registry-remove-on-exhaustion,
   then `return response;` (the SAME value already serialized — do not
   rebuild a fresh `DbResponse::CursorPage` literal that could accidentally
   diverge from what was stashed).

Verify after implementing: a test that asserts the wire bytes actually
served for a successful `CreateCursor`/`FetchNext` are byte-identical to
what a fresh `rmp_serde::to_vec_named(&response)` would produce (regression
guard against the stash/actual-response divergence risk called out above),
plus a test that a `CursorLimitExceeded` rejection (register failure) still
returns the CORRECT error response body, not stale `CursorPage` bytes (this
is the specific bug class this task must not introduce — the register-limit
test harness already exists per CR-A2/CR-C2's cursor cap tests, extend
rather than duplicate it).

## Part 2 (N-5, MED) — document the upfront-reserve concurrency math

### Problem

CR-B2 made `execute()` reserve `batch.limits.max_result_size` UPFRONT from
the RI-15 global budget (`handler.rs:563-567`), before execution — a
deliberate, already-correct design (gates execution-time memory, not just
write-path residency). Consequence: since the client-side default equals
the server cap (per `07-operations.md`'s own example, `max_result_size_bytes:
1 GiB`), the server can run at most `max_inflight_response_bytes /
max_result_size_bytes` concurrent batches AT ANY GIVEN CAP CONFIGURATION —
regardless of how small the ACTUAL responses turn out to be. With the doc's
own worked example (`max_inflight_response_bytes: 4 GiB`,
`max_result_size_bytes: 1 GiB`) that's only **4 concurrent batches**, which
would surprise an operator sizing this purely as "a memory cap," not also as
an implicit concurrency limiter. This consequence is not documented anywhere
today.

### Fix

Add a short paragraph to `docs/guide-docs/guide/07-operations.md`, in
Russian (matching the file's existing language), placed right after the
existing `max_inflight_response_bytes` validation paragraph (currently
`07-operations.md:197-202`, ends with "...действует только per-batch cap
`max_result_size_bytes`."). State plainly, with the actual numeric division
worked through using the doc's own example values (1 GiB / 4 GiB):

- `max_inflight_response_bytes` is not just a memory ceiling — because
  CR-B2's reservation is UPFRONT and PESSIMISTIC (the batch's own
  `max_result_size`, clamped to the server cap, not the eventual real
  response size), it doubles as a hard cap on concurrently-EXECUTING
  batches: `effective_concurrency ≈ max_inflight_response_bytes /
  max_result_size_bytes` (a batch that sets a smaller
  `limits.max_result_size` reserves less and so contributes less to this
  division — the bound is the WORST case, not the typical case).
  Worked example with the doc's own config values: `4 GiB / 1 GiB = 4`
  concurrent batches at most, even if every actual response is a few KiB.
- Operator guidance: size `max_inflight_response_bytes` with this division
  in mind, not purely as a memory budget — if legitimate concurrency needs
  to exceed what the memory cap alone would suggest, either raise
  `max_inflight_response_bytes` accordingly or have clients set a smaller
  per-batch `limits.max_result_size` (lowers each reservation, raising
  the number of batches the SAME cap admits concurrently).

Keep it a short, self-contained paragraph (4-6 sentences) — this is an
operator-facing clarification of an already-shipped, already-correct
mechanism, not a design change. Do not touch the mechanism itself
(`byte_budget.rs`, `handler.rs`'s upfront-reserve call) — this half of the
task is doc-only.

## Docs (both halves)

- Fix the misleading `enforce_page_budget` doc comment identified in Part 1
  (the "matching execute()'s choice to measure the payload alone" claim) to
  accurately describe what both paths now do: measure and single-serialize
  the FULL wire envelope (`DbResponse`), not an inner payload alone.
- `07-operations.md`: the new N-5 paragraph (see Part 2 above).
- `CHANGELOG.md`: one bullet under `[Unreleased]` for N-4 (cursor path now
  single-serializes, mirroring CR-B2) — reuse/extend the existing RI-15
  bullet rather than inventing a new top-level entry, since this is a direct
  continuation of that same mechanism's rollout. N-5 is docs-only and does
  not need its own CHANGELOG bullet (no behavior changed) — a short mention
  folded into the same RI-15 bullet is enough if you think it adds value,
  otherwise skip it.

## Tests (TDD — write failing tests first)

In `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs` (or a
new focused test module alongside it if that file is already large — check
before deciding):

- **Single-serialization regression**: for both `CreateCursor` and
  `FetchNext`, assert the bytes actually served match a fresh
  `rmp_serde::to_vec_named(&response)` of the SAME logical response (proves
  no divergence between what was stashed and what was returned). If the
  existing test harness doesn't expose the raw wire bytes directly, add the
  minimal plumbing to observe them (check how `byte_budget_upfront_reserve_tests.rs`
  or similar CR-B2 tests already verify this for the plain `Execute` path —
  mirror that approach rather than inventing a new one).
- **Register-failure does not leak stale bytes**: force a `CursorLimitExceeded`
  (open cursors up to the per-session cap, then attempt one more) and assert
  the response body is the CORRECT `CursorLimitExceeded` error — not a
  `CursorPage` (this is the specific correctness risk flagged in Part 1's
  "subtlety" section; a naive stash-too-early implementation would fail this
  test).
- **Regression**: every existing cursor test stays green — this task must
  not change any cursor's OBSERVABLE response content, only how many times
  it gets serialized internally and when the RI-15 guard is stashed.

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All must pass before returning. Primary code area: `shamir-server`
(`cursor_handlers.rs`, `byte_budget.rs` if any signature needs adjusting,
`handler.rs` only if the misleading comment needs fixing there too — check
whether `execute()`'s own comment needs any wording sync, though its
BEHAVIOR is not changing, only cursor_handlers.rs's is), its tests,
`07-operations.md`, `CHANGELOG.md`. Do NOT touch CR-D1/CR-D2/CR-D3's
pagination-mode or numeric-comparison logic — this task is purely about
serialization/stash plumbing and one docs paragraph, disjoint from that
code.
