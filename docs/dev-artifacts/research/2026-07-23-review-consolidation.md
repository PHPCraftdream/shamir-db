# Review consolidation → task actualization, 2026-07-23

Consolidates BOTH reviews of the RI-15 + FG-5 wave into one
finding-by-finding disposition, and records how the TaskList was
actualized as a result. This is the durable rationale behind tasks
#760–#766 (Wave A, pre-existing) and the Wave B/C tasks created alongside
this document.

## Sources

1. **User-relayed review** (2026-07-23, in-chat; scope `33109972..8a499694`,
   ~286 files / ~22k added lines). Produced the six P0 release blockers +
   a High-priority tier + an optimizations tier. Wave A (#760–#766) was
   created from its P0 tier before this consolidation.
2. **`@fx` follow-up review** — `docs/dev-artifacts/research/2026-07-23-wave-review-followup.md`
   (committed `a889d672`; scope `42dccbeb..bc3c99b8`, explicitly EXCLUDING
   the six already-known P0s). Findings R-1…R-11, P-2…P-6, B-1…B-5.

## Wave A state at consolidation time

- #760 CR-A1 (cursor ACL) — **done**, `b860765c`.
- #761 CR-A2 (terminal-page leak) — **done**, `9f7b9d55`.
- #762 CR-A3 (page_size validation) — **in progress** (delegated agent
  mid-flight at consolidation time).
- #763 CR-A5 (byte budget + page byte cap), #764 CR-A4 (keyset
  tie-breaker), #765 CR-A6 (bootstrap token), #766 CR-A7 (docs truth) —
  pending, chained.

## Disposition table — every finding, one line each

| Finding (source) | Sev | Disposition |
|---|---|---|
| Cursor ACL bypass (rev 1 P0) | P0 | DONE — #760 (`b860765c`) |
| Terminal-page cursor leak (rev 1 P0) | P0 | DONE — #761 (`9f7b9d55`) |
| page_size=0 infinite loop (rev 1 P0) | P0 | IN PROGRESS — #762 |
| Cursor bypasses byte budget + result cap (rev 1 P0; = R-7 @fx) | P0 | COVERED — #763 (row cap: #762; byte cap + budget wiring: #763) |
| Keyset tie loss on duplicate ORDER BY (rev 1 P0) | P0 | COVERED — #764 (incl. fixing pagination mode once at creation) |
| Bootstrap token not one-time (rev 1 P0) | P0 | COVERED — #765 |
| Stale KNOWN_LIMITATIONS / README overclaims (rev 1 + R-4/B-4 @fx) | MED | COVERED — #766 (description extended by this consolidation) |
| **R-1 @fx: concurrent DELETE breaks cursor snapshot stability** | HIGH | **NEW TASK — CR-B1** (release blocker; engine `read_as_of` enumerates CURRENT ids, not pinned-version ids) |
| **R-2 @fx: byte budget gates residency, not allocation** (+ P-2 double serialization — same code area) | HIGH | **NEW TASK — CR-B2** (upfront estimate-reserve → post-hoc shrink, single serialization; else reword all claims) |
| R-5 @fx: FetchNext default-page-size contract unimplementable; `default_page_size` dead field | MED | **NEW TASK — CR-B3** (make wire field `Option<u32>` with stored fallback, or delete field + doc claim) |
| has_more off-by-one on exact multiples (rev 1 High; = B-3 @fx) | MED | **NEW TASK — CR-B4** (peek-ahead `page_size + 1`) |
| Cursor + with_version returns no versions (rev 1 High) | MED | **NEW TASK — CR-B5** (reject `with_version=true` at CreateCursor, or document) |
| TS CAS types `number` vs `bigint` (rev 1 High) | MED | **NEW TASK — CR-B6** |
| Restore: ticket invalidation after swap + manifest hardening (rev 1 High) | MED | **NEW TASK — CR-B7** |
| R-3 @fx: `by_session` entry leak + B-1 @fx: hasher convention in both registries | MED/LOW | **NEW TASK — CR-B8** (one registry-chores commit) |
| Release workflow runs only lib tests before tag (rev 1, pipeline section) | MED | **NEW TASK — CR-B9** (full gate + TS + smoke as tag-release dependency) |
| Offset/keyset mode switching mid-cursor (rev 1 High) | MED | COVERED — folded into #764's description at Wave-A creation |
| Replication reseed / Resume-hits-same-gap (rev 1 High) | MED | **DEFERRED** — roadmap R2; a full reseed is a campaign, not a task. The narrow `mark_subscription_resync_required` read→SetOp race noted for a future conditional-update fix |
| R-6 @fx: FetchNext O(table)/page + full server-side materialization | MED→HIGH | Split: cost-model DOCS → #766 (extended); cheap batched-lookup optimization → **NEW TASK — CR-C3**; engine-owned cursor → deferred (roadmap) |
| R-9 @fx: cursors config no validation / no operator docs | LOW | **NEW TASK — CR-C1** (polish batch) |
| R-10/R-11 @fx: TS concurrent `next()` guard, `return()` throw masking; Rust error-path close doc | LOW | CR-C1 (polish batch) |
| P-3 @fx: ByteBudget fast path not lock-free; oversized-acquire starvation undocumented/untested | LOW/MED | CR-C1 (polish batch) |
| P-4/P-5/P-6 @fx: per-page clones, per-record mutex writes, per-page re-resolution; TS `Array.shift()` (rev 1) / recursion nit | LOW | CR-C1 (polish batch) |
| B-5 @fx: repo-not-found → misleading `unknown_db` code on cursor path | LOW | CR-C1 (polish batch) |
| B-2 @fx: test gaps (no-ORDER-BY path, page-size change, concurrent FetchNext, oversized acquire) | MED | delete-mid-scroll test → CR-B1; the rest → **NEW TASK — CR-C2** |
| Backup reads whole files into RAM for SHA-256 (rev 1, optimizations) | LOW | **NEW TASK — CR-C4** (streaming hash + copy) |
| Big-number comparisons still lossy via f64 in spots (rev 1, optimizations) | MED | **NEW TASK — CR-C5** (exact integer/decimal comparison sweep) |
| Cursor is re-execution, not a true stream (rev 1, optimizations; = R-6 root) | — | DEFERRED — engine-owned cursor/iterator is roadmap-scale; honest docs (#766) + CR-C3 mitigation now |

## Task-graph notes

- Wave B cursor-code tasks (CR-B3, CR-B4, CR-B5) are blocked by #764 —
  the last Wave-A task editing `cursor_handlers.rs` — to preserve the
  serial commit-gate discipline on a shared file set. CR-B2 is blocked by
  #763 (same `handler.rs`/`byte_budget.rs` area). CR-B1 (engine
  `read_temporal.rs`) and CR-B6/B7/B9 (disjoint file sets) are unblocked
  but will execute in id order under the sequential strategy anyway.
- #766 (docs truth) stays blocked by Wave A only. Wave B tasks that change
  wire semantics (CR-B3/B4/B5) each carry their own doc updates in their
  own commits; #766's description now instructs it to describe whatever
  has actually landed at execution time, including the R-6 cost model and
  the R-2-dependent RI-15 wording.
- Priority rationale: CR-B1 and CR-B2 are the two @fx release blockers and
  were created first (lowest ids in Wave B); the sequential executor picks
  the lowest-id unblocked task, so they run before polish.

## Deliberately NOT tasked

- Full replication snapshot reseed (roadmap R2 — campaign-scale).
- Engine-owned true streaming cursor (roadmap — the bookmark model stays
  for alpha, with honest docs).
- Moving ALL DbResponse kinds (auth/admin/error) under the byte budget —
  CR-B2 covers Execute + cursor paths; a universal writer-side budget is
  an architecture change deferred until after the release candidate.
