בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Movement B — Performance: execution plan

**Status:** plan / proposed (revision 2026-06-07). Step-by-step, measured,
each cycle closed by a commit with **before→after** numbers. Companions:
[`PERF_OPPORTUNITIES.md`](./PERF_OPPORTUNITIES.md) (the opportunity
catalogue), [`../ops/PERF_BASELINE.md`](../ops/PERF_BASELINE.md) (measured
numbers), [`PLAN.md`](./PLAN.md) (the spine — Movement B). Discipline:
`.claude/skills/opti` (`/opti`).

---

## 0. Findings that reshape Movement B (code, not the stale catalogue)

The opportunity catalogue (`PERF_OPPORTUNITIES.md`) was last graded in
2026-05; the **code has moved past it**. Verified now against the tree:

- **Opt R (reverse iteration) — DONE.** `Store::iter_range_stream_reverse`
  exists with native overrides in all 7 backends; the engine uses it
  (`sorted_index_manager` `lookup_last`/`lookup_last_k`, `read_exec`).
  MAX / `ORDER BY DESC LIMIT K` on indexed columns already takes the fast
  path.
- **Opt P (vectored multi-get) — DONE.** `Store::get_many` exists with
  **native overrides in all 7 backends** and is wired into the hot read
  path (`table/read_exec.rs:716` — "avoid N round trips via
  `Store::get_many`").
- **Opt O (covering index) — OPEN.** Sorted-index entries still carry
  `physical_value = Bytes::new()` (`index/sorted_index_manager.rs`
  :17, 183, 210, 244, 335). No projected fields stored. **The one
  genuinely-remaining disk-ceiling item.**
- **M1 / M2 — OPEN, bench-fixtured.** `benches/order_by_pipeline.rs`,
  `benches/select_projection.rs` committed; implementations not done.
- **H₂ (Persistable) — OPEN.**
- **New-feature overhead benches — partial.** `authorize_gate`,
  `permission_check`, `tx_overhead` exist. **Missing:** changefeed
  emission, validator pass, CAS `canonical_hash`.

### The pivotal consequence: re-measure Opt O *after* P
Opt O's headline "100–1000× on disk" was measured against the **pre-P**
baseline — K independent `data_store.get(id)` B-tree walks (~125 µs each).
**P already batches those into one vectored pass.** So the penalty O
removes is now "one `get_many` of K records + K decodes" — smaller than
the old "N independent walks". O is still a real win (covered queries
touch the data store **zero** times and decode **zero** full records), but
**its multiple must be re-measured before we spend a week on it.** This is
the catalogue's own rule ("ground truth before each item"), doubly true
now that R/P landed since the estimate.

---

## Measurement discipline (applies to every phase)

Per `/opti` and the catalogue's "ground truth" rule:

1. **Baseline first.** Find/add a bench that reproduces the hot path; run
   it and record numbers as **text** (mean, throughput) — don't trust
   criterion's `change:` field (compares to last run, not the baseline).
   `BENCH_QUICK=1 cargo bench -p <crate> --bench <name> -- '<filter>'` for
   fast before/after.
2. **One change per cycle.** No piggy-backed refactors — wins must be
   attributable.
3. **Tests green before post-bench** (`cargo test --workspace --lib`, and
   `--tests` where the path is integration-covered).
4. **Post-bench, explicit:** "was X ms, now Y ms → Z×". **Never commit a
   regression** — revert, find why, try another hypothesis.
5. **Idle machine for CPU benches (M1/M2).** Repeated 2026-05-27 runs
   swung ±30–80 % under parallel load. Run twice, keep the second;
   cross-check with `examples/prof_order_by.rs` /
   `examples/count_allocs_read_pipeline.rs` (deterministic, no criterion
   sampling).
6. **Commit** `perf(<scope>): <what>` with baseline / after / N× / the
   mechanic (locks, syscalls, copies, allocations removed).

---

## Phase B0 — Re-baseline + catalogue sync (½–1 day, decides B2)

**Goal:** replace stale assumptions with measured truth; this phase's
output gates Opt O.

1. **Sync the catalogue to reality (doc-only):** in
   `PERF_OPPORTUNITIES.md` mark **Opt R DONE** and **Opt P DONE** (with the
   evidence above), and re-open Opt O's win estimate as "to be re-measured
   post-P". Remove R/P from "sprint γ — NEXT".
2. **Add a disk range-with-index bench group** to
   `crates/shamir-db/benches/engine_perf.rs`: `range_with_index_sled`
   (and `_redb`), parameterised by **selectivity** (e.g. 1 %, 6 %, 10 %,
   25 %). Reuse the existing `fresh_db_sled`/`fresh_db_redb` fixtures and
   `req_range_age*` builders. (Infra precedent:
   `order_limit_top10_desc_sorted_sled`.)
3. **Run it.** Record current per-record cost and the **break-even
   selectivity** (full-scan vs index+`get_many`) now that P is in.
4. **Decision gate for B2:** if covered-query projection would still avoid
   a large fraction of the measured cost at realistic selectivity → O is
   worth the week (proceed to B2). If P already flattened the curve so the
   residual is small → down-grade O to "conditional" and prefer B3/B4.

**Deliverable:** corrected catalogue + a measured break-even number + an
explicit go/no-go on Opt O. **Commit:** `bench(engine): disk range-with-
index selectivity sweep` + `docs(perf): mark Opt R/P done, re-grade O`.

---

## Phase B1 — New-feature overhead guard benches (~1 day, regression-guard)

**Goal:** prove the write-lifecycle arc (changefeed, validators, CAS) did
**not** slow the hot paths. Not optimisations — guard rails. (Access gate
+ tx are already fixtured.)

- **B1a — changefeed emission.** Bench commit-path **with vs without**
  subscribers (live-push `try_send`) and the durable journal write.
  Expectation: non-blocking, negligible when no subscribers.
- **B1b — validator pass.** Bench a write with 0 / 1 / N bound validators
  vs none. Isolates the per-write validator dispatch cost.
- **B1c — CAS `canonical_hash`.** Bench a sequenced write (`_prev_hash`
  set) vs a plain write. Isolates `canonical_hash` + compare cost.

For each: record the delta. If any shows a **material** hot-path
regression, open a dedicated `perf` fix task (do not fix inline here —
this phase only *measures*). **Commit:** `bench(<scope>): <feature>
overhead guard` (one per sub-item, or one combined `feature_overhead`
bench).

---

## Phase B2 — Opt O: covering index ★ (GATED on B0; ~1 week)

**Proceed only if B0 confirms the win.** TDD, one slice per commit, green
gate throughout.

**Target file:** `crates/shamir-engine/src/index/sorted_index_manager.rs`
(+ index meta / DDL / planner / read-exec).

1. **DDL + meta.** Accept `"include": ["email","name"]` on
   `create_index` (sorted). Persist `included_fields` per index in the
   catalogue. Round-trip tests.
2. **Storage layout.** Index entry `physical_value` goes from empty →
   `bincode(Map of included_fields)`:
   ```text
   key   = SORTED_TAG || name_interned || encoded_value || record_id
   value = bincode(Map{ field → InnerValue })   ← NEW
   ```
3. **Write-path maintenance (the cost side).** On every insert / update /
   delete, refresh the covered projection in the index entry. **Measure
   the write-amplification** (re-run B1-style write benches) — covered
   fields are now re-encoded on each change.
4. **Planner.** Recognise a *covered* query: filter on the indexed field
   **and** `SELECT ⊆ included_fields` (no other fields, no `*`). Route it
   to an index-only path.
5. **Read-path.** Covered range query returns the projected map straight
   from the index scan — `data_store` **never opened**. True
   `O(log N + K)` on disk.
6. **Bench (verdict).** The B0 `range_with_index_sled` group, now with a
   covered variant: covered vs non-covered vs full-scan. Prove the read
   win; report the write-amplification cost alongside (honest trade).
7. **Tests.** Covered correctness; projection stays in sync across
   update/delete; graceful fallback when a query is *not* covered;
   `include` of a non-existent field rejected at DDL.

**Acceptance:** measured read win on covered disk range queries with the
write-amplification cost stated; non-covered + all existing paths
unchanged. **Commits:** per slice (`feat(index): covering-index DDL+meta`,
`feat(index): store projected fields in sorted entry`, `feat(planner):
covered-query recognition`, `perf(index): index-only covered range scan`).

---

## Phase B3 — M1: ORDER BY single-column columnar (~2–3 days)

**Bench (verdict, committed):** `benches/order_by_pipeline.rs`. Symptom
(profiled): `Value::get` inside the comparator is **85 % of ORDER BY**,
17 % of the read pipeline.

1. Single-column ORDER BY (the 90 % case) extracts into a **typed columnar
   buffer**: `Vec<i64>` / `Vec<f64>` / `Vec<&str>` (borrow lives only
   during the sort) / `Vec<bool>`.
2. Probe the column type from the first non-null value; **abort to the
   existing enum path** on a heterogeneous column. Multi-column ORDER BY
   keeps the enum path — **must not regress**.
3. **Verify:** `cargo bench --bench order_by_pipeline -- --quick` on an
   idle machine, twice; cross-check `examples/prof_order_by.rs`.
4. **Target:** ≤ 10–15 ms per single-column scenario (from ~37 ms);
   `order_by_multi_column/...` regression guard unchanged.

**Commit:** `perf(read): single-column columnar ORDER BY fast path`.

---

## Phase B4 — M2: streaming JSON serializer (~2–3 days, conditional)

**Bench (verdict, committed):** `benches/select_projection.rs`. Symptom:
`apply_select` is **61.6 %** of the pipeline, 800k allocs / 100k records
(per `examples/count_allocs_read_pipeline.rs`).

1. Add `inner_to_json_writer(value, interner, writer)` **alongside**
   `inner_to_json_value` — wraps `serde_json::Serializer` over a byte
   writer, walks `InnerValue` once, no intermediate `Value` tree. Not
   wired up yet.
2. **Equivalence tests:** streaming bytes parsed back == `inner_to_json_value`.
3. **Bench** streaming vs tree on the 100k fixture (`select_then_serialize`).
4. **Kill-criterion:** wire into `apply_select` (when the consumer is a
   byte writer — the wire codec) **only if ≥ 30 %** win on
   `select_then_serialize`; otherwise **close as not-worth-it** and keep
   the tree path. ORDER BY / DISTINCT keep the tree (need in-memory
   inspection).

**Commit:** `perf(read): streaming JSON projection (bypass Value tree)` —
or `chore: drop streaming-serializer experiment (sub-30%)` if killed.

---

## Phase B5 — H₂: `Persistable` trait + registry (~1 day, cleanup)

**Goal:** stop the write-amplification recurrence (fixed by hand twice
already — interner, counter). Not a perf spike (0–5 % direct); recurrence
prevention. Also listed under Movement A.

1. `Persistable` trait + `PersistRegistry` in `shamir-engine`.
2. End-of-batch hook calls `flush_dirty()` once — remove per-op
   `.persist().await` from `write_exec.rs`.
3. Migrate interner + counter onto it.
4. Verify no write-path regression (B1 write benches) + all tests green.

**Commit:** `refactor(engine): Persistable trait + registry (end-batch flush)`.

---

## Order & gating

```
B0 (re-baseline) ──┬─→ B2 (Opt O)        ← only if B0 says O is worth it
                   │
B1 (overhead guard, independent)
B3 (M1)  ─ independent of the disk story ─┐
B4 (M2)  ─ conditional on its own bench ──┘  (interleave with B2/B3)
B5 (H₂)  ─ anytime (cleanup; pairs with Movement A)
```

**Recommended sequence:** **B0 first** (cheap, decides the biggest spend),
then **B1** (guard rails), then the verdict from B0 picks **B2** (if O
confirmed) and/or **B3/B4** (CPU read-pipeline, disk-independent). **B5**
slots in with Movement A.

## Guardrails
- Never commit a regression; revert and re-hypothesise (`/opti`).
- One change per cycle — attributable wins only.
- Disk benches: `BENCH_QUICK` for compares; CPU benches (M1/M2) on an idle
  machine, twice, cross-checked against the deterministic examples.
- O's write-amplification is part of its verdict — report read win **and**
  write cost together; a covered index that doubles write latency for a
  niche read win is not a win.
- "Don't over-build": B4 has an explicit kill-criterion; O is gated; the
  overestimated items (I/J/M/N) stay deferred — and **Opt N is retired**
  under the OQL principle (no text parse → nothing to cache; see
  `PLAN.md` §3).

---

_Plan revision 2026-06-07 — Movement B re-scoped after verifying R+P
shipped and O remains. Next: B0 (re-baseline) decides whether Opt O earns
its week. Update as cycles land._
