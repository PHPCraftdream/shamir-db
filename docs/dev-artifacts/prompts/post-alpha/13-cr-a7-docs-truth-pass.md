# Brief: CR-A7 — docs truth pass after Wave A (#766)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

This is a **docs-only** task. No Rust/TS code changes. No gate beyond the
orchestrator's own review (there's no compiler to check prose).

## Context

Wave A (#760–#765) just landed real cursor enforcement: ACL checks on both
`CreateCursor` and every `FetchNext` (CR-A1), no leaked registration of an
already-exhausted first page (CR-A2), `page_size` validation — reject `0`,
cap at `max_cursor_page_size` (CR-A3), cursor pages routed through the RI-15
byte budget with a per-page size cap (CR-A5), and a composite
`(order_value, record_id)` tie-breaker so duplicate ORDER BY values no
longer silently drop rows (CR-A4). CR-A6 (bootstrap token rotation,
unrelated to cursors) also landed and already corrected its own CHANGELOG
wording — verify but do not re-touch that bullet unless you spot a mistake.

Several docs files describe an EARLIER, now-stale state: some say cursors
"aren't implemented yet" (they are, and hardened), some overclaim what FG-5
already closes (it reduces wire/client memory, not server-side peak memory
— the server still executes an `O(table)` scan per page under the hood),
and the KNOWN_LIMITATIONS file still lists two limitations that no longer
exist (cursors, global inflight budget) since RI-15 and FG-5 both shipped.

**Important — do NOT claim Wave B fixes that have not landed.** Wave B
(#767–#775) is still pending. Specifically:
- `CreateCursor` does **not yet** reject `with_version: true` (that's
  CR-B5/#771, not done) — `ReadQuery::with_version` is currently accepted
  through a cursor unchanged; document this as a live caveat, not a "will
  be rejected" future-tense claim.
- `FetchNext.page_size` is still a required `u32` with no stored-default
  fallback (CR-B3/#769, not done) — don't describe it as optional.
- `has_more` is still computed WITHOUT a peek-ahead fetch (CR-B4/#770, not
  done) — don't claim the off-by-one is fixed.
- The concurrent-DELETE snapshot-stability gap (CR-B1/#767, not done) is
  real — the cursor's "stable snapshot" guarantee can still be broken by a
  concurrent DELETE between pages. Document this as a known limitation.
- The byte budget is still acquired AFTER execution+serialization, not
  upfront-reserved (CR-B2/#768, not done) — don't describe it as a
  pre-execution admission-control gate.

Verify current code state yourself (grep/read) rather than trusting this
brief's snapshot if anything looks ambiguous — Wave A specifics like exact
config field names (`max_cursor_page_size`), error codes
(`invalid_page_size`, `cursor_page_too_large`), and the temporal restriction
(`Temporal::Latest`-only, `cursor_temporal_not_supported` error) are already
verified correct as of this writing, but re-check before citing a specific
line number.

## Files to update

### 1. `docs/guide-docs/KNOWN_LIMITATIONS.md`, section "## 6. Results"

Currently (around the relevant lines):
- "Query results materialize fully into a `Vec`; no true server-side
  streaming to the client yet." — keep this bullet but ADD a clause: this
  is now mitigated client/wire-side by server-side cursors (FG-5,
  `CreateCursor`/`FetchNext`/`CancelCursor`), which page results so neither
  side holds the full set in memory over the wire at once — but the SERVER
  still executes a full pinned-version scan per page internally (no true
  server-side streaming cursor at the engine level), so server-side peak
  memory during a single page's execution is not reduced by cursors, only
  wire/client-side memory is.
- REMOVE the "No server-side cursors yet." bullet entirely (shipped).
- REMOVE the "No global inflight response-memory budget across concurrent
  connections yet." bullet entirely (shipped as RI-15,
  `security.query_limits.max_inflight_response_bytes`).
- ADD new limitations reflecting the current real state:
  - Cursors only support `Temporal::Latest` reads — `AsOf`/`History`
    queries are rejected outright at `CreateCursor` with
    `cursor_temporal_not_supported`, not silently downgraded.
  - `CreateCursor`/`FetchNext` do not yet reject `with_version: true` —
    combining cursor pagination with per-record CAS version stamps is
    unsupported/unverified and may produce confusing results; avoid until
    this is explicitly rejected or supported (tracked separately).
  - A cursor's "stable snapshot" is pinned at creation, but a concurrent
    DELETE on the underlying table between `FetchNext` calls can still
    disturb enumeration order/completeness (the pinned-version read
    re-enumerates via the CURRENT id set, not a truly frozen id set) —
    tracked separately as a hardening item.

### 2. `README.md`

- Around "true serializability" (search for that phrase, ~line 64): cross
  check against KNOWN_LIMITATIONS' own qualifications on predicate/range
  lock coverage (search KNOWN_LIMITATIONS.md for "predicate" / "phantom" /
  "stream" near the top) and soften the README claim so it doesn't
  overclaim relative to what KNOWN_LIMITATIONS itself admits. Keep it
  factual and short — this is a wording alignment, not a rewrite.
- Around "Constant memory usage regardless of dataset size" (search for
  that phrase, ~line 135, under a "Streaming" bullet): this is false for
  `ORDER BY`/`GROUP BY`/`DISTINCT` queries (which must materialize/sort
  the full matching set before returning even the first page) and for
  cursor bookmarks (which re-run a full pinned-version scan per page, see
  above) — qualify the claim honestly rather than deleting the bullet
  outright (streaming DOES bound wire-side memory for the unsorted case).

### 3. `CHANGELOG.md`, `[Unreleased]` section

- Verify the CR-A6 bullet (RI-9 bootstrap wording) reads correctly — it
  should already be accurate since CR-A6 authored its own fix; only touch
  it if you spot an actual inaccuracy.
- Reword the RI-15 bullet's framing of the ~64 GiB burst scenario: since
  the byte budget is STILL acquired post-execution/serialization (CR-B2 not
  landed), don't describe it as "closing the gap" in a way that implies
  pre-execution admission control — it bounds bytes on the WRITE path
  (post-serialization, held until the socket write completes), not
  execution-time memory. Adjust wording to be precise about what point in
  the pipeline the budget applies to.
- Reword the FG-5 bullet's clause "closing the `QueryResult` materializes
  as a `Vec` limitation" — this is only true for wire/client-side memory;
  the server still executes a full per-page scan and materializes each
  page's `Vec` internally before serializing it. Qualify accordingly.
- In the same FG-5 bullet (or as a follow-up sentence), mention the Wave A
  hardening that landed since the original FG-5 entry was written: ACL
  enforcement on cursor open/fetch, no leaked registration of an
  already-exhausted first page, `page_size` validation (reject 0, cap at
  `max_cursor_page_size`), byte-budget + per-page size-cap coverage for
  cursor responses, and a tie-safe composite bookmark that no longer drops
  rows on duplicate ORDER BY values.

### 4. `docs/guide-docs/client-server-protocol-spec/CURSORS.md`

This file is the most stale — it was written when cursors were pure wire
scaffolding with a placeholder `cursor_not_yet_implemented` response and
needs a real status update now that FG-5b (engine/session state) and Wave A
(hardening) have both landed:

- The top status blockquote (lines ~3-9) says the engine/session state
  "is NOT implemented yet — that is FG-5b" and every request "is currently
  answered by a placeholder... `cursor_not_yet_implemented`". This is no
  longer true — FG-5b shipped. Rewrite the blockquote to describe the
  CURRENT real status: cursors are backed by real MVCC-snapshot-pinned
  engine state, `Temporal::Latest`-only, with the Wave A hardening listed
  above.
- §1 "Overview": the "Idle-timeout eviction... detailed in FG-5b" line
  (~line 27, ~line 49) — update to state plainly rather than
  forward-reference a now-shipped milestone (check whether idle-timeout
  eviction is actually implemented in the current cursor registry code —
  if it's real, describe it as implemented; if still pending, say so
  honestly without the "FG-5b" placeholder framing).
- §2 `CreateCursor`: the "per-session cursor cap — FG-5b" parenthetical
  (~line 76-77) — same treatment, describe as implemented (verify against
  `security.cursors.max_cursors_per_session` config, already referenced in
  CHANGELOG).
- §3 `FetchNext`: no changes needed to the field table itself (`page_size`
  is still required — don't claim CR-B3's optional-with-default landed).
- §6 Errors: the line "**Current status (FG-5a):** none of the above three
  are enforced yet... placeholder `cursor_not_yet_implemented`" (~line 173
  onward) is now FALSE — `cursor_not_found`/`cursor_expired`/
  `cursor_limit_exceeded` are real enforced errors today. Remove the
  placeholder-status paragraph and the placeholder msgpack example
  entirely, replacing with a short accurate statement that all error codes
  in the table are live. The `invalid_page_size` row is already correctly
  documented (added by CR-A3) — leave it. If a `cursor_page_too_large` error
  code exists in the current code (added by CR-A5, check
  `crates/shamir-query-types/src/batch/batch_error.rs`), add a matching row
  to the errors table — verify the exact code string via
  `error_code()` in `crates/shamir-server/src/db_handler/handler.rs`.
- Add a short note (new paragraph, doesn't need its own section) for API
  consumers describing the R-6 cost model: each `FetchNext` re-executes a
  full pinned-version table scan server-side to reach the next page — the
  cost model is O(table) per page, not O(page_size); cursors reduce
  wire/client-side memory footprint, not server-side per-page execution
  cost. This matters for consumers deciding whether to use a cursor vs. a
  single large `Read` with `max_result_size_bytes` headroom.
- §7 "Out of scope for this document": if FG-5b/c/d/e have all actually
  shipped (check git log / CHANGELOG for FG-5c Rust SDK stream, FG-5d TS
  SDK stream, FG-5e e2e tests), update or remove this section — it's
  written as a forward-looking roadmap list, which is misleading once the
  referenced work is done. If any of those milestones genuinely haven't
  shipped, keep only the ones still pending.
- §8 References: the "Server implementation" bullet (~line 210-217) says
  `handler.rs`'s "compile-safety stub arms... return
  `cursor_not_yet_implemented` for all three requests today" — this is
  false now; update to point at the real dispatch (`cursor_handlers.rs`)
  instead of the old stub description.

### 5. Sweep for other stale claims

While in `docs/`, grep for `cursor_not_yet_implemented`,
`"not yet implemented"` near cursor/budget context, and `FG-5b` outside
CURSORS.md to catch any other file with the same staleness pattern (e.g.
`docs/guide-docs/client-server-protocol-spec/OPTIMISTIC_CONCURRENCY.md` or
a root-level roadmap doc might reference the same milestones). Fix what you
find using the same "describe current real state, don't overclaim
Wave-B-not-yet-landed fixes" discipline as above.

## Style

- Every wire-format example must match the REAL current shape (don't
  invent fields; if unsure, grep the wire DTO struct in
  `crates/shamir-query-types/src/wire/db_message.rs`).
- Keep edits surgical — reword/remove/add specific bullets and paragraphs,
  don't rewrite whole files or restructure sections that are already
  accurate.
- Commit message prefix: this whole task lands as a single `docs:` commit
  (the orchestrator will write and make the actual commit — you just leave
  the working tree with the edits).

## Report back

List every file you touched and, for each, a one-line summary of what
changed. If you found a stale claim not listed in this brief, call it out
explicitly rather than silently fixing it — the orchestrator will verify
before committing.
