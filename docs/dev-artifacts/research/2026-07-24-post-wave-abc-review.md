# Post-Wave-A/B/C Release-Readiness Review — 2026-07-24 (@fh)

Broad review of the full three-wave remediation campaign (`bc3c99b8..3e85e976`,
46 commits, tasks #760–#780) over FG-5 (cursors), RI-15 (byte budget),
backup/restore, numeric comparison, TS SDK, and release CI. Everything the
2026-07-23 reviews flagged and Wave A/B/C fixed was re-verified at line level
and is NOT re-reported here (see §4 for the verified-clean list). This
document reports what is **still** wrong, plus the requested confirmation of
the `Int`↔`F64` precision gap.

Severity legend: **HIGH** = fix before public release; **MED** = fix before
release or document loudly; **LOW** = polish / follow-up.

---

## Summary table

| ID | Sev | Area | Finding |
|----|-----|------|---------|
| N-1 | HIGH | cursors / livelock | Keyset cursor livelocks (infinite empty pages, `has_more: true` forever) once a single ORDER BY value's tie run reaches `max_cursor_page_size` — both SDK loops spin forever, rows beyond the run are unreachable |
| N-2 | HIGH→MED | cursors / data loss | Keyset cursor silently drops every row whose ORDER BY value is `Null`, missing, `NaN`, or a different type than the seek key — no error, `has_more: false`, rows just vanish |
| N-3 | MED | numbers / correctness | **CONFIRMED**: plain `Int`↔`F64` comparison collapses through `f64` for `\|i\| ≥ 2^53` — real, reachable (ns timestamps, snowflake ids), currently "tracked-elsewhere" but **no tracking task actually exists** and `KNOWN_LIMITATIONS.md` §7 does not mention it |
| N-4 | MED | RI-15 / perf | Cursor pages are still serialized twice (CR-B2's single-serialization fix covers only the `Execute` path); cursor budget accounting measures the inner `QueryResult`, not the wire envelope |
| N-5 | MED | RI-15 / ops | Upfront-reserve turns the byte budget into a hard concurrency limiter: every batch reserves the full 64 MiB default regardless of actual response size → `cap / 64 MiB` concurrent batches server-wide; consequence not documented for operators |
| N-6 | LOW/MED | restore | `SwapPartialFailure` returns the same misleading "operator must manually rename" message even when the automatic rollback SUCCEEDED; staged `restore_tmp` copy is never cleaned up on failure |
| N-7 | LOW | cursors | `create_cursor` treats a failed `drain_all` as a log-line warning yet serves the cursor's entire lifetime from the possibly-incoherent snapshot |
| N-8 | LOW | docs / contract | `has_more` doc ("will return at least one more record", `db_message.rs:392-394`, `CURSORS.md:174`) is falsified by N-1; keyset limitations (N-1/N-2) absent from `CURSORS.md`/`KNOWN_LIMITATIONS.md` |
| N-9 | LOW | hygiene | Stale FG-5a-era comment in `handler.rs:731-733`; `fetch_next` authorizes AFTER repo-resolution (create does the reverse); TS `return()` not serialized behind the `pending` chain (manual-driving edge) |

---

## 1. N-1 (HIGH) — Keyset tie-run ≥ `max_cursor_page_size` livelocks the cursor

`fetch_keyset_page` (`crates/shamir-server/src/db_handler/cursor_handlers.rs:515-651`)
bounds its internal fetch at `limit_ceiling = max_cursor_page_size` (default
10,000, `crates/shamir-server/src/config.rs:331-333`), but `tie_skip` — the
count of already-returned rows tied at the boundary — grows WITHOUT bound
across pages (`bookmark_from_tail`, `cursor_handlers.rs:471-494`, counts the
whole consumed prefix). Walk the arithmetic:

- Paging through a tie run with page size `p`, `tie_skip` grows by `p` per
  page: `p, 2p, 3p, …`.
- Once `tie_skip` reaches the ceiling `C`, the next call computes
  `internal_limit = max(p, tie_skip+1).min(C) = C`
  (`cursor_handlers.rs:539-542`), fetches `C+1` rows (all tied), the
  skip-walk consumes exactly `C` (`cursor_handlers.rs:595-611`), so
  `usable_len = min(C+1, C) − C = 0` (`cursor_handlers.rs:616`),
  `data_exhausted = (C+1 ≤ C) = false`, and the ceiling branch
  (`cursor_handlers.rs:637-647`) returns **`take = 0` rows with
  `has_more = consumed(C) < fetched(C+1) = true`** and an UNCHANGED
  bookmark (`next_tie_skip` recounts to `C` again).
- Every subsequent `FetchNext` is byte-identical: **empty page,
  `has_more: true`, forever.**

Both SDKs loop on "empty page + has_more" by design (TS:
`doNext`'s `for(;;)` in
`crates/shamir-client-ts/src/core/cursor-iterator.ts:161-184`, added by CR-C1
precisely to survive empty pages; Rust `CursorStream` similarly refetches) —
so a `for await` / `StreamExt` consumer **hangs forever while hammering the
server with an O(C) scan per round-trip**. Rows in the tie run beyond the
first `C` are unreachable through the cursor at all.

Trigger is realistic, not adversarial: `ORDER BY status` /
`ORDER BY category` on any table where one value has > 10,000 rows. It can
even fire on the SECOND call of a cursor (`page_size = 10,000`, first page
all tied → `tie_skip = C` immediately).

The existing test `keyset_tie_run_larger_than_one_page_uses_bounded_retry`
(`cursor_handler_tests.rs:1354`) uses a 10-row tie run under
`CursorLimitsCap::UNLIMITED`, so the ceiling interaction is completely
untested.

**Recommendation: fix now (release blocker for the cursor feature).**
Cleanest fix: when the ceiling branch would return `usable_len == 0` with
`has_more == true`, fall back to the row-count `offset` bookmark for the
remainder of the scroll — `CursorState.offset` is already maintained in
parallel on the keyset branch (`cursor_handlers.rs:1117`) exactly as
"rows returned so far in global sorted order", and post-CR-B1 the pinned-
version enumeration + stable sort make that offset sound (this is the same
fallback the `Keyset`-mode-with-`seek_key == None` path already takes). A
typed error (`cursor_tie_run_too_large`) is the acceptable minimum if the
fallback is judged too risky. Either way add a test: tie run > a small
configured `max_cursor_page_size` (e.g. cap 8, run of 20) and assert the
drain terminates with every row exactly once (fallback) or a clean error.

---

## 2. N-2 (HIGH→MED) — Keyset cursor silently loses `Null`/missing/`NaN`/mixed-type ORDER BY rows

The keyset bookmark's boundary filter is `field >= seek_key` (ASC)
(`boundary_filter`, `cursor_handlers.rs:357-383`) evaluated through
`compare_values`, whose cross-type / null semantics make the filter FALSE for
any row whose ORDER BY value cannot be ordered against the seek key
(`crates/shamir-engine/src/query/filter/resolve.rs:212` — `_ => None`; per the
`ValueCompare` 3-way contract at `resolve.rs:130-138`, an unresolvable
comparison satisfies only `Ne`). Meanwhile the ORDER BY sort places exactly
those rows where later pages should find them:

- **`Null` / missing field** → `QvSortKey::Null`, sorted LAST under the ASC
  default (`crates/shamir-engine/src/query/read/order.rs:296-310`). Page 1
  (no boundary) returns only the leading real-valued rows; every subsequent
  page's `field >= seek` excludes all null/missing rows → the scan "runs out"
  and reports `has_more: false`. **Every null/missing-value row is silently
  dropped.** (DESC puts them first, which happens to work.)
- **Mixed-type column** (e.g. some rows `Int`, some `Str`): the sort treats
  cross-type keys as Equal (`order.rs:364`, `_ => Equal` — insertion-order
  interleave), but after page 1 the boundary `field >= Int(x)` returns `None`
  for every `Str` row → all rows of the other type(s) are silently dropped.
- **`NaN` in an `F64` ORDER BY column**: `Gte(NaN)`/any comparison with NaN
  is `None`/false → same loss; NaN additionally breaks
  `same_boundary_value`'s tie counting (f64 `PartialEq`).

No test covers any of these (every keyset test uses a uniformly-typed,
non-null `i64` column). Unlike the CR-A4/CR-B1 bugs, the client gets **no
error and a clean `has_more: false`** — the worst failure shape this
feature family has.

**Recommendation: fix-or-document before release.** A full fix (two-phase
scan: keyset over comparable values, then an offset-bookmarked tail phase
for null/incomparable rows) deserves its own task. The minimum for release:
(a) at `create_cursor` time, when the query has an ORDER BY, keep Keyset mode
only if you can also express the null tail — otherwise honestly document in
`CURSORS.md` + `KNOWN_LIMITATIONS.md` §6 that keyset cursors return only
rows whose ORDER BY value is non-null and order-comparable with the first
page's boundary value, and (b) add the failing tests so the limitation is at
least pinned. Note `pagination_mode_for_query` cannot detect this statically
(it depends on data, not query shape), which is why documentation +
tail-phase are the honest options.

---

## 3. N-3 (MED) — `Int`↔`F64` f64-collapse: CONFIRMED; needs a real task + a KNOWN_LIMITATIONS entry

**Confirmed exactly as described.** Both sites carry the CR-C5 re-verification
doc comment and the lossy cast:

- `crates/shamir-engine/src/query/filter/resolve.rs:167-168` —
  `(Value::Int(a), Value::F64(b)) => (*a as f64).partial_cmp(b)` (and mirror).
- `crates/shamir-engine/src/query/read/order.rs:323-328` — same cast in
  `compare_qv_sort_keys`.

`(i64::MAX) as f64 == (i64::MAX − 1) as f64` holds (ulp of f64 in
`[2^62, 2^63)` is 1024 — up to 1,024 distinct `i64` values collapse to one
`f64` near the top of the range; 256 near 1e18).

**Reachability assessment.** `Value::Int` covers the full `i64` range (only
`u64 > i64::MAX` promotes to `Big`), and values ≥ 2^53 (~9.0e15) arise
routinely: nanosecond epoch timestamps (~1.75e18 today), snowflake-style
ids (~1e18+), 63-bit hashes/counters. The bug fires ONLY on a **cross-type**
`Int`↔`F64` comparison, which requires either (a) a float literal/operand
against an Int column — easy to produce from JSON/JS clients where a large
numeric literal decodes as f64 (note: a JS `number` > 2^53 has usually
already lost precision client-side, so the interesting server-side cases are
float-typed literals like `1.5e18` and F64-valued fields), or (b) an ORDER BY
column mixing `Int` and `F64` values.

**What breaks.** `Eq` can match distinct values (any Ints sharing an f64
bucket vs. an equal-bucket float operand); `Gt/Gte/Lt/Lte` boundaries are
fuzzy by up to ~1,024 at i64::MAX scale / ~256 at 1e18 scale — wrong
inclusion/exclusion for range filters over ns-timestamps or large ids. In
ORDER BY the colliding values compare Equal and fall back to stable
insertion order — not data loss, but a non-total order; in a keyset cursor a
mixed Int/F64 column additionally intersects finding N-2. `Int`↔`Int` and
`Int`↔`Dec` remain exact, so homogeneous integer workloads are unaffected.

**Verdict.** Real bug, narrow trigger, NOT a release blocker — but two gaps
around it are: (1) the code comments say "tracked as a separate follow-up"
and the CHANGELOG says "tracked elsewhere", yet **no task exists in the task
graph** — create one now (the exact fix is compact and does not need BigInt:
handle NaN/±inf, then compare `i64` against the f64's integer part with
range checks and a fract tiebreak — a well-known exact i64/f64 comparison;
materially smaller than CR-C5's own cross-multiplication work); (2)
`KNOWN_LIMITATIONS.md` §7 ("Numbers") documents only the `u64 → Big`
contract and says nothing about this — the "single citation-backed list"
currently under-discloses a known precision bug that the CHANGELOG admits.
Add the §7 bullet in the same commit that creates the task.

---

## 4. Verified clean (no action) — what this review checked and confirms

- **CR-B1 / MVCC snapshot stability** (`crates/shamir-tx/src/mvcc_store/mod.rs:1283-1379`,
  `crates/shamir-engine/src/table/read_temporal.rs:78-142`,
  `table_manager_streaming.rs:81-110`): the tombstone-inclusive enumeration
  is correct and correctly scoped (only `read_as_of` uses it; every other
  `current_stream` caller unchanged). Re-derived the offset-bookmark
  stability argument post-fix: enumeration is key-ordered, concurrently
  inserted keys resolve to `None` at the pinned version (excluded without
  shifting earlier offsets), deleted keys stay enumerated via tombstone, and
  updated keys route through the `cur_v > snapshot` fallback to the pinned
  value — the "stable at a pinned version" premise now genuinely holds. The
  delete-mid-scroll and update-mid-scroll tests exist
  (`cursor_handler_tests.rs:1940,2038`).
- **CR-C3 `get_at_many`** (`mvcc_store/mod.rs:1088-1160`): the
  classify/partition logic is sound including the classification-vs-write
  race (a write landing after classification gets a version above the pinned
  snapshot, so the captured `cur_v` remains the newest ≤ snapshot);
  output-order reassembly correct; empty-input early return correct.
- **CR-B2 byte budget** (`crates/shamir-server/src/byte_budget.rs`,
  `handler.rs:563-660`): the CAS fast path is now genuinely lock-free
  (`try_cas` before any `Notify` op); the `enable()`-before-recheck pattern
  preserves the no-lost-wakeup invariant; `shrink_to`/`grow_unchecked` delta
  math cannot double-release; the `current == 0` oversized escape hatch is
  now unit-tested (`tests/byte_budget_tests.rs:288`); the two task-locals
  are scoped together per dispatched request and the stashed-bytes reuse in
  `handle` (`handler.rs:451-453`) serves the exact same `DbResponse` value
  serialized in `execute` (nothing mutates it in between). Guard hand-off to
  the writer task verified (`connection/request_loop.rs:332-395`), including
  cursor ops.
- **CR-B8 registry chores** (`cursor_registry.rs:443-448`): the
  `remove_if`-based decrement-and-maybe-remove is atomic vs. a concurrent
  `register` exactly as its comment argues (same-shard write lock); both
  registries now on `THasher`. The `state.exhausted` early-return in
  `fetch_next` (`cursor_handlers.rs:1035-1045`) is NOT dead code — it is
  reachable via a concurrent-FetchNext race (second caller holds the `Arc`
  from before the first caller's exhaustion-removal), and the concurrent
  test exists (`cursor_handler_tests.rs:1724`).
- **CR-B7/C4 backup+restore**: invalidation genuinely runs inside the staged
  copy before the swap (`restore.rs:143-156`); manifest hardening covers
  absolute/`..` paths, duplicates, unknown `format_version`, unmanifested
  extras (`backup.rs:363-445`); dest-inside-source uses
  nearest-existing-ancestor canonicalization (`backup.rs:235-289`);
  streaming SHA-256 is byte-identical with bounded memory (`backup.rs:55-69`).
- **CR-B3/B4/B5 wire semantics**: `FetchNext.page_size` is
  `#[serde(default)] Option<u32>` (`db_message.rs:239-248`) so an old client
  emitting the field stays compatible; the stored default is validated again
  after lookup (defense-in-depth, `cursor_handlers.rs:1021-1027`); the
  peek-ahead trims the extra row BEFORE budget measurement and advances the
  offset by the returned count only (`cursor_handlers.rs:826-832,1149-1162`);
  `with_version` rejection wired end-to-end with a regression test that a
  plain read still returns versions (`cursor_handler_tests.rs:666`).
- **CR-B6 TS CAS types**: all four surfaces are `number | bigint`
  (`types/batch.ts:234`, `types/write.ts:181,208`, `types/ddl.ts:147`) with
  the `Number.isSafeInteger` throw in the builders (`builders/write.ts:53`);
  `CursorId` precedent matched.
- **CR-C1 TS iterator**: overlapping `next()` calls serialized on the
  `pending` chain; empty-page handling is a loop, not recursion;
  `return()` swallows cancel failures to avoid masking the loop body's
  exception (`cursor-iterator.ts:102-144,225-249`).
- **CR-C5 aggregate lane** (`crates/shamir-engine/src/query/read/aggregate.rs`):
  `checked_add` → `BigInt` promotion, one-way float door with exact-total
  fold-in at the transition, `Dec::trunc().mantissa()` exactness for
  integer-valued decimals — all correct as implemented.
- **CR-B9 release workflow**: integration matrix, TS unit, TS e2e against a
  release binary, tag↔version + CHANGELOG checks all present and wired into
  downstream `needs:` (`.github/workflows/release.yml`).
- **Docs**: `KNOWN_LIMITATIONS.md` §6, `CURSORS.md`, the operations guide
  (`07-operations.md:163,197-210`) and the CHANGELOG `[Unreleased]` bullets
  were checked against the code line-by-line for the cursor cost model,
  budget semantics, page-size optionality, temporal/with_version scope cuts,
  and config keys — accurate, with only the N-3/N-8 gaps listed above.
- **Test suite quality**: the cursor handler suite is now ~60 behavioral
  tests covering the offset path, page-size changes, concurrent FetchNext,
  drop-table mid-scroll, revoked-ACL mid-scroll, exact-multiple peek-ahead,
  and error-code distinctness — the B-2 gaps from the prior review are
  closed (except the two NEW gaps named in N-1/N-2).

---

## 5. Remaining findings (MED/LOW detail)

### N-4 (MED) — Cursor path missed CR-B2's single-serialization fix

`enforce_page_budget` serializes the `QueryResult` purely to measure it
(`cursor_handlers.rs:274`) and discards the bytes;
`RequestHandler::handle` then serializes the full `DbResponse::CursorPage`
again for the wire (`handler.rs:443-455` — its own comment notes cursor ops
"never stash"). That is an extra full-page encode + allocation per
`CreateCursor`/`FetchNext` on exactly the large-result path cursors exist
for — the same P-2 defect CR-B2 fixed for `Execute`. (Related nit: the
cursor path budgets the inner `QueryResult` while `execute` budgets the full
envelope — a bounded few-dozen-byte inconsistency, worth aligning when
touching this.) Fix: stash the measured bytes wrapped in the envelope (the
envelope adds only `cursor_id`/`has_more`/discriminator — serialize the full
`DbResponse` in `enforce_page_budget` instead of the bare page and reuse via
`stash_serialized_response`). Track as follow-up; land with or after N-1.

### N-5 (MED) — Upfront-reserve = hard concurrency limiter; not in the ops guide

`execute` reserves `batch.limits.max_result_size` upfront
(`handler.rs:563-567`). The client-side default equals the server cap
(64 MiB), so with the ops-guide example `max_inflight_response_bytes: 4 GiB`
(`07-operations.md:163`) the server executes at most **64 concurrent
batches** — even if every response is 1 KiB. That is a deliberate CR-B2
tradeoff (execution-gating requires a pessimistic estimate), but the
operational consequence — "your effective batch concurrency is
`cap / max_result_size_bytes`; size the cap accordingly, and/or have clients
lower `limits.max_result_size` per batch" — appears nowhere in the operator
docs. One paragraph in `07-operations.md` + a sentence on the CHANGELOG
bullet. (Also worth a `verify` pass someday: `grow_unchecked` assumes the
engine's `max_result_size` enforcement keeps the actual response within
"a few framing bytes" of the estimate; if any `BatchResponse` field escapes
that enforcement, the overshoot is admitted past the cap unchecked.)

### N-6 (LOW/MED) — restore.rs error-path polish

`restore.rs:165-184`: both the rollback-SUCCEEDED and rollback-FAILED arms
return the identical `SwapPartialFailure`, whose message states "data_dir
could not be reconstructed automatically — operator must manually rename one
of these two directories to {data_dir}". After a successful rollback that
instruction is wrong (data_dir exists again, holding the pre-restore data) —
an operator following it in a disaster scenario gets a confusing rename
failure at best. Split into two variants ("swap failed, rolled back — your
old data_dir is intact; restored copy left at {temp_dir}" vs. the true
partial-failure). Also: the staged `*.restore_tmp_*` directory (a full copy
of the snapshot) is never deleted on a step-4/step-5 failure
(`restore.rs:138-156`) — document or best-effort-remove; and
`FjallUserDirectory::open(temp_dir.join("users"))` creates an empty `users`
store when the snapshot lacks one (cosmetic materialization).

### N-7 (LOW) — `create_cursor` shrugs off a failed `drain_all`

`cursor_handlers.rs:772-774` logs a warning and proceeds; the comment says
the drain is what makes "the pinned version's AsOf reads coherent for the
cursor's whole lifetime". If that is literally true, a failed drain serves
potentially incomplete pages for up to the cursor's whole lifetime with only
a server log line. In practice the overlay-aware read paths
(`current_stream_impl`'s overlay merge, `get_at_many`'s overlay probes)
appear to cover undrained data anyway — in which case say so in the comment
and demote the drain to an optimization; otherwise propagate the error and
fail the `CreateCursor`. Either resolution is one small change; the current
state is the worst of both (claimed-load-bearing but ignorable).

### N-8 (LOW) — Contract/docs drift created by N-1/N-2/N-3

- `db_message.rs:392-394` + `CURSORS.md:174` promise `has_more: true` ⇒ "a
  further FetchNext will return at least one more record" — falsified by
  N-1's empty-page livelock. Resolves with N-1's fix; otherwise soften.
- Neither `CURSORS.md` nor `KNOWN_LIMITATIONS.md` §6 mentions the keyset
  null/mixed-type row-loss (N-2) or the tie-run ceiling (N-1).
- `KNOWN_LIMITATIONS.md` §7 lacks the `Int`↔`F64` gap (N-3).

### N-9 (LOW) — hygiene batch

- `handler.rs:731-733`: "actual cap/eviction enforcement … lands in FG-5b" —
  FG-5b landed; stale.
- `fetch_next` resolves db→repo (`cursor_handlers.rs:1029`) BEFORE
  re-authorizing (`:1059`), the reverse of `create_cursor`'s order. Impact
  is negligible (the session already owned the cursor), but the asymmetry is
  gratuitous — swap for uniformity when next touching the file.
- TS `CursorIterator.return()` is not chained behind `pending`
  (`cursor-iterator.ts:225`): a manual driver overlapping `next()` and
  `return()` can have the in-flight `doNext` repopulate the buffer after
  `return()` cleared it. `for await` never does this; polish only. Same
  class: `return()` during a still-in-flight FIRST `next()` (cursor id not
  yet known) skips the cancel, leaving that cursor to the idle-timeout
  backstop — acceptable, worth a one-line comment.

---

## 6. Prioritized action list

1. **N-1** — tie-run-ceiling livelock: offset-bookmark fallback (or typed
   error) + small-cap test. *(Release blocker for cursors.)*
2. **N-2** — null/missing/NaN/mixed-type keyset row loss: pin with failing
   tests; document in `CURSORS.md` + `KNOWN_LIMITATIONS.md` now; two-phase
   tail scan as its own follow-up task. *(Release blocker in undocumented
   form — silent data loss.)*
3. **N-3** — create the actual `Int`↔`F64` follow-up task (exact i64/f64
   comparison, no BigInt needed) + the `KNOWN_LIMITATIONS.md` §7 bullet.
   *(Docs half must ship with the release; code fix can follow.)*
4. **N-8** — fold the doc/contract updates into whichever of N-1/N-2/N-3
   commits land.
5. **N-5** — operations-guide paragraph on `cap / max_result_size`
   concurrency math.
6. **N-4** — cursor-path single serialization (perf follow-up).
7. **N-6 + N-7 + N-9** — one small polish batch.
