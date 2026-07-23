# Brief: CR-D1 — keyset cursor tie-run-ceiling livelock (#782, release blocker)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

**This is the highest-severity bug found in the whole cursor campaign —
read this brief fully, reproduce the bug with a real test BEFORE writing
any fix, and be extremely careful about correctness (exactly-once
semantics) in whatever you change.**

## Problem — verified with exact arithmetic by an independent review, re-verify yourself before fixing

`fetch_keyset_page` (`crates/shamir-server/src/db_handler/cursor_handlers.rs`,
~lines 515-651) bounds its internal fetch at `limit_ceiling =
max_cursor_page_size` (default 10,000, `config.rs` ~lines 331-333), but
`tie_skip` (the count of already-returned rows tied at the current ORDER BY
boundary value) grows WITHOUT bound across `FetchNext` calls as a tie run
is paged through (`bookmark_from_tail` counts the WHOLE consumed prefix
each time, not just the newly-returned rows).

Walk it through: paging a tie run with page size `p`, `tie_skip` grows by
`p` per page (`p, 2p, 3p, …`). Once `tie_skip` reaches the ceiling `C`,
`internal_limit = max(p, tie_skip+1).min(C)` clamps to `C` (~line 539-542),
the fetch asks for `C+1` rows (the peek-ahead pattern), the skip-walk
(~lines 595-611) consumes up to `tie_skip` of them, and
`usable_len = min(fetched, internal_limit) - skip_count` (~line 616) can
land at EXACTLY `0`. The ceiling branch (~lines 637-647) then returns
`take = 0` rows with `has_more = (skip_count + 0 < page.records.len()) =
true`, and — critically — the refreshed bookmark
(`finish_keyset_page`/`bookmark_from_tail`, called with
`consumed_from_front = skip_count + take = skip_count`) recomputes to
THE SAME `tie_skip` value the NEXT call started with. Every subsequent
`FetchNext` against this cursor is therefore byte-identical: an empty
page, `has_more: true`, forever — and BOTH SDKs loop on exactly that shape
by design (the TS `CursorIterator`'s `for(;;)` loop in `doNext`,
`crates/shamir-client-ts/src/core/cursor-iterator.ts` ~lines 161-184,
added by CR-C1 specifically to survive a legal empty-but-`has_more`-true
page; the Rust `CursorStream` similarly refetches on an empty page). A
`for await`/`StreamExt` consumer hangs FOREVER while hammering the server
with an `O(C)` scan per round-trip. Every row in the tie run beyond the
first `C` is permanently unreachable through the cursor.

**Trigger is realistic, not adversarial**: `ORDER BY status` /
`ORDER BY category` (or any low-cardinality column) on any table where a
single value has more than `max_cursor_page_size` rows. It can even fire
on the SECOND `FetchNext` call if the first page happened to be entirely
tied (`page_size = max_cursor_page_size`, first page all-tied →
`tie_skip = C` immediately).

**Re-verify this arithmetic yourself** (read the current code at the exact
line numbers above — they may have shifted slightly since this brief was
written) before designing the fix. The existing test
`keyset_tie_run_larger_than_one_page_uses_bounded_retry`
(`cursor_handler_tests.rs`, search for it) uses `CursorLimitsCap::UNLIMITED`,
so it never exercises the ceiling interaction at all — this bug has zero
existing coverage.

## Fix — offset-bookmark fallback when the ceiling genuinely can't make progress

**Preferred design**: when `fetch_keyset_page`'s ceiling branch would
return `usable_len == 0` (the exact stuck condition — NOT the general
"hit the ceiling but still made SOME progress" case, which is fine as-is),
signal this to the caller instead of silently returning a zero-progress
page. The caller (`fetch_next`'s Keyset dispatch arm) then falls back to
the ROW-COUNT OFFSET bookmark for the REST of this cursor's lifetime —
`CursorState`'s `offset` field is ALREADY maintained in parallel even on
the keyset branch today (`fetch_next`, search for
`new_offset = state.offset + outcome.result.records.len() as u64` inside
the Keyset arm) — so this fallback has a ready-made, already-correct
"how many rows have I returned so far" bookmark to continue from. Since
CR-B1 made the pinned-version enumeration + stable sort genuinely stable
at a fixed snapshot, switching coordinate systems mid-scroll (something
CR-A4 deliberately avoided doing per-page, for good reason) is SAFE here
specifically because you're doing it in response to a detected failure
condition, once, with a bookmark (`state.offset`) that has been tracking
the true count all along regardless of which mode was active — not
flipping opportunistically or repeatedly.

Concretely:

1. Give `KeysetPage` (or `fetch_keyset_page`'s return type) a way to
   signal "stuck, cannot make progress via keyset — caller must switch to
   offset mode and retry THIS call that way" instead of returning a
   zero-progress page. A new field/variant is fine — pick whatever shape
   reads cleanest given the existing `KeysetPage` struct.
2. In `fetch_next`'s dispatch (the `match (state.mode, state.seek_key.clone())`
   block, search for it), when this signal comes back from the Keyset arm:
   permanently flip `state.mode` to `PaginationMode::Offset` (so every
   FUTURE `FetchNext` on this cursor goes straight through the offset
   branch, no need to re-detect stuck-at-ceiling every call), clear
   `state.seek_key`/`state.tie_skip` (no longer meaningful once in Offset
   mode), and re-run THIS SAME call's fetch via the offset-mode logic
   (`Pagination::LimitOffset { offset: state.offset, limit:
   effective_page_size }`) so the caller still gets a real page back
   instead of an error or an empty response for the call that TRIGGERED
   the fallback. Consider extracting the offset-arm's existing body into
   a small helper/closure so both the normal `_ =>` arm and this recovery
   path can call the identical logic rather than duplicating it.
3. Document this fallback clearly — a code comment explaining WHY a
   keyset cursor can transition to offset mode mid-lifecycle here
   specifically (unlike CR-A4's per-page mode stability guarantee, which
   is about never flip-flopping OPPORTUNISTICALLY), citing this brief/task
   number.

**Acceptable minimum if the fallback proves too risky to implement
correctly under time pressure**: a new, distinct, typed error (e.g.
`BatchError::CursorTieRunTooLarge` / wire code `cursor_tie_run_too_large`)
returned instead of the silently-looping empty page, so the client at
least gets a clear, actionable error instead of hanging forever. This is
explicitly a FALLBACK option — prefer the real fix (offset-bookmark
fallback) if you can implement and test it correctly; only fall back to
the typed-error minimum if the mode-transition logic turns out genuinely
too risky to get right confidently. State clearly in your final report
which path you took and why.

## Tests (TDD — reproduce the hang FIRST, before any fix)

In `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`:

- **Reproduce the bug**: a cursor over a column where ALL rows share the
  same ORDER BY value (or enough of them to exceed a SMALL configured
  `max_cursor_page_size`, e.g. cap it at 8 via `CursorLimitsCap` and seed
  a tie run of 20 rows), page_size small enough to force multiple
  `FetchNext` calls into the tie run. Drain the cursor in a BOUNDED loop
  (e.g. `for _ in 0..N` with `N` comfortably larger than the expected
  number of pages, NOT an unbounded `loop`/`while has_more`, which would
  literally hang the test suite on the pre-fix code) — assert that
  EITHER every row is returned exactly once within the bounded iteration
  count (if you land the offset fallback) OR a clean, distinct error is
  returned once the ceiling is hit (if you land the typed-error minimum).
  Confirm this test FAILS (hangs past the bounded iteration count, or
  never terminates cleanly) against the CURRENT code before your fix —
  this is the core regression proof.
- **Regression guard**: `keyset_tie_run_larger_than_one_page_uses_bounded_retry`
  and every other existing keyset test must stay green — this fix must
  not change behavior for a tie run that stays within the ceiling.
- If you land the offset-mode fallback: an additional test proving the
  transition itself is exactly-once-correct — e.g. a tie run that's
  SLIGHTLY over the ceiling (say cap 8, tie run of 12, plus 3 more
  DISTINCT-valued rows after the tie run) — drain the whole cursor and
  assert every one of the 15 rows appears EXACTLY once, in the correct
  overall order, proving the mode switch didn't duplicate or skip
  anything at the transition boundary.

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All must pass before returning — and pay EXTRA attention to whether the
full suite run actually completes in reasonable time (a lingering bug in
your own fix could still hang a test). Primary code area: `shamir-server`
(`cursor_handlers.rs`, its tests). Do not touch `create_cursor`'s first
page — it never calls `fetch_keyset_page` (no seek key exists yet on the
first page), so this bug and its fix are scoped entirely to `fetch_next`'s
Keyset branch.
