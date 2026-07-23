# Brief: CR-C1 — polish batch (#776)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations. Same for TS
typecheck/test commands.

This is a **batch of eight small, independent polish items**, verified
against the current tree 2026-07-23. Land them as one or two commits
(group Rust changes together, TS changes together, if that's cleaner —
your call), but implement and test each item individually; don't let one
item's scope creep into another's. Read the whole brief before starting —
several items touch the same functions.

## R-9 — cursor config validation + docs (Rust)

`crates/shamir-server/src/config.rs::Config::validate()` (~lines 603-672)
currently validates ONLY `security.cursors.max_cursor_page_size == 0`
(~lines 650-654, added by CR-A3). It does NOT validate
`idle_timeout_secs` or `max_cursors_per_session` for zero/invalid values
(declared ~lines 294-301, defaults 16 / 60s). Add validation: reject
`idle_timeout_secs == 0` (a zero idle timeout would evict every cursor
almost immediately, likely not the operator's intent, and is
indistinguishable from a misconfiguration) and `max_cursors_per_session ==
0` (would make `CreateCursor` always fail with `cursor_limit_exceeded`,
silently disabling the whole feature) — OR, if you judge zero is a
legitimate value for either field with real semantics (e.g. "0 sessions
allowed" as an explicit cursor-feature killswitch), document that
decision instead of adding a rejection, matching this codebase's existing
practice of explaining WHY a value is or isn't rejected. Make the call and
state it in a doc comment either way.

Docs: `docs/guide-docs/guide/07-operations.md` documents
`max_inflight_response_bytes` (~lines 163, 197-202) but has NO mention of
`security.cursors.*` anywhere. Add the three keys
(`idle_timeout_secs`/`max_cursors_per_session`/`max_cursor_page_size`,
defaults 60/16/10000) next to the `max_inflight_response_bytes` entry,
matching that entry's format/tone.

## R-10 — TS `CursorIterator`: serialize `.next()`, graceful `return()` (TS)

`crates/shamir-client-ts/src/core/cursor-iterator.ts`'s `next()`
(~lines 113-140) has NO internal serialization — two overlapping calls to
`.next()` (before the first resolves) can both reach `sendRequest()`
concurrently, and on the FIRST call both could issue `createCursor`,
leaking the loser's cursor until idle-timeout (nothing ever calls
`CancelCursor` on it, since the caller only holds the winner's state).
Fix: serialize `.next()` on an internal promise chain (a common pattern:
keep a `private pending: Promise<...> = Promise.resolve(...)` and chain
each call's work onto it, e.g. `this.pending = this.pending.then(() =>
this.doNext())`, returning the chained promise) so a second overlapping
call queues behind the first rather than racing it. Check this file's
existing style/conventions before picking an exact implementation shape.

`return()` (~lines 174-177) currently `throw`s on an unexpected response
kind for `cancel_cursor`. This throw happens during `IteratorClose` —
including the case where `return()` is called as PART OF unwinding an
exception from inside a `for await...of` body (JS runtime calls `.return()`
on the iterator during exception propagation). A throw from `return()` in
that path REPLACES/masks the original exception the loop body threw,
which is almost always the wrong debugging experience. Change it to
swallow the unexpected-kind case with a `console.warn`/logged message (or
whatever this SDK's existing logging convention is — check other files
for precedent) instead of throwing.

## R-11 — Rust `CursorStream`: best-effort cancel after mid-pagination error

`crates/shamir-client/src/cursor_stream.rs`'s `unfold` state machine
(~lines 151-159) collapses to `State::Exhausted` and yields the error on
a mid-pagination failure, WITHOUT ever sending `CancelCursor` — the server
cursor stays pinned (MVCC snapshot held) until the idle-timeout reaper
eventually reclaims it, which could be up to `idle_timeout_secs` later.
Add a best-effort `CancelCursor` fire-and-forget (don't block yielding the
error on it, and don't let a cancel failure mask/replace the original
error — same "don't lose the real error" principle as R-10 above) right
before transitioning to `Exhausted`. If a genuinely clean
fire-and-forget send isn't straightforward given this stream's async
structure (check how `close()` already sends `CancelCursor` for the
precedent to reuse), it's acceptable to instead add ONE clear doc-comment
sentence in the module's cleanup-behavior section explicitly stating this
gap and pointing callers at explicit `close()` as the reliable cleanup
path for the happy-path-abandon case — but PREFER the real fix if it's a
small addition; only fall back to the doc-only path if it turns out
genuinely awkward.

## P-3 — `ByteBudget::acquire`: fast-path setup cost + oversized-acquire docs/test

`crates/shamir-server/src/byte_budget.rs::acquire()` (~lines 113-176)
currently creates AND `enable()`s a `Notified` future (~lines 131-137)
BEFORE the first CAS attempt on EVERY call — including the common
uncontended case where the very first CAS succeeds and the notified
future is immediately dropped unused. The module's own doc comment
describes this as a "lock-free CAS-loop fast path," which is not quite
accurate as currently structured (two `Notify`-related ops happen on every
acquire, contended or not). Restructure: try the CAS loop FIRST; only
create/`enable()` the `Notified` future on the FIRST CAS failure, right
before actually needing to park. Preserve the existing race-safety
reasoning (the comment about `enable()` needing to happen before
re-checking the CAS to avoid a lost wakeup) — that reasoning still applies
to the failure-path notified future, just move WHEN it's constructed, not
the invariant itself.

Also: `acquire`'s existing "admit if `current == 0`" branch (the
oversized-request-doesn't-deadlock-forever escape hatch) is documented in
the module doc but has no DEDICATED unit test as far as this brief's
author found — check `crates/shamir-server/src/tests/byte_budget_tests.rs`
for existing coverage; if none directly exercises `bytes > cap` admitted
via the `current == 0` path, add one, and document (in the test's own
comment) the starvation caveat this branch accepts (an oversized request
admitted this way could, in theory under a pathological release pattern,
prevent OTHER waiters from ever getting a turn while it holds the budget —
state this plainly, matching the module's existing "Fairness" doc-comment
section's honesty about at-least-one-progress vs. strict-FIFO).

## P-4 — `fetch_next`'s double `ReadQuery` clone (offset path)

`crates/shamir-server/src/db_handler/cursor_handlers.rs::fetch_next`'s
offset-bookmark branch (the `_ =>` arm, ~line 1093) clones
`base_query` once (`let mut next_query = base_query.clone();`) — verify
whether the brief's claim of a SECOND redundant clone in this exact
branch is still accurate against the current tree (it may have already
been resolved by an earlier Wave A/B commit — CR-B4's peek-ahead change
touched this exact branch). If you find only one clone here, this item
may already be moot for the offset path — check the KEYSET path
(`fetch_keyset_page`, ~line 546) too, which clones `base_query` once per
INTERNAL retry iteration (by design, since each retry needs a fresh
`query.r#where` overwrite) — that repeated clone is NOT the redundant
kind this item is about (each iteration's clone is necessary, not
duplicated). **Verify what's actually redundant before changing
anything** — if you cannot find a genuine double-clone of the SAME query
value within a single page-fetch after re-reading the current code, say
so explicitly in your report and skip this item rather than inventing a
change. `boundary_filter`/`order_by_field_value` already take references,
not owned values, per the existing code — no signature changes needed if
a redundant clone IS found; the fix is simply removing the second `.clone()`
call and reusing the first.

## P-5 — Rust `unfold` cursor-id write; TS iterator loop + O(1) buffer read

**Rust** (`crates/shamir-client/src/cursor_stream.rs`): the brief's
originating description claims the cursor-id write under the
`std::sync::Mutex` happens after EVERY yielded record. Verify this
against the current code (~lines 209-211) — if the write is ALREADY
guarded to only happen once (an `is_none()`/first-transition check) rather
than unconditionally on every yield, this item is already resolved; say
so and skip it. If it genuinely re-writes unconditionally on every
record, add the guard (only write when the stored cell is still `None`,
i.e. the `Init -> Buffered` transition) so subsequent yields skip the
mutex lock entirely.

**TS** (`crates/shamir-client-ts/src/core/cursor-iterator.ts`):
1. The recursive `this.next()` call on an empty-page retry (~line 137,
   `if (value === undefined) { return this.next(); }`) risks unbounded
   recursion / stack growth on a pathological "every page comes back
   empty but `has_more` stays true" server bug (should never happen
   given CR-B4's `has_more` fix landed server-side, but a client
   shouldn't rely on server correctness for its own stack safety).
   Replace the recursion with a `while` loop inside `next()` that keeps
   fetching pages until it gets a non-empty buffer or `done`.
2. `this.buffer.shift()` (~line 132) is O(n) per call (array
   re-indexing) — replace with an index-based read: keep a `private
   position = 0` field, read via `this.buffer[this.position++]`, and
   reset `position = 0` whenever a fresh page's buffer is installed.
   Check whether `buffer.length` checks elsewhere in the file
   (~line 118) need to become `this.buffer.length - this.position`
   instead of `this.buffer.length` once you introduce the index.

## P-6 — `fetch_next` re-resolves db→repo→table + interner every page

`fetch_next` (`cursor_handlers.rs`) calls `resolve_repo` (~line 991) and
rebuilds the `FilterContext`/interner (~line 1034) on EVERY page. This is
a real per-page cost, but caching a `TableManager` handle directly on
`Cursor`/`CursorState` has a correctness question attached: does
re-resolution let a cursor legitimately OBSERVE a concurrent `DropTable`
(and fail cleanly, "table went away mid-scroll") that a cached handle
would instead paper over (continuing to serve reads against a
conceptually-dropped table)? **Investigate this before choosing** — check
whether `resolve_repo`/`get_table` failing mid-cursor-lifetime today
produces a sensible error to the client, and whether a cached handle
would change that behavior. Two acceptable outcomes for this item:
(a) cache the handle on `Cursor` if you're confident it doesn't regress
the drop-table-mid-scroll behavior (write a test proving whichever
behavior you land on), or (b) keep re-resolution AS-IS and add a doc
comment on `fetch_next` explicitly stating this per-page cost is
deliberate (observes concurrent schema/table lifecycle changes) rather
than an oversight. Do NOT silently do nothing without a doc comment
either way — this item's whole point is making the tradeoff visible,
not necessarily changing the code.

## B-5 — `resolve_repo`'s "not found" errors misclassified as `unknown_db`

`resolve_repo` (`cursor_handlers.rs`, ~lines 116-132) returns
`BatchError::QueryError { alias: String::new(), message: "Repository
'{}' not found", code: None }` for BOTH the db-not-found AND
repo-not-found cases. `error_code()`'s legacy heuristic
(`crates/shamir-server/src/db_handler/handler.rs`, ~lines 711-728) then
classifies ANY `QueryError` with an empty `alias` and a message containing
"not found" as `"unknown_db"` — so a REPO-not-found error incorrectly
reports the wire code `unknown_db`, which is misleading (the database
exists; the repo inside it doesn't). Fix: give these two cases distinct
structured codes by setting `code: Some(...)` explicitly rather than
relying on the message-sniffing fallback — e.g. a `code: Some("unknown_db")`
for the db-not-found case (preserves today's actual code, just makes it
EXPLICIT instead of accidental) and a NEW distinct code (e.g.
`"unknown_repo"`) for the repo-not-found case. Check whether `QueryError.code`
is a plain `Option<String>` or a more structured type before picking the
exact string, and check whether `"unknown_repo"` collides with any
existing wire error code elsewhere in the codebase (grep for it) before
introducing it.

## Tests (TDD — write failing tests first, per item)

- R-9: config validation rejects the invalid value(s) you chose to reject
  (or a test asserting the documented zero-semantics, if you chose the
  "document, don't reject" path).
- R-10: a test proving two overlapping `.next()` calls result in exactly
  ONE `create_cursor` request (not two); a test proving `.return()` with
  an unexpected response kind does not throw (logs/swallows instead).
- R-11: a test proving a mid-pagination error triggers a best-effort
  `CancelCursor` (or, if you took the doc-only fallback, no test needed —
  say so).
- P-3: the restructured `acquire()` must not regress ANY existing
  `byte_budget_tests.rs`/`byte_budget_exhaustion_tests.rs`/
  `byte_budget_upfront_reserve_tests.rs` test; add the new
  oversized-acquire-admitted-via-current==0 test if none exists today.
- P-4: only if you found a genuine redundant clone — a test isn't really
  meaningful for a pure clone-count reduction; rely on the existing
  regression suite staying green plus your own diff review.
- P-5 (Rust): only if the unconditional-write bug is real — same "existing
  tests stay green" bar, this is an internal optimization with no
  observable behavior change.
- P-5 (TS): a test with an artificially-crafted repeated-empty-page mock
  response sequence proving the iterator doesn't stack-overflow /
  correctly proceeds past multiple empty pages via the loop; a test
  proving buffer reads still return records in order after the
  shift-to-index change.
- P-6: whichever outcome you choose, a test proving that specific
  behavior (either "cursor still observes a concurrent drop_table" if you
  keep re-resolution, or "cursor keeps working correctly after caching,
  behavior X for a concurrent drop_table" if you cache).
- B-5: a test asserting a repo-not-found `CreateCursor`/`FetchNext`
  produces the NEW distinct code, and a db-not-found case still produces
  `unknown_db` (regression).

## Gate

```
cargo fmt -p shamir-server -p shamir-client -- --check
cargo clippy -p shamir-server -p shamir-client --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -p shamir-client --full
```

For the TS-touched items (R-10, P-5's TS half), also run (from
`crates/shamir-client-ts/`):
```
npm run typecheck
npm test
```

All must pass before returning. In your final report, for EACH of the
eight items, state explicitly: fixed as described / found already
resolved (skipped, say why) / took the documented-alternative path
instead of the code fix. Do not silently skip an item without saying so.
