# Wave-Review Follow-up — RI-15 + FG-5 (broader pass), 2026-07-23

Fresh, broader release-readiness review of the RI-15 (global in-flight
response-memory budget) + FG-5a..e (server-side cursors / streaming reads)
wave, commits `42dccbeb..bc3c99b8`. Reviewed at worktree HEAD `bc3c99b8`
(master had advanced by 2 commits — see §1).

**Explicitly out of scope** (already known, being fixed in the parallel
Wave-A stream, tasks #760–#766 — NOT re-reviewed here): cursor ACL bypass,
terminal-page resource leak, `page_size=0` infinite loop, keyset pagination
losing rows on duplicate ORDER BY values, cursor responses bypassing the
byte budget, bootstrap-token issue.

Severity legend: **HIGH** = fix before public release; **MED** = should fix
before release or document loudly; **LOW** = polish / follow-up task.

---

## Summary table

| ID | Sev | Area | Finding |
|----|-----|------|---------|
| R-1 | HIGH | cursors / correctness | Cursor "stable snapshot" guarantee is broken by a concurrent DELETE — the pinned-version read enumerates the *current* id set, so a row deleted mid-scroll silently vanishes (and shifts offset-bookmark pages) |
| R-2 | HIGH | RI-15 / resource exhaustion | Byte budget is acquired AFTER execution + serialization — burst allocation is still unbounded; the budget only bounds write-path residency, not the ~64 GiB scenario the CHANGELOG claims to close |
| R-6 | MED→HIGH | cursors / perf+memory | Every `FetchNext` is a full-table scan with a per-record async MVCC lookup AND materializes the *entire* match set server-side — the feature does not reduce server-side peak memory at all, only wire/client-side |
| R-3 | MED | cursors / leak | `CursorRegistry::by_session` entries are never removed — one leaked `Arc<AtomicUsize>` per session (per connection) that ever opened a cursor, forever |
| R-4 | MED | docs | `KNOWN_LIMITATIONS.md` §6 still says "No server-side cursors yet" and "No global inflight response-memory budget yet" — both stale |
| R-5 | MED | wire spec/impl | `FetchNext.page_size` is mandatory on the wire, so the documented "default for calls that omit an override" is unimplementable; `Cursor::default_page_size` is dead code |
| R-7 | MED | cursors / DoS | Cursor path bypasses `BatchLimits` entirely — no server-side clamp on `page_size` and no `max_result_size_bytes` clamp on a cursor page (distinct from the known byte-budget bypass) |
| P-2 | MED | RI-15 / perf | Response is serialized TWICE when the budget is bounded — once just to measure its length, then again for the wire (extra full-size alloc + encode per response) |
| P-3 | LOW/MED | RI-15 / perf | `ByteBudget::acquire` registers a `Notify` waiter (mutex-guarded list insert+remove) on every call, even when the CAS succeeds immediately — the "lock-free fast path" comment is inaccurate |
| R-9 | LOW | config | No validation for `security.cursors.*` (`idle_timeout_secs=0` / `max_cursors_per_session=0` accepted silently); cursors config absent from the operations guide |
| R-10 | LOW | TS SDK | Concurrent `next()` calls unguarded — can double-issue `create_cursor` and leak a server cursor; `return()` throwing during IteratorClose can mask the loop's original exception |
| R-11 | LOW | Rust SDK | An error-terminated `CursorStream` leaves the server cursor open (60 s snapshot pin) with no doc pointing at `close()` |
| B-1 | LOW | conventions | `CursorRegistry::open`/`by_session` use default `RandomState` while `reaped_tombstones` in the SAME struct uses `THasher` — violates CLAUDE.md pillar 4 |
| B-2 | MED | test coverage | No test at all for the no-ORDER-BY offset-bookmark path; no delete-mid-scroll, page_size-change, multi-column-ORDER-BY-fallback, concurrent-FetchNext, or oversized-acquire tests |
| B-3 | LOW | wire contract | `has_more = len >= page_size` makes `has_more: true` a documented lie on exact-multiple result sets (doc promises "at least one more record") |
| P-4/P-5/P-6 | LOW | perf polish | Avoidable per-page query clones, per-record mutex writes in `CursorStream`, per-page repo/table/interner re-resolution |

---

## 1. Did we do the Wave A remediation correctly?

**Mostly: in progress — could not evaluate.** At review time master carried
exactly one Wave-A commit past this worktree's HEAD:

- `b860765c` `fix(server): CR-A1 -- enforce ACL/DAC on the cursor
  create/fetch path` (#760). Reviewed at the design level only (commit +
  message): the shape is right — authorization happens BEFORE the MVCC
  snapshot is opened, `fetch_next` re-authorizes on every call (closes the
  permission-revoked-mid-scroll window), a denial removes the cursor, and
  `cancel_cursor` correctly relies on the session-ownership check. Includes
  negative e2e + positive control + revoked-mid-scroll tests. No red flags
  from this altitude; a line-level verification should happen when the full
  Wave-A series is complete.

Tasks #761–#766 had not landed. One coupling note for the in-flight work:
after the terminal-page-leak fix (create no longer registering an
already-exhausted cursor), the `state.exhausted` early-return branch in
`fetch_next` (`crates/shamir-server/src/db_handler/cursor_handlers.rs:327-337`)
becomes dead code — it is only reachable today *because* create registers
exhausted cursors. Whoever lands that fix should delete the branch or keep
it with a comment explaining what still reaches it.

---

## 2. What else needs to happen before a public release?

### R-1 (HIGH) — Concurrent DELETE breaks the cursor's snapshot-stability guarantee

The cursor's headline claim — "every `FetchNext` reads a stable,
snapshot-consistent view for the cursor's whole lifetime"
(`crates/shamir-server/src/cursor_registry.rs:20-24`, repeated in the
CHANGELOG) — does not hold against a concurrent DELETE:

- `read_as_of` enumerates the **current** id set
  (`self.list_stream(FULL_SCAN_BATCH)`,
  `crates/shamir-engine/src/table/read_temporal.rs:85`), which for an MVCC
  table streams `MvccStore::current_stream` — the per-key winner at the
  floor captured **at stream-open time** (i.e. "now", not the pinned
  version). Only *after* enumeration does it read each id at the pinned
  version via `get_at` (`read_temporal.rs:98`).
- A delete removes the record from the current view:
  `delete_versioned` writes a tombstone as the new current winner and
  `self.table.delete(id)` removes the main-table entry
  (`crates/shamir-engine/src/table/table_manager_crud.rs:334-343`;
  tombstones are dropped from current streams —
  `crates/shamir-storage/src/storage_membuffer.rs:729`).

Consequence: a row that existed at the pinned version but is deleted
between two `FetchNext` calls is **never enumerated again** → it silently
disappears from the cursor's remaining pages. On the no-ORDER-BY
offset-bookmark path the vanished row *additionally* shifts every
subsequent offset by one, dropping an unrelated second row.

The snapshot-stability test only exercises a concurrent **INSERT**
(`crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs:243-336`).
There is no delete-mid-scroll or update-mid-scroll test.

This is technically a pre-existing property of the `read_as_of` strategy
(its own doc comment at `read_temporal.rs:29-34` says "enumerate every
record id" without noting the current-vs-pinned enumeration mismatch), but
FG-5 is what elevates it into a user-facing guarantee. Action, in order of
preference: (a) fix `read_as_of` enumeration to include keys whose current
winner is a tombstone but whose `get_at(pinned)` value exists (the history
store still has them — GC can't collect above the pinned snapshot's
`min_alive`); or (b) at minimum add the failing test + a prominent entry in
`KNOWN_LIMITATIONS.md` and `CURSORS.md` before release.

### R-2 (HIGH) — RI-15 gates residency, not allocation; the claimed OOM gap is only partially closed

`ShamirDbHandler::execute` acquires the budget **after** the batch has
fully executed and the response has been fully serialized
(`crates/shamir-server/src/db_handler/handler.rs:585-590`). Therefore:

- N concurrent dispatch tasks can ALL materialize + serialize max-size
  responses before any of them reaches `acquire` — the 1000 × 64 MiB burst
  the commit message / CHANGELOG describe as "closed" still allocates in
  full before the gate can bite.
- Worse, a task that blocks in `acquire` **holds** its serialized response
  (plus the un-serialized `BatchResponse`) in memory while waiting, so under
  saturation the gate *increases* the duration of peak memory occupancy.

What the budget genuinely bounds is write-path residency — the classic
slow-client amplification where responses pile up in the writer queue
(`WriterMsg` guards dropped post-write,
`crates/shamir-server/src/connection/request_loop.rs:186-216`). That is
valuable, but it is a materially weaker guarantee than advertised.

Action: acquire an upfront reservation *before* `db.execute_as` (the
clamped `batch.limits.max_result_size` is the natural estimate), then
shrink it to the actual serialized size afterwards (release the delta).
That closes the burst window at the cost of the documented
cap-to-actual under-utilization — which can itself be reclaimed by the
post-hoc shrink. If the current semantics are kept instead, the CHANGELOG
`[Unreleased]` RI-15 bullet and `byte_budget.rs` module docs (lines 10-19)
must be reworded to say "bounds in-flight bytes on the write path", not
"closes the ~64 GiB gap".

### R-6 (MED→HIGH) — Every `FetchNext` is O(full table) and materializes the whole match set server-side

The bookmark model's *re-execution* is documented; what is not documented
is how expensive the re-executed read actually is on the `AsOf` pipeline
every cursor uses:

- Full scan of every current record id, with an **individually awaited**
  `mvcc.get_at` per record (`read_temporal.rs:91-123`) — no index is ever
  consulted on the AsOf path, so the keyset boundary filter saves nothing
  server-side.
- The ENTIRE match set is materialized as `Vec<(RecordId, Bytes)>`
  (`read_temporal.rs:88`), then (when ORDER BY is present, i.e. the normal
  cursor case) the full set is projected and sorted before one page is
  sliced off (`read_temporal.rs:150-172`).

So paging an N-row result at page size p costs O(N²/p) total work with
N async MVCC lookups *per page*, and server-side peak memory per
`FetchNext` ≈ the full result set — meaning FG-5 does **not** deliver the
"results no longer materialize as one big Vec" property on the server at
all (only on the wire and in the SDKs). The CHANGELOG bullet ("closing the
'QueryResult materializes as a Vec' limitation") oversells this.

Action: (a) document the cost model in `CURSORS.md` + `KNOWN_LIMITATIONS.md`
(replace the stale "no cursors yet" bullet, see R-4, with an honest "cursor
pages re-run an O(table) pinned-version scan"); (b) as a cheap first
optimization, batch the per-record lookups (`get_current_many`-style API
already exists in `shamir-tx`); (c) longer term, a versioned index or a
true snapshot scan.

### R-3 (MED) — `CursorRegistry::by_session` leaks one entry per session forever

`free_session_slot` only decrements the counter — it never removes the map
entry (`crates/shamir-server/src/cursor_registry.rs:366-370`), and nothing
else does. Session ids are per-connection random `[u8; 32]`, so **every
connection that ever opens a cursor** permanently leaks a
`[u8;32] → Arc<AtomicUsize>` entry. Compare `TxRegistry::remove`, which
does clean up via `remove_if` (`crates/shamir-server/src/tx_registry.rs:243-251`).
Slow (tens of bytes per connection) but unbounded on a long-lived server.
Fix: `remove_if` the entry when the count reaches 0 (careful with the
concurrent-register race — a CAS from 0 in `register` vs. removal; the
`entry()` API can arbitrate), or sweep zero-count entries in the reaper.

### R-4 (MED) — Stale KNOWN_LIMITATIONS entries

`docs/guide-docs/KNOWN_LIMITATIONS.md:170-178` still lists "**No
server-side cursors yet**" and "**No global inflight response-memory budget
across concurrent connections yet**". Both features shipped in this wave.
The §6 "results materialize fully into a `Vec`" bullet also needs
qualification per R-6 (server-side materialization per page is unchanged).
This file is the public honest-limitations surface — shipping a release
with it contradicting the CHANGELOG is a credibility bug.

### R-5 (MED) — `FetchNext` default-page-size contract is unimplementable; dead field

`DbRequest::FetchNext { page_size: u32 }` is mandatory
(`crates/shamir-query-types/src/wire/db_message.rs:239-244`), yet the
`CreateCursor` doc (`db_message.rs:220-233`) and `CURSORS.md` (table row
for `page_size`) promise it is "the default for subsequent `FetchNext`
calls that omit an override" — there is no way to omit it. Correspondingly,
`Cursor::default_page_size` is stored and exposed but **never consumed**
(`crates/shamir-server/src/cursor_registry.rs:139,193-195`; `fetch_next`
uses its parameter unconditionally,
`crates/shamir-server/src/db_handler/cursor_handlers.rs:307-312`). Either
make the wire field `Option<u32>` with the stored fallback (which would
also give the in-flight `page_size=0` fix a natural "0/absent → default"
semantic), or delete the field, the accessor, and the doc claim.

### R-7 (MED) — Cursor path bypasses `BatchLimits` (`page_size` unclamped, no per-page byte cap)

`create_cursor`/`fetch_next` never touch `query_limits`: a client may send
`page_size = u32::MAX` and the handler will happily attempt to return the
whole table in one `CursorPage`; nothing applies
`max_result_size_bytes` to a cursor page the way `execute` clamps
`batch.limits.max_result_size`
(`crates/shamir-server/src/db_handler/handler.rs:472-476` — no equivalent
in `cursor_handlers.rs`). This is *adjacent to but distinct from* the known
"cursor responses bypass the byte budget" issue: even after the RI-15
budget is wired into the cursor path, the per-response 64 MiB clamp still
would not apply. Verify the Wave-A byte-budget task's scope; if it only
wires `ByteBudget`, add a `page_size`/page-bytes clamp
(`min(page_size, derived_row_cap)` or a serialized-size check).

### R-9 (LOW) — Cursor config: no validation, no operator docs

`Config::validate` gained the RI-15 cross-check
(`crates/shamir-server/src/config.rs:622-632`) but nothing for
`security.cursors.*`: `idle_timeout_secs = 0` (every cursor evicted on the
next 5 s sweep) and `max_cursors_per_session = 0` (CreateCursor always
fails with `cursor_limit_exceeded`) are accepted silently. Also
`docs/guide-docs/guide/07-operations.md` documents
`max_inflight_response_bytes` (line 163) but not the two new
`security.cursors` keys, and the example `.ktav` profiles don't mention
them. Add `>= 1` validation (or document zero semantics) + an operations
paragraph.

### R-10 / R-11 (LOW) — SDK edge cases

- TS: two overlapping `next()` calls (legal JS, no protocol guard) that
  both observe an empty buffer with `_cursorId === undefined` will issue
  **two** `create_cursor` requests; the loser's cursor id is overwritten by
  `applyPage` and that server cursor leaks until idle-timeout
  (`crates/shamir-client-ts/src/core/cursor-iterator.ts:113-140,147-157`).
  Standard fix: serialize `next()` bodies on an internal promise chain.
  Also `return()` throws on an unexpected `kind`
  (`cursor-iterator.ts:173-177`); a throw out of `return()` during implicit
  IteratorClose (e.g. triggered by a `break` after an error) can mask the
  original exception — prefer swallowing/logging there.
- Rust: after a mid-pagination error the stream collapses to `Exhausted`
  (`crates/shamir-client/src/cursor_stream.rs:151-160`) but the
  server-side cursor stays open with its snapshot pinned until the 60 s
  reaper. `close()` after the error would cancel it, but neither the
  module doc's cleanup section (`cursor_stream.rs:10-30`) nor the error
  path mentions this case. One doc sentence, or best-effort `CancelCursor`
  before yielding the terminal `Err`.

---

## 3. Suboptimal code to speed up or simplify

### P-2 (MED) — RI-15 serializes every response twice

When the budget is bounded, `execute` runs
`rmp_serde::to_vec_named(&response)` **solely to measure its length** and
throws the buffer away (`handler.rs:586-589`); `RequestHandler::handle`
then serializes the same response again for the wire (`handler.rs:443`).
For a 64 MiB response that is an extra 64 MiB allocation plus a full
msgpack encode pass per response, on exactly the path the feature is
supposed to protect. Fix options, cheapest first: (a) a counting
`io::Write` sink (`rmp_serde::encode::write_named` into a
`struct CountingWriter(usize)`) — zero allocation, same byte count; (b)
serialize once in `execute` and stash the bytes alongside the guard so
`handle` reuses them (bigger refactor across the trait boundary).

### P-3 (LOW/MED) — `ByteBudget::acquire` is not lock-free on the fast path

Every `acquire` creates and `enable()`s a `Notified` future *before* the
first CAS attempt (`crates/shamir-server/src/byte_budget.rs:124-130`).
`enable()` inserts the waiter into `Notify`'s mutex-guarded list and
dropping the future removes it — i.e. two mutex acquisitions per
successful fast-path acquire, plus `notify_waiters()` (another lock) on
every guard drop (`byte_budget.rs:203`). The comment "Fast path: CAS loop,
no lock, no waiter registration" (`byte_budget.rs:132`) is therefore
inaccurate as written. Restructure: run the CAS loop once *first*; only on
failure create/enable the `Notified` future, re-check, then park. This
makes the common uncontended case genuinely lock-free and leaves the
documented race-free pattern intact for the contended case.

Also untested/undocumented corner: the oversized-request admission branch
(`current == 0` special case, `byte_budget.rs:138-140`) has no unit test,
and its liveness depends on the budget *fully* draining — under continuous
small traffic an oversized waiter can starve indefinitely. Config
validation makes this unreachable today (responses ≤ `max_result_size_bytes`
≤ cap), but the primitive's docs should state the starvation caveat since
it is a public-ish building block.

### Cursor bookmark model — avoidable overhead beyond the documented re-execution

- **The dominant cost is R-6 above** (per-page full scan + per-record
  awaited `get_at` + full-set materialize/sort). Everything below is minor
  by comparison.
- P-4: `fetch_next` clones the full `ReadQuery` twice per page
  (`cursor_handlers.rs:352-353` — `base_query` then `next_query`);
  `boundary_filter` and `seek_value_from_last_record` take references, so
  one clone suffices.
- P-6: `fetch_next` re-resolves db → repo → table and re-fetches the
  interner on every page (`cursor_handlers.rs:321-350`). Caching the
  `TableManager` handle in the `Cursor` would shave this; re-resolution
  does have the virtue of observing a concurrent `drop_table`, so if kept,
  say so in a comment.

### P-5 (LOW) — TS/Rust wrapper micro-nits

- Rust `CursorStream`: the `unfold` wrapper locks the `StdMutex` and
  rewrites the same cursor id after **every yielded record**
  (`cursor_stream.rs:205-215`); it only changes on the Init→Buffered
  transition — guard with `if guard.is_none()` or write only on
  transition.
- TS `CursorIterator.next()`'s empty-page handling recurses
  (`return this.next()`, `cursor-iterator.ts:132-139`) — safe in async
  (microtask trampolining) but a `while` loop reads clearer and avoids the
  promise-chain growth on pathological many-empty-page servers.

---

## 4. Broader improvements (conventions, test quality, tech debt)

### B-1 — Hasher convention violation, internally inconsistent

`CursorRegistry` uses default-`RandomState` DashMaps for `open` and
`by_session` while `reaped_tombstones` in the same struct uses `THasher`
(`cursor_registry.rs:249-253`, via `#[derive(Default)]`). CLAUDE.md pillar
4 mandates `DashMap::with_hasher(THasher::default())` everywhere.
`TxRegistry` has the same pre-existing gap (`tx_registry.rs:185-188`) —
one small `chore` commit can fix both registries.

### B-2 — Test coverage gaps (the tests that exist are genuinely rigorous)

Credit where due: the new tests are behavioral (through the real handler /
real server), the snapshot test performs a *real* concurrent write with a
sanity check that it landed, the budget tests reproduce the exact
production task-local scoping, and both SDKs got true e2e suites. Gaps:

1. **The no-ORDER-BY offset-bookmark path has zero coverage** — every
   handler test uses `OrderBy::asc("v")`
   (`cursor_handler_tests.rs:139,202,251`), yet this path is also the
   silent fallback for multi-column ORDER BY, nested field paths, an
   unprojected seek field, and unconvertible seek values
   (`cursor_handlers.rs:129-165,368-379`). Its central assumption
   (deterministic enumeration order at a pinned version) is exactly what
   R-1 undermines.
2. No delete-mid-scroll / update-mid-scroll snapshot test (R-1).
3. No test that `FetchNext` with a different `page_size` than
   `CreateCursor` behaves (a documented capability).
4. No concurrent `FetchNext` on the same cursor (the state-mutex
   serialization and double-advance semantics are untested).
5. `ByteBudget`: the oversized (`bytes > cap`) admission branch and the
   zero-byte acquire are untested (`byte_budget.rs:138-140`).

### B-3 — `has_more` heuristic violates its own doc on exact multiples

`has_more = page.records.len() >= page_size`
(`cursor_handlers.rs:257,393`) means a result set that is an exact multiple
of the page size reports `has_more: true` on its true last page, and the
wire doc explicitly promises "a further `FetchNext` will return at least
one more record" (`db_message.rs:387-390`). Cost today: one spurious
empty-page round-trip and a technically false contract. Either soften the
doc ("may return zero records") or peek ahead (`limit = page_size + 1`,
return `page_size`, `has_more = got == page_size + 1`) — the peek also
composes correctly with the terminal-page-leak fix in flight.

### B-4 — Overclaiming in durable prose

Two places where the committed prose promises more than the code delivers,
which a public release turns into external commitments: the RI-15
CHANGELOG bullet ("closing the gap … worst case ~64 GiB", see R-2) and the
FG-5 CHANGELOG bullet ("closing the 'QueryResult materializes as a Vec'
limitation", see R-6). Both should be reworded in the same PR that fixes
or documents the underlying behavior. Similarly, `CURSORS.md` still
carries FG-5a-era "enforcement lands in FG-5b" notes (e.g. around line
188) that should be refreshed now that FG-5b has landed.

### B-5 — Error-code mapping nit

`resolve_repo`'s "Repository 'x' not found" flows through the legacy
heuristic in `error_code` (`handler.rs:655-660`) and comes out as
`unknown_db` (empty alias + "not found") — misleading for a repo-level
miss on the cursor path. Consider a structured `code` on those two
`QueryError`s (`cursor_handlers.rs:69-80`).

### Positive observations (no action)

- The tombstone mechanism distinguishing `cursor_expired` from
  `cursor_not_found`, with its own bounded TTL sweep, is a thoughtful UX
  touch (`cursor_registry.rs:240-260,382-388`).
- The per-session cap CAS loop closes the TOCTOU window correctly
  (`cursor_registry.rs:291-306`).
- The task-local guard hand-off (`run_with_guard_slot` /
  `stash_guard` / `take_stashed_guard`) is well-documented and its
  out-of-scope no-op fallback fails safe (`byte_budget.rs:208-265`).
- The deliberate, documented Rust-vs-TS cleanup asymmetry (TS `return()`
  cancels deterministically; Rust cannot in `Drop`) is the right call and
  honestly explained on both sides.

---

## Prioritized action list

1. **R-1** — add the delete-mid-scroll failing test; fix `read_as_of`
   enumeration (include tombstoned-now / alive-at-pin keys) or document
   the guarantee's limits loudly. *(Release blocker.)*
2. **R-2** — move the budget acquisition ahead of execution (estimate →
   shrink) or reword every claim; coordinate with the in-flight cursor
   byte-budget task. *(Release blocker in its current oversold form.)*
3. **R-7** — clamp `page_size` / per-page bytes on the cursor path (check
   Wave-A #762 scope first). *(Release blocker if not already covered.)*
4. **R-4 + B-4** — refresh `KNOWN_LIMITATIONS.md` §6, the two CHANGELOG
   bullets, and `CURSORS.md` status notes; add the R-6 cost model to the
   docs. *(Must ship with the release.)*
5. **R-3** — fix the `by_session` entry leak (mirror `TxRegistry::remove`).
6. **R-5** — resolve the `FetchNext` default-page-size contract (Option +
   fallback, or delete the dead field) — best done together with the
   `page_size=0` Wave-A fix.
7. **P-2** — replace the measurement serialization with a counting writer.
8. **B-2** — close the test gaps (no-ORDER-BY path first; then oversized
   budget acquire, page-size change, concurrent FetchNext).
9. **P-3 + B-1 + B-3 + R-9 + R-10 + R-11 + P-4/P-5/P-6 + B-5** — batch as
   one or two small `chore`/`fix` follow-up tasks post-release-candidate.
