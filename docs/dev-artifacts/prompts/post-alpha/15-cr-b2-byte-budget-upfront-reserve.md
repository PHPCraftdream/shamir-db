# Brief: CR-B2 — byte budget upfront-reserve + single serialization (#768)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem — verified against the current tree 2026-07-23

### R-2: the RI-15 budget is acquired too late to bound execution-time memory

`ShamirDbHandler::execute` (`crates/shamir-server/src/db_handler/handler.rs:451-599`)
acquires the RI-15 `ByteBudget` reservation AFTER `self.db.execute_as(...)`
has fully run (line 535) and the response has been fully materialized —
the acquire call is at lines 585-590, gated only by
`self.byte_budget.cap().is_some()`. This means N concurrent requests can
all execute to completion and hold their full result in memory BEFORE any
of them is throttled by `acquire`. The budget genuinely bounds only
write-path residency (how long serialized bytes sit around waiting for the
socket write), not execution-time peak memory — CR-A7 already reworded the
CHANGELOG's RI-15 bullet to describe this honestly; once this task lands,
that wording needs to change again to describe the real (fixed) semantics.

### P-2: the same response gets serialized twice

Still inside that block: line 586 does
`rmp_serde::to_vec_named(&response)` (where `response` is the
`BatchResponse`, NOT the enclosing `DbResponse`) purely to measure
`.len()` for the budget acquire call — the resulting `Vec<u8>` is
discarded immediately after `.len()` is read. Then, back in
`RequestHandler::handle` (same file, line 443), the SAME logical
response — now wrapped as `DbResponse::Batch { response }` — gets
serialized AGAIN for the actual wire bytes. For a large response this is
a full extra allocation + encode pass on exactly the path RI-15 is meant
to protect.

## Fix — upfront pessimistic reserve, shrink to actual, serialize once

### 1. `ByteBudgetGuard` needs a shrink operation

In `crates/shamir-server/src/byte_budget.rs`: add a method on
`ByteBudgetGuard` (e.g. `shrink_to(&mut self, new_bytes: usize)`) that:
- No-ops if `self.inner` is `None` (unbounded budget) or `new_bytes >=
  self.bytes` (shrink-only; never grows via this path — this reservation
  scheme only ever estimates HIGH then narrows down, so `new_bytes` should
  always be `<= self.bytes` in the intended call sites, but defend against
  the edge case by treating `new_bytes >= self.bytes` as a no-op rather than
  panicking or growing).
- Computes `delta = self.bytes - new_bytes`, does
  `inner.used.fetch_sub(delta, Ordering::AcqRel)`, calls
  `inner.notify.notify_waiters()` (same release pattern `Drop` already
  uses), and updates `self.bytes = new_bytes` so a later `Drop` releases
  the CORRECT remaining amount (not double-releasing the shrunk delta).
- **Overshoot edge case** (the final serialized `DbResponse` envelope can
  be a handful of bytes larger than the raw estimate, e.g. enum
  discriminator framing on top of the inner payload) — do NOT try to
  re-acquire the delta via a blocking call at this point (the response is
  already computed and holding it hostage behind a fresh `acquire().await`
  risks deadlocking against the very budget this guard already holds a
  slice of). Handle this with a small non-blocking "grow" path instead: add
  a `grow_unchecked(&mut self, extra_bytes: usize)` (or fold into
  `shrink_to` by accepting `new_bytes > self.bytes` as an unconditional
  `fetch_add` rather than a no-op — your call on the cleanest shape) that
  just adds the shortfall to `inner.used` without waiting or checking the
  cap, and bumps `self.bytes` accordingly so `Drop` still releases the
  right total. Document this as an accepted, bounded (a few bytes of
  envelope framing, not unbounded) overshoot — the alternative (blocking
  here) is strictly worse.
- Add unit tests for the guard itself (in whatever test module already
  covers `byte_budget.rs`, e.g. `crates/shamir-server/src/tests/` — find
  it) proving: shrink reduces `budget.used()` by the right amount and wakes
  a parked waiter; a repeated/no-op shrink (new size >= old) doesn't
  under/over-release; `Drop` after a shrink releases only the REMAINING
  (already-shrunk) amount, not double-counting.

### 2. `ShamirDbHandler::execute` — reserve upfront, serialize once, shrink

Restructure the flow (still `crates/shamir-server/src/db_handler/handler.rs`,
`execute`, lines ~451-599):

1. Right before `let exec_result = self.db.execute_as(...)` (line 535,
   AFTER the version/admin-gate/read-only/HMAC checks that can cheaply
   reject a request without touching the budget at all — don't move the
   acquire earlier than this, those checks should stay budget-free fast
   rejects), if `self.byte_budget.cap().is_some()`: acquire a reservation
   using `batch.limits.max_result_size` (already clamped to the
   server-side cap a few lines above, lines 472-475) as the pessimistic
   upfront estimate. Hold this guard in a local `Option<ByteBudgetGuard>`
   for the rest of the function (don't stash it into the task-local yet —
   that still happens once, at the very end, same as today).
2. Let the existing `match exec_result { Ok(...) => ..., Err(...) => ... }`
   logic run exactly as it does today (slow-query logging,
   `persist_table_lifecycle`, subscription activation) — no changes there.
3. At the SINGLE point where the function currently returns
   `DbResponse::Batch { response }` (success path) or
   `DbResponse::Error { .. }` (error path), converge both branches into a
   single tail: build the final `DbResponse` value first, THEN serialize
   it ONCE via `rmp_serde::to_vec_named(&final_response)`, THEN (if a guard
   was acquired in step 1) call `guard.shrink_to(bytes.len())`, stash the
   guard via `stash_guard` (unchanged from today), and ALSO stash the
   already-serialized `bytes` for reuse by the caller (see step 3 below) —
   add a new task-local next to `PENDING_RESPONSE_BUDGET_GUARD` in
   `byte_budget.rs`, e.g. `PENDING_SERIALIZED_RESPONSE:
   RefCell<Option<Vec<u8>>>`, with `stash_serialized_response(bytes:
   Vec<u8>)` / `take_stashed_serialized_response() -> Option<Vec<u8>>`
   helpers mirroring `stash_guard`/`take_stashed_guard`'s exact pattern
   (same `run_with_guard_slot` scope covers both — check whether that
   function needs to scope a second task-local, or whether one combined
   struct is cleaner; keep it consistent with the existing style rather
   than introducing a divergent pattern).
   - This requires restructuring `execute`'s current `match exec_result {
     Ok(mut response) => { ...; DbResponse::Batch { response } } Err(e) =>
     DbResponse::Error { .. } }` so the shared serialize+shrink+stash tail
     runs AFTER the match produces a `DbResponse` value, not inside the
     `Ok` arm only (today only the success path measures/acquires
     anything — extend that so BOTH paths get the same
     reserve-then-shrink-to-actual treatment, since an error response
     still needs its upfront over-reservation shrunk back down to its
     tiny actual size).
   - `execute`'s return type stays `DbResponse` — do not change its
     signature or any caller's expectations (tests assert on the returned
     `DbResponse` directly).
4. In `RequestHandler::handle` (same file, line 443,
   `rmp_serde::to_vec_named(&response).map_err(...)`): before falling back
   to serializing `response` fresh, check
   `crate::byte_budget::take_stashed_serialized_response()` — if `Some(bytes)`
   is present, use those bytes directly (they're byte-identical to what a
   fresh serialize of the SAME `response` value would produce, since
   nothing mutates `response` between where `execute` built it and this
   point) instead of re-encoding. Every OTHER dispatch arm (the ones that
   don't go through `execute` — `Ping`, `ChangePassword*`, cursor ops,
   etc.) has nothing stashed, so `take_stashed_serialized_response()`
   naturally returns `None` for them and they fall through to the existing
   fresh-serialize path unchanged.

### 3. Cursor path (CR-A5's `enforce_page_budget`) — align, don't duplicate drift

`crates/shamir-server/src/db_handler/cursor_handlers.rs`'s
`enforce_page_budget` (~lines 205-240) has the SAME post-hoc-acquire shape
`execute` used to have. Apply the analogous upfront-reserve-then-shrink
treatment: when `handler.query_limits.max_result_size_bytes < usize::MAX`
(the cap is active — this is the natural pessimistic upper bound for one
page, since `CursorPageTooLarge` already rejects anything past it), acquire
upfront using that cap BEFORE running the pinned-version read for the page,
then shrink to the actual serialized size afterward. When the cap is NOT
active (`usize::MAX`, e.g. some unit-test configs), there's no natural
upfront estimate — keep the existing post-hoc-only behavior for that case
(don't invent an estimate out of nothing; this mirrors the brief's own
"don't over-engineer the unbounded case" precedent from CR-A4). The
too-large rejection check itself must stay AFTER the real page is built
(it inherently needs the actual size) — the upfront reserve only affects
WHEN the RI-15 budget is acquired, not the `CursorPageTooLarge` rejection
logic, which is unchanged.

Whether cursor pages also benefit from the single-serialization dedup (a
`enforce_page_budget` currently serializes the page once already for its
own measurement, and then `CursorPage { page, .. }` gets serialized again
by the wire-encode step) is worth doing the same way if it's a clean fit —
check whether an analogous "stash the serialized page for the wire step to
reuse" hookup is straightforward given `enforce_page_budget`'s call sites
(`create_cursor` and `fetch_next`, search for `enforce_page_budget(self,
&page)`). If it turns out awkward given the cursor response shape, it's
acceptable to defer that specific dedup — the budget-timing fix is the
must-have, the serialization dedup for cursors is a nice-to-have; note
which you did in your final report.

## Tests (TDD — write failing tests first)

In whatever test module covers `byte_budget_exhaustion_tests.rs`
(`crates/shamir-server/src/db_handler/tests/`) and/or a new focused test
file if that one is getting crowded:

- **Upfront gating**: with a budget cap sized for exactly N requests' worth
  of `max_result_size`, launch N+1 concurrent requests whose actual
  responses are all small (well under `max_result_size`) — WITHOUT the
  fix, all N+1 execute freely (nothing gates them since acquire happens
  post-execution and the tiny actual sizes never approach a cap sized for
  N). WITH the fix, the (N+1)th request's EXECUTION (not just its
  write) must block until an earlier one's guard is shrunk/released. This
  is the core regression proof for R-2 — needs a way to observe
  "execution is blocked" vs. "execution proceeded" (e.g. an instrumented
  test hook, a channel the query touches mid-execution, or timing-based
  with generous tolerance — pick whatever this codebase's existing
  concurrency tests already use as a pattern, check
  `byte_budget_exhaustion_tests.rs` for precedent).
- **Shrink reclaims over-reservation**: after one request completes with an
  actual response much smaller than `max_result_size`, `budget.used()`
  must reflect the ACTUAL size, not the estimate — prove the guard's
  `shrink_to` really ran end-to-end through `execute`, not just at the
  unit level.
- **No double serialization**: hard to assert directly at the type level;
  at minimum, existing tests in `byte_budget_exhaustion_tests.rs` must
  stay green (including their existing msgpack-jitter tolerance — this
  session hit this flake class twice already; if a new assertion needs
  one, use the same `+16` byte tolerance pattern already established
  there), and add one clear test asserting the exact byte length used for
  the budget acquisition matches the exact byte length written to the
  wire in an end-to-end request (via whatever test harness already sends
  a real request and can observe both — check for an existing e2e
  fixture rather than inventing one).
- **Cursor path parity**: a cursor `FetchNext` test proving the same
  upfront-then-shrink behavior for `enforce_page_budget` when
  `max_result_size_bytes` is active.

## Docs follow-up (do NOT skip)

`CHANGELOG.md`'s `[Unreleased]` RI-15 bullet currently says (per CR-A7's
just-landed wording) that the budget "bounds write-path residency, not
execution-time memory" and is "not an upfront/pre-execution admission
control gate" — once THIS fix lands, that wording is now WRONG in the
other direction. Update it to accurately describe the new upfront-reserve
+ shrink-to-actual semantics: the budget now DOES gate before execution
begins (using the clamped `max_result_size` as a pessimistic estimate),
then narrows the reservation down to the real serialized size once known.

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All must pass before returning. Primary code area: `shamir-server`
(`byte_budget.rs`, `db_handler/handler.rs`, `db_handler/cursor_handlers.rs`,
tests, `CHANGELOG.md`). Do NOT touch cursor pagination/tie-breaker logic
(CR-A4's territory) or the ACL/validation checks CR-A1/CR-A3 added — this
task only changes WHEN and HOW OFTEN serialization/budget-accounting
happens, not any request-acceptance/rejection logic.
