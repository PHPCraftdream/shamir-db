Task: Interner reverse-lookup spine clones the WHOLE vec on every first-touch
of a new field name. Task #501, audit finding 2.3
(`docs/audits/2026-07-06-perf-radical-o-notation.md`).

## Context (re-investigate — this campaign has already touched this file
## once this session for Op B/Opt G; read the CURRENT code, not the audit's
## line numbers, which may have drifted)

`crates/shamir-types/src/core/interner/interner.rs`, `touch_ind` (currently
~line 90) and `touch_with_id` (currently ~line 250): both have a CAS-loop
that does `let mut new_rev = (*cur).clone();` — a full clone of the ENTIRE
`Vec<Option<Arc<str>>>` reverse-lookup spine — on **every single** first
touch of a brand-new field name, not just when the vec needs to grow.
Because `new_id` is always `current_id + 1` (strictly monotonic), the vec is
always exactly at capacity when a new touch lands, so this "clone the whole
thing, resize by exactly one slot, CAS-swap" pattern runs unconditionally on
every touch. For a schema-rich cold start (bulk-loading documents with many
distinct field names — nested JSON with 10k+ unique keys), this is O(N²)
total slot-copies across N first-touches.

**This has already been measured.** `crates/shamir-engine/benches/interner_cold_growth.rs`
has an existing `interner_touch_ind_cold_growth/touch_ind_{100,200,300}`
bench (pure in-memory `touch_ind` loop, no persistence). Baseline numbers
from this session (`CARGO_TARGET_DIR=<isolated> cargo bench -p shamir-engine
--bench interner_cold_growth -- interner_touch_ind_cold_growth`):

```
touch_ind_100   588,057 ns total  (5.88 µs/touch avg)
touch_ind_200 1,623,777 ns total  (8.12 µs/touch avg)
touch_ind_300 3,353,289 ns total (11.18 µs/touch avg)
```

Average per-touch cost is growing linearly with N (confirming the O(N²)
total / O(N) average shape). Extrapolating the observed per-touch trend to
N=10,000 (the audit's stated "schema-rich" scenario) projects roughly
1.5-2 seconds of pure CPU time for the cold-start touches alone — a real,
worth-fixing one-time cost. Re-run this bench yourself to get your own
before numbers rather than trusting this extrapolation.

## IMPORTANT — do NOT implement the audit's literal suggested fix as-is;
## implement the lower-risk variant below instead

The audit's own fix sketch (finding 2.3) suggests a **segmented/chunked
spine**: `ArcSwap<Vec<Arc<[OnceLock<Arc<str>>; 1024]>>>` — growth appends a
chunk (small clone), filling a slot is a `OnceLock::set` with no vec clone.
This DOES fix the write-side complexity, but it changes the READ-side
indexing scheme from flat single-index (`rev[id]`) to two-level indexing
(`rev[id / 1024][id % 1024]`) — and this reverse spine is read on **every
single field of every single record decode** in the entire engine
(`Interner::reverse_snapshot()` / `get_str()` / `with_str()` feed
`crates/shamir-types/src/codecs/interned/codec.rs`'s
`inner_value_to_query_value_with_rev` / `record_view_to_query_value_with_rev`
and `crates/shamir-types/src/codecs/interned/validate_keys.rs`'s
`validate_keys_resolve` — together these are the hottest read path in the
whole codebase, exercised by every SELECT that returns a record, called from
38+ non-test sites in `shamir-engine` alone). Adding a divide/modulo and a
second pointer-chase to that path to speed up a RARE, one-time schema
cold-start is the wrong trade.

**Implement this instead — same O(N²)→O(N) complexity win, zero change to
the read-side indexing shape:**

1. Change the reverse spine's cell type from `Option<Arc<str>>` to
   `std::sync::OnceLock<Arc<str>>`, keeping it a **flat, single-index**
   `Vec` (no chunking): `reverse: ArcSwap<Vec<OnceLock<Arc<str>>>>`.
2. On first-touch, split the CAS-loop into two phases:
   - **Ensure capacity** (rare — only when `new_id >= cur.len()`): clone the
     current vec into a bigger one using **doubling growth** (e.g.
     `(cur.len() * 2).max(new_id + 1)`), padding the new slots with fresh
     `OnceLock::new()`. CAS-swap. Retry on CAS loss (someone else grew
     first). Total clone work across N touches is a geometric series ≈ 2N
     (O(N) total, not O(N²)) — this is the actual fix.
   - **Set the slot** (the common case — capacity already exists from a
     prior growth event): call `cur[new_id].set(arc)` directly. This is
     lock-free and requires **no vec clone at all** — `OnceLock::set` only
     touches that one cell.
   - Do NOT rely on `std`'s blanket `OnceLock<T>: Clone` impl when building
     the bigger vec during a capacity-ensure — build each new cell
     explicitly (`match old_cell.get() { Some(v) => { let c = OnceLock::new(); let _ = c.set(v.clone()); c }, None => OnceLock::new() }`)
     so this doesn't depend on which Rust version stabilized that impl.
     Confirm current MSRV/toolchain before deciding either way, but the
     explicit form is the safe default regardless.
3. `touch_with_id` (the WAL-recovery / persistence-hydration path, currently
   ~line 250) has the **identical** unconditional-full-clone bug in its own
   CAS-loop. Apply the same ensure-capacity/set-in-place split there too —
   don't fix only `touch_ind` and leave `touch_with_id` with the old O(N²)
   behavior.
4. Update the 3 read-side call sites that consume `reverse_snapshot()`'s
   slice shape (`crates/shamir-types/src/codecs/interned/codec.rs`'s two
   `_with_rev` functions, `crates/shamir-types/src/codecs/interned/validate_keys.rs`'s
   `validate_keys_resolve`) — this should be a **mechanical** signature
   change only: `&[Option<Arc<str>>]` → `&[OnceLock<Arc<str>>]`,
   `.as_ref()` → `.get()`, `slot.clone()` → `slot.get().cloned()`. The
   indexing SHAPE (flat, single `id` index) does not change, so this must
   NOT require touching the walk/recursion logic itself, only the leaf
   accessor calls.
5. `get_str`, `with_str`, `all_entries`, `entries_in_id_range`,
   `entries_after` (all currently in interner.rs) need the same mechanical
   accessor-method swap. Pay particular attention to `entries_after`'s
   documented "gap" semantics (a `Some(None)` slot = reserved-but-unswapped
   id, does not stop the scan but freezes the high-water mark) — with
   `OnceLock`, "reserved but unswapped" and "genuinely not-yet-touched-at-all"
   both read as `.get() == None`, which is the SAME collapsed semantics the
   current `Option<Arc<str>>` design already has (a resized-but-unset slot
   is also plain `None` today) — confirm this by re-reading the existing
   gap-handling doc comments and tests before assuming it's unaffected.
6. `touch_with_id`'s id-collision-detection branch (currently ~line 275:
   `if let Some(Some(existing_arc)) = rev.get(id as usize)`) needs the same
   mechanical swap to `rev.get(id as usize).and_then(|c| c.get())`.

## Scope-down guidance

If investigation reveals the doubling-Vec-of-OnceLock design has a real
correctness gap this brief didn't anticipate (e.g. a genuine race between
"ensure capacity" and "set slot" that can silently drop a touch under
concurrent load — trace this carefully, since two phases means a window
opens between them), STOP and document the specific gap + a follow-up task,
per this campaign's established pattern. Do not fall back to implementing
the audit's literal chunked-spine suggestion without flagging it to the
orchestrator first — that path trades hot-path risk for cold-path gain and
needs a conscious decision, not a silent substitution.

## TDD/regression requirement

1. All existing `crates/shamir-types/src/core/interner/tests/interner_tests.rs`
   tests must stay green unchanged (behavioral contract preserved).
2. A new concurrent-growth test: spawn many threads calling `touch_ind` with
   distinct fresh names concurrently (forcing both the "ensure capacity"
   CAS-race path and the "set in place" path to interleave); assert every
   name ends up correctly resolvable via `get_str`/`get_ind` afterward with
   no lost touches, no panics, no duplicate-id assignment.
3. A test proving `entries_after`/`entries_in_id_range`'s gap-handling
   semantics are unchanged: reserve an id (forced via a raced/aborted touch
   scenario, matching the existing "small leaked slot, harmless" comment in
   `touch_ind`), confirm the gap-freezing high-water-mark behavior still
   matches pre-change behavior.
4. Re-run the existing decode-path test suites unchanged
   (`crates/shamir-types/src/codecs/interned/tests/*`,
   `crates/shamir-types/src/record_view/tests/*`) — these exercise the
   `_with_rev` call sites and must pass with zero behavioral change, only
   the accessor-method-level type swap.

## Performance verification requirement (MANDATORY — this is a PERF task)

Use the EXISTING `crates/shamir-engine/benches/interner_cold_growth.rs`
`interner_touch_ind_cold_growth/touch_ind_{100,200,300}` bench for a direct
before/after comparison (no new bench file needed — this one already
exists and already isolates exactly the code path being fixed). Report
honest before/after ns/op at N=100/200/300, and ideally add one larger N
(e.g. 2000 or 5000 — check the bench's own per-call cost budget comments
before picking N, it targets ~10ms/call) to make the O(N²)→O(N) shape
visible at a scale closer to the audit's "10k+ fields" scenario. Follow
this repo's `CARGO_TARGET_DIR=<isolated-dir>` (note: on this Windows/bash
setup, use a POSIX-style path like `/d/dev/rust/.cargo-target-bench`, NOT
a backslash Windows path — the bash env-var expansion mangles backslashes)
convention for a clean incremental-cache bench run.

## Test scope

```
./scripts/test.sh -p shamir-types
```

## Gate

```
cargo fmt -p shamir-types -- --check
cargo clippy -p shamir-types --all-targets -- -D warnings
```

If clippy flags PRE-EXISTING lints in code you did not touch, do not fix
them here — note them in your final report instead.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Report format

```
[Investigation] Status: complete
  > Confirmed/refined which design was implemented (doubling-Vec-of-OnceLock
    per this brief, or a deviation — explain why).

[Implementation] Status: fixed / partially-fixed / deferred
  > What changed + regression tests added
  > Bench: baseline vs after, at N=100/200/300 (+ one larger N)
  > OR: deferral reason + follow-up task description (if deferred)
```
Full test/gate/bench results (exact commands + pass/fail).
