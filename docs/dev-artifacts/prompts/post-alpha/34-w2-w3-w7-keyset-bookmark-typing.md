# Brief: W-2 + W-3 + W-7 — keyset cursor bookmark typing gaps (#789)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

This brief bundles three related findings from the `@fh` Wave D review
(`docs/dev-artifacts/research/2026-07-24-wave-d-review.md`, findings W-2,
W-3, W-7) — all about the keyset bookmark mechanism not checking whether
the ORDER BY column's actual value TYPE is safe for the boundary-filter
scheme, the same shape of bug CR-D2 already fixed for `Null`.

## Background — read before touching code

`query_value_to_filter_value` (`crates/shamir-query-types/src/filter/filter_value.rs:213-230`)
converts `QueryValue::Bin`/`QueryValue::List` to a `FilterValue` SUCCESSFULLY
(`Binary`/`Array` respectively) but returns `None` for `Map`/`Set`/`Dec`/`Big`.

`compare_values` (`crates/shamir-engine/src/query/filter/resolve.rs:218-283`)
has NO `(Value::Bin, Value::Bin)` or `(Value::List, Value::List)` arm — its
own doc comment (line 202) says "Only the scalar arms
(Null/Bool/Int/F64/Str) participate" — both fall through to the catch-all
`_ => None` (line 281). It DOES have working `Dec`/`Big` arms (Int↔Dec,
Big↔Int, Big↔Dec, Big↔Str, etc. — CR-C5's territory), so `Dec`/`Big` ARE
comparable via `compare_values`, just NOT convertible to a `FilterValue` for
building a `boundary_filter`.

This mismatch between "what `query_value_to_filter_value` can convert" and
"what `compare_values` can actually compare" is the root of both W-2 and
W-3:

- **W-2**: `Bin`/`List` converts fine to a `FilterValue` (so `boundary_filter`
  builds a filter that LOOKS valid), but `compare_values` can never evaluate
  it as true against any row (no comparison arm exists) — the boundary
  filter silently matches nothing past page 1, exactly CR-D2's failure
  shape (`has_more: false`, clean, silent, no error).
- **W-3**: `Dec`/`Big` does NOT convert to a `FilterValue` at all
  (`query_value_to_filter_value` returns `None`), so `boundary_filter`
  itself returns `None`, and `fetch_keyset_page`'s
  `let Some(filter) = boundary_filter(...) else { return Err(...) }`
  (`cursor_handlers.rs:691-696`) hits the hard error "cursor: keyset seek
  key has no comparable filter form" on every `FetchNext` past page 1 — even
  though `Dec`/`Big` VALUES are perfectly comparable via `compare_values`,
  the BOOKMARK mechanism just can't express them as a filter.

Both are symptoms of the same root cause: `seek_key` is set from whatever
raw `QueryValue` the ORDER BY column holds, with NO check that this
specific value can actually round-trip through `boundary_filter`'s
`query_value_to_filter_value` call. CR-D2 already established the correct
FIX PATTERN for exactly this shape of problem (a data-dependent property,
not a query-shape property) — this task generalizes it.

## Fix — treat "not safely keyset-comparable" as equivalent to "value absent"

`seek_key` is currently set from two places, NEITHER of which checks
convertibility:

1. **`create_cursor`'s first-page bookmark** (`cursor_handlers.rs`, inside
   the `if has_more && mode == PaginationMode::Keyset` block, currently
   around line 1098-1120) — extracts the last row's ORDER BY value via
   `order_by_field_value` and uses it directly as `seek_key`.
2. **`bookmark_from_tail`** (`cursor_handlers.rs:612-635`) — same pattern,
   extracts `last_value` via `order_by_field_value` and returns it as
   `next_seek_key` unconditionally.

At BOTH sites, after extracting the candidate value, check whether
`query_value_to_filter_value(&candidate)` returns `Some(_)` before using it
as the seek key. If it returns `None` (a `Map`/`Set`/`Dec`/`Big`/anything
else not convertible — or, per W-2's finding, a value that CONVERTS but
that `compare_values` can never actually match against another value of the
same shape, see below for how to detect THAT sub-case), treat this exactly
like the EXISTING "no seek_key" case the code already has a safety net for:
`cursor_handlers.rs`'s own doc comment (~lines 1383-1395) documents that a
`Keyset`-mode cursor with `seek_key == None` "falls back to the row-count
bookmark for THIS call only" — reuse that exact mechanism, do not invent a
new one.

For W-3 (`Dec`/`Big`, and anything else `query_value_to_filter_value`
already refuses): this is a simple `Option`-chaining addition — filter the
candidate through `query_value_to_filter_value(&v).is_some()` before
accepting it as `Some(v)`, else treat as `None`. This alone closes W-3
completely (a `Dec`/`Big` seek key value is never handed to
`boundary_filter` in the first place, so the hard error can never fire —
the cursor instead rides the offset bookmark for that call, same
degraded-but-correct behavior CR-A4's existing safety net already provides
for the "ORDER BY field absent from projection" case).

For W-2 (`Bin`/`List`): convertibility ALONE is not enough — `Bin`/`List`
DOES convert to a `FilterValue`, but `compare_values` can never match it.
You have two acceptable approaches, pick whichever is cleaner given what
you find when you look at the code:

- **(a) Extend `compare_values` with real `(Bin, Bin)`/`(List, List)` arms**
  (e.g. lexicographic byte comparison for `Bin` — `Ord` on `Vec<u8>` is
  already lexicographic; a similar recursive/lexicographic comparison for
  `List`, mirroring how ORDER BY's `QvSortKey` machinery in `order.rs`
  likely already needs SOME total order for these types if they're used as
  sort keys at all — check whether `order.rs` already has a working
  `Bin`/`List` sort-key comparison you can reuse/mirror, since if ORDER BY
  already sorts these correctly, only the BOUNDARY FILTER evaluation is
  missing the corresponding arm). This is the more complete fix and
  probably not much code if `order.rs` already solved the "how do I total-
  order a `Bin`/`List` column" problem.
  - **(b) If (a) turns out to be a bigger change than this task's polish-batch
  scope warrants** (e.g. `List`'s recursive comparison opens more edge
  cases than expected), fall back to the SAME "not safely keyset-
  comparable" detection CR-A4/CR-D2 already use: after extracting a
  candidate seek value, ALSO check (in addition to
  `query_value_to_filter_value`'s `Some`/`None`) whether `compare_values`
  can actually produce a total order for values of this shape — the
  simplest robust signal is: is this value's `QueryValue` discriminant one
  of `Bin`/`List` (currently uncomparable)? If so, treat as unsafe exactly
  like a failed `query_value_to_filter_value` conversion (fall back to
  offset for that call). This is a narrower, more conservative fix than (a)
  but fully closes the SILENT ROW LOSS (converts it into the documented,
  understood offset-mode degradation instead).

State clearly in your final report which of (a)/(b) you chose and why —
this is exactly the kind of judgment call the brief cannot fully pre-decide,
similar to how CR-D5's N-7 required an investigate-then-decide resolution.

## W-7 — offset-fallback duplicate-row doc gap (one-sentence fix)

CR-D1's offset-mode fallback (and now potentially this task's own new
Bin/List/Dec/Big fallback triggers) over a mixed-type/NaN column can
additionally DUPLICATE rows, not just lose them as currently documented —
`state.offset` undercounts the true position in the global sorted order
once earlier keyset pages silently skipped incomparable rows via the
boundary filter. Find the existing disclosure in
`docs/guide-docs/client-server-protocol-spec/CURSORS.md` (§1.1, added by
CR-D2) and `docs/guide-docs/KNOWN_LIMITATIONS.md` §6 (also CR-D2) that
currently promises only "rows may be silently dropped" for the
mixed-type/NaN residual — extend both to also mention that a subsequent
offset-mode fallback triggered by such a column may additionally DUPLICATE
some rows already returned before the fallback, not just omit others. This
is purely a documentation correction; no code change for W-7 itself. If
your W-2 fix (option (a) above) fully closes the `Bin`/`List` case, narrow
this doc update's scope accordingly (it would then apply only to the
already-accepted mixed-type/NaN residual, not to `Bin`/`List` anymore).

## Docs (do NOT skip)

- `docs/guide-docs/client-server-protocol-spec/CURSORS.md` §1.1: extend
  with W-2 (if you chose fix (b), a documented residual; if (a), no new
  residual — say so) and W-3 (now CLOSED, mirroring how CR-D2's Null fix is
  described as closed there) and W-7's duplication clarification.
- `docs/guide-docs/KNOWN_LIMITATIONS.md` §6: mirror the same disclosure
  updates.

## Tests (TDD — write failing tests first)

In `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`:

- **W-3 regression**: a keyset cursor over a `Dec` (and separately a `Big`)
  ORDER BY column — confirm this FAILS against the current code (a
  `FetchNext` past page 1 hard-errors with "no comparable filter form")
  BEFORE your fix, then passes after (every row appears exactly once,
  served via the offset-bookmark fallback once the unconvertible seek value
  is detected — assert on `PaginationMode`'s test-visible signal, mirroring
  CR-D2's own test convention, to confirm the fallback actually engaged
  rather than coincidentally succeeding some other way).
- **W-2 regression**: whichever of (a)/(b) you chose — if (a), a keyset
  cursor over a `Bin` (and `List`) ORDER BY column now produces a correct
  total order/pagination with every row exactly once; if (b), a test
  pinning the offset-mode-fallback behavior (every row still appears
  exactly once, just via the degraded bookmark) instead of the OLD silent
  drop.
- **Regression**: every existing keyset test over a `Str`/`Int`/`F64`
  column (the types that were ALREADY safe) must stay green and must still
  actually exercise the Keyset code path, not accidentally start routing
  through the new fallback check for a column that was never unsafe (same
  concern CR-D2's own brief called out).
- **W-7**: no new test required if this stays doc-only per the brief's own
  scope — confirm via review, not a new assertion.

## Gate

```
cargo fmt -p shamir-server -p shamir-engine -- --check
cargo clippy -p shamir-server -p shamir-engine --all-targets -- -D warnings
./scripts/test.sh -p shamir-server -p shamir-engine --full
```

All must pass before returning. Primary code area: `shamir-server`
(`cursor_handlers.rs`, its tests, `CURSORS.md`, `KNOWN_LIMITATIONS.md`) and,
only if you chose W-2's option (a), `shamir-engine`
(`query/filter/resolve.rs`'s `compare_values`, possibly `query/read/order.rs`
if a `QvSortKey` arm needs adding/reusing). Do NOT touch CR-D1's tie-run-
ceiling logic, CR-D3/W-1's numeric-comparison fix, or CR-D4's serialization
plumbing — this task is scoped to the bookmark-typing gap alone.
