בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Task #533: keyset-pagination record-id tie-breaker — design decision

Scoped out of G2/#526 (`docs/prompts/audit/47-perf-g2-526-keyset-short-page-fix.md`).
Originally found during #496's `@fl` review (former task #517).

## The problem, precisely

Keyset ("seek") pagination (`Pagination::After { key: Vec<QueryValue>, .. }`,
`crates/shamir-query-types/src/read/limit.rs:43-49`) asks the server for "up
to `limit` rows strictly after the tuple `key`" — `key` is the ORDER BY
column value(s) of the last row the client already has. The server encodes
`key` and walks the sorted index
(`crates/shamir-index/src/legacy/sorted_index_manager.rs::lookup_range_first_k_page`),
whose physical key layout is `[tag|name|encoded_value|record_id]` — so within
one physical index, rows are already totally ordered by `(value,
record_id)`, not just `value`.

The bug: `lookup_range_first_k_page`'s walk drops every entry whose encoded
value equals the seek value outright —

```rust
if key_value_slice(kb, value_start) == seek_encoded {
    continue;
}
```

(`sorted_index_manager.rs:784` and `:810`, unconditional, applied on the
FIRST physical page AND on every internal continuation page driven by
`crates/shamir-engine/src/table/read_index_scan.rs::read_keyset_seek`'s
stale-posting retry loop from G2/#526). This is correct for the ONE row
that established the boundary (the row the client already has), but wrong
for every OTHER row that happens to share the same ORDER BY value — those
are un-seen, un-returned rows, not duplicates. If ORDER BY value V is held
by rows `R0 (id=100, already returned to the client on a previous page),
R1 (id=200), R2 (id=300)`, and the client's next request seeks past `V`,
the server treats ALL THREE as "the already-seen boundary" and skips all
three. `R1` and `R2` are silently and permanently lost — they can never
appear on any subsequent page either, since every future request re-seeks
on the same value `V` (the client's only handle on its position) and hits
the identical unconditional skip.

This is not a rare interaction of two independent bugs — it is the direct
consequence of the wire contract itself: `Pagination::After.key` carries
only ORDER BY *values*, never a row identity. The server has no way,
looking at `key` alone, to distinguish "the exact row the client already
saw" from "a different row that happens to tie on the same ORDER BY
value." Any fix at the storage-scan level (e.g. deleting the unconditional
skip) would just move the ambiguity, not resolve it, because the
information needed to resolve it (which specific row was last delivered)
is not present in the request at all.

## Why this needs a wire-protocol decision, not a backend patch

Confirmed directly (not assumed) that today's read-path wire format has
**no row-identity channel whatsoever**:

- `QueryRecord::Direct` (`crates/shamir-query-types/src/read/query_record.rs`)
  — the variant every SELECT response actually uses — carries only the
  projected `QueryValue`, no `RecordId`. `RecordId` IS injected as a
  synthetic `_id` field, but only on the **write**-result variant
  (`InsertedRecord`) — never on read results.
- `Pagination::After.key: Vec<QueryValue>` accepts exactly the ORDER BY
  column values; there is no reserved trailing slot and no separate field
  for a tie-breaker.
- Neither the Rust query builder nor the TS client (`crates/shamir-client-ts`)
  has any existing concept of "the id of the last row returned" to round-trip.

So closing this gap for real needs, at minimum:
1. Expose a stable per-row identity in read results (extending
   `QueryRecord::Direct` or a sibling field) — itself a client-visible wire
   change every consumer (Rust client, TS client, any external integration)
   would need to tolerate.
2. Extend `Pagination::After` with an optional tie-breaker component (e.g.
   `after_id: Option<RecordId>` or append the id as a trailing element of
   `key`) that the client echoes back from the row identity in (1).
3. Change `lookup_range_first_k_page`'s skip condition from "value ==
   seek_encoded → always skip" to "value == seek_encoded AND (id compares
   not-strictly-past `after_id`, or `after_id` is absent) → skip" — i.e.
   bound the physical-key range scan by `(seek_encoded, after_id)` instead
   of an approximate value-only filter.

None of this can be done invisibly inside the engine — every legitimate
fix touches what the client sends and/or receives. That is exactly the
audit's own framing for this task, confirmed by re-reading the code myself
rather than taking the original finding on faith.

## Backward compatibility, if implemented

A trailing-optional-field design is fully backward compatible: `after_id:
Option<RecordId>` defaults to `None` for any client that doesn't send it
(old client, old query-builder version), which reproduces **today's exact
behavior** (skip-all-ties) — a silent, pre-existing, known limitation for
those callers, not a new regression. Only clients that opt in by echoing
back the new id field get correct tie-breaking. This is the same shape of
compromise `docs/design/keybytes-inline-cap-506-decision.md` and other
`*-decision.md` docs in this session settled on: additive, never a forced
migration.

## How often this actually bites, in practice

Keyset pagination is used specifically for ORDER BY + LIMIT queries over a
**sorted index**. Ties on the ORDER BY value are common exactly when the
ORDER BY column is low-cardinality relative to the table (a status field,
a category, a boolean, a coarse timestamp truncated to a shared value) —
which is a normal, not edge-case, usage pattern for this kind of query
(e.g. "list all orders with status=PENDING ordered by status, paginate").
On a high-cardinality / effectively-unique ORDER BY column (an actual
timestamp with sub-millisecond granularity, a UUID, a monotonic counter)
the bug is dormant — ties essentially never occur, so no rows are lost in
practice.  This is a genuine, silent **data-loss bug** (rows permanently
un-retrievable, not just mis-ordered) whenever the ORDER BY column is
low-cardinality, which is a realistic and not-rare shape of query.

## Recommendation

Given (a) this is a silent, permanent data-loss bug rather than a
performance nit, and (b) the fix is backward-compatible and additive (no
break for existing clients), the fix is worth doing — but it is a
multi-crate wire-protocol change (`shamir-query-types`, `shamir-index`,
`shamir-engine`, `shamir-query-builder`, `shamir-client`,
`shamir-client-ts`, plus whatever tests exercise the wire shape in each)
that deserves its own scoped task and its own review pass, not a
same-task implementation folded into this design doc. Per this task's own
instruction ("do not implement without a design review, since this
touches the client wire protocol"), this document stops at the design
decision; a follow-up implementation task should be opened ONLY with
explicit user sign-off on the wire-shape choice above (trailing optional
`after_id` field vs. some other encoding the user may prefer), since it is
the kind of externally-visible contract change this campaign's rules
single out for explicit confirmation before code is written.

## Disposition

No code change in this task. This document is the complete deliverable:
the root cause (confirmed by reading the actual skip condition and the
actual wire types, not assumed from the original audit finding), why it
requires a wire decision, the backward-compatible shape a fix would take,
and an honest assessment of real-world impact. Implementation is
deliberately deferred pending the user's explicit choice of wire shape.
