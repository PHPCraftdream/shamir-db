בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Temporal ShamirDB — time as an optional dimension

**Status:** design / proposed (revision 2026-06-07). The arc that turns
ShamirDB from a current-state KV into an *optional* temporal store: query the
past (`as_of`), list a record's timeline (`history`), follow the future
(`change-since` / subscriptions), and bound how long the past lives
(retention) — all **opt-in**, the default path unchanged. Companions:
[`MVCC_CELL.md`](./MVCC_CELL.md) (the cell / write-strategy substrate),
[`PLAN.md`](./PLAN.md), `PERF_OPPORTUNITIES.md`. Discipline:
`.claude/skills/opti`, OQL principle (`PLAN.md` §3).

This document is the contract; §5 is the implementation plan. **Build on
need** — nothing here ships until a real workload asks. What is locked *now*
is the principle + the additive-field contract, so that when it lands it
slots in **without ever touching the default path**.

---

## §0 — Design law: simple by default (the law above all others)

> **Power is invisible until summoned. The default is plain. Every temporal
> capability is opt-in, and until opted into it costs the user nothing —
> not a concept, not a wire byte, not a CPU cycle.**

By default a ShamirDB table is exactly what it is today: `key → current
value`. Time, history, versions, retention **do not exist for that user**
until they reach for them. This is the governing constraint; every phase
below carries the guard **"default path byte-identical to today"** as a
per-slice invariant (not a nice-to-have).

The enforcement mechanism *is* the OQL principle: every addition is a **new
optional typed field** with the simplest default, `skip`-serialized so it is
absent from the wire until set. No textual syntax, no parse step, no "v2".

Progressive-disclosure ladder — you meet only the complexity you call:

```
Level 0 (default, ~everyone):  create table, read/write current.  Zero temporal concepts. IDENTICAL to today.
Level 1 (want history):        retention on a table (one declaration).  Now history/as_of work for it.
Level 2 (want live):           subscribe change-since.
Level 3 (want CAS):            version-stamped reads.
```

"Simple by default" also bounds *scope*: we do not ship machinery nobody
asked for. The whole arc is build-on-need.

---

## §1 — Substrate decision: one version-log (escape), not two layouts

Closing MVCC-2 (#232) had two shapes (see `MVCC_CELL.md` §6): **seqlock**
(overwrite + per-key optimistic consensus handshake) and **version-log**
(append-only, never overwrite). The deciding requirement turned out to be
**on-the-fly switchability of a table's "type"**:

- The strategy is the table's *physical layout*, so it cannot be per-tx
  (that is isolation) and must not auto-switch at runtime (that is a data
  migration). It is per-table, chosen at DDL.
- **But** if "type" is to be a true superstructure option — toggled live,
  no reshape — then the *substrate must be uniform*. Two byte-layouts force a
  migration to switch; one layout makes the "type" a pure policy.

**Decision: the substrate is a single version-log (append-only).** Each
write appends a new version (`key::version → value`); "current" = the max
version (O(1) via the cell pointer); a snapshot at `Vs` reads the largest
version `≤ Vs`. Consequences:

- **MVCC-2 dissolves by construction.** Append never overwrites, so no
  snapshot can read a value written after it opened (new versions have
  higher numbers; a snapshot reads `≤` its own). The fast/slow archive fork —
  and the whole class of "two paths, the cheap one skips the invariant" — is
  gone here. The seqlock **consensus handshake becomes unnecessary** (there
  is nothing to coordinate; it was only ever a patch for overwrite).
- **The "type" is now a runtime retention policy**, not a layout — switchable
  on the fly (forward only; you cannot resurrect versions already vacuumed).
- **Visibility is no new complexity** — "largest version ≤ snapshot" is
  exactly what `get_at` already computes; append merely merges `main`+
  `history` into one log so the "main or history?" fork disappears.
- **Cost:** every write appends even for current-state tables (≈2 ops with
  eager vacuum vs one in-place overwrite). Mitigated to near-free on an **LSM
  engine** (fjall): append is native, vacuum ≈ compaction it already does.
  Felt more on a B-tree engine. (Engine↔substrate affinity — see below.)
- **One substrate to loom/maintain**, not two.

This supersedes `MVCC_CELL.md` §6's two-impl `WriteStrategy` trait framing and
§7's R2 (the trait extraction is dropped — there are not two layouts to
abstract over; there is one substrate + a policy on top). #232 closes inside
this substrate; #237 is re-scoped from "extract trait" to "build the
version-log substrate + retention".

**Engine affinity (orthogonal axis, see `MVCC_CELL.md` §6 / the layering):**
the version-log is cheapest on append-structured engines (LSM: fjall). The
durability engine (sled/redb/fjall/…) and the version-log substrate compose
through the `Store` trait and are tested on orthogonal axes (strategy
correctness on `in_memory`; engine durability via the conformance suite) — no
N×M test explosion.

---

## §2 — Temporal OQL contract (additive optional typed fields)

For existing queries: **nothing changes**. The substrate is below the query
layer; reads/writes use the same DTOs and return the same results. Every
temporal capability is an additive optional field that defaults to "now".

**Read — a `Temporal` selector on `ReadQuery`:**
```
ReadQuery { ..., temporal: Temporal }     // #[serde(default)] = Latest, skip-serialized

Temporal =
  | Latest                                   // default — today's read
  | AsOf(At)                                  // point-in-time snapshot (ONE value/state)
  | History { from: Option<At>, to: Option<At>, limit: Option<u64>, order: Asc|Desc }
                                             // the LIST — a key's timeline of versions

At = Version(u64) | Timestamp(...)           // Version is exact/cheap; Timestamp resolves ts→version
```
- `AsOf` → the world as of that point (one value per matched key).
- `History` → `[{ version, ts?, value }, …]` per key — the changelog,
  paginated/ranged/ordered. Bounded by retention (you see only the past kept).
- `Latest` (absent field) → byte-identical to today.

**Result — optional version stamp** (`with_version`): a normal read may
optionally return each record's `version` so the client can hold an `as_of`
cursor and do optimistic CAS (complements the canonical-hash CAS, Phase 3a).
Off by default → default results are plain.

**Change-since / subscriptions (#201):** `Subscribe { from_version: V }` — the
version-log makes the cursor durable and gap-free (CF-1 gap-marker + CF-2
heartbeat). "List the past" (bounded, `History`) and "follow the future"
(tail, subscribe) are the **same log read from two ends** — one mechanism,
three payoffs (internal MVCC snapshots · user history · live subscriptions).

---

## §3 — Retention & purge

### Two distinct features — do not conflate
- **History-version expiry** (this section): keep `current`, drop OLD
  *versions* per policy. "How long does the past live."
- **Record/row TTL** (a *separate* future feature): delete whole rows older
  than X (data expiry). NOT the same as history expiry.

### Retention is an orthogonal STRUCT, not an either/or enum
Time and count are independent axes, each optional, each set/changed alone:
```
Retention {
  max_age:   Option<Duration>,   // CAP by time:  prune history older than this
  max_count: Option<u64>,        // CAP by count: keep at most N versions/key
  min_count: Option<u64>,        // FLOOR by count: always keep ≥ M recent versions,
                                 //   even past max_age (guarantees recency for
                                 //   rarely-changed keys)
}
```
- all `None` → **Forever** (eternal; manual purge only).
- `max_count: Some(0)` (the default) → **CurrentOnly** (no history; the
  simple-by-default state).
- caps **intersect** (the tighter prunes); the **floor overrides the
  age-cap**. Validation: `min_count ≤ max_count` when both set.

Per-key keep/prune rule (rank `r`, 1 = newest old version; age `a`; `current`
is always kept separately):
```
r ≤ min_count            → KEEP   (floor wins)
else r > max_count       → prune
else a > max_age         → prune
else                     → KEEP
```

**Change independently = patch semantics.** Altering retention sets/clears
ONE axis without touching the others (`set max_age = 60d` leaves count/floor
intact). It is **on-the-fly** (a policy change, no data reshape; forward only
— already-vacuumed past does not return).

### The sacred invariants (above every retention knob)
Retention knobs are only an **upper bound on the history we'd like to keep**.
Two lower bounds always win:
1. **Live-snapshot floor** — never reclaim a version a currently-open
   snapshot / long-running tx can still see (else a reader falls into `None`
   → MVCC corruption). Vacuum reads the gate's oldest-active-snapshot
   watermark and never prunes at/above it.
2. **Current is sacred** — the latest version of a key is never pruned,
   whatever its age (a row written 100d ago and untouched is still current).

```
effective_keep(key) = { current }  ∪  versions_pinned_by_live_snapshots
                                    ∪  versions_within_retention_knobs
```
This floor/current invariant is the subtle correctness point — **loom/test
it explicitly.**

### Manual purge — the imperative twin, both predicates
```
PurgeHistory {
  table,
  scope: OlderThan(Timestamp)      // "older than this date"  ← supported
       | OlderThanAge(Duration)    // "older than this age"   ← supported
       [ | BeyondCount(u64) ]      // (count-purge — optional, same orthogonal axis)
  // optional key/filter scoping
}
```
Same mechanism as background vacuum, user-triggered with an explicit
predicate. **Best-effort-safe**: reclaims everything matching the predicate
that is NOT pinned by a live snapshot and is NOT `current`; pinned versions
survive until their snapshot closes. A user command can never tear a live
reader.

### Mechanics & honest costs
- **Timestamp-per-version** is required for date/age predicates: the version
  is a monotonic counter, not a clock. Store a commit wall-time per version
  (changefeed events already carry version and can carry ts). "Older than
  age" = `now − age → ts`.
- **Wall-clock caveat** (rust-intel §B-time): retention means *calendar*
  time, so wall-clock is correct — but a version's recorded ts is its commit
  wall-time and is subject to NTP/skew; document it. Do not use a monotonic
  clock for retention semantics.
- **Vacuum on LSM** = tombstone + compaction (the engine already compacts) →
  cheap, async, off the hot path. On a B-tree it is explicit deletes.
- **Subscriber cursor vs purge:** a slow subscriber whose `from_version`
  falls below the retained window gets the **CF-1 gap-marker** ("re-sync") —
  the foundation already exists.

---

## §4 — What changes for the user

- **Existing queries:** nothing. Same DTOs, same results, substrate invisible.
- **Newly expressible (opt-in):** `as_of` point reads, `history` timelines,
  `change-since` subscriptions, version-stamped CAS, and the retention
  policy + manual purge on tables that opt into history.
- **Mental model:** tables that opt in gain a *queryable time dimension* —
  KV → temporal store, per-table, by policy. The simple user never meets any
  of it.

---

## §5 — Implementation plan (corrected; OQL · DDL · macros · tests · e2e)

Each phase obeys §0: **the default path stays byte-identical**; every
addition is an optional typed field defaulting to "now/current". TDD; one
slice per commit under the green gate; the MVCC substrate is loom-verified.

**T0 — Contract & design law** ✅ (this document). Locks: §0 law, §1
version-log substrate, §2 OQL shapes, §3 retention struct + invariants +
purge. Reconcile `MVCC_CELL.md` §6/§7 (drop the two-impl trait; on-the-fly ⟺
version-log).

**T1 — Substrate: version-log + retention engine** (feature, loom; closes #232).
- T1.1 version-log layout (append; cell → latest; read = scan `≤` snapshot),
  built in the R1-named substrate region of `mvcc_store.rs`.
- T1.2 retention engine: orthogonal knobs (`max_age`/`max_count`/`min_count`),
  vacuum honoring the keep/prune rule **and** the sacred floor/current
  invariants; per-table policy, runtime-patchable.
- T1.3 timestamp-per-version (commit wall-time; reuse changefeed stamping).
- T1.4 **loom** verification; flip the `mvcc2_*` / `covering_delete_window_*`
  characterization tests (the window now cannot occur).

**T2 — OQL DTOs** (`shamir-query-types`, additive/optional):
`Temporal` (Latest|AsOf|History), `At` (Version|Timestamp), optional
result version-stamp, `Retention` struct (3 orthogonal Options),
`PurgeHistory`, `Subscribe{from_version}` (with #201). Serde roundtrip +
**backward-compat** tests (absent field ≡ Latest/CurrentOnly, byte-identical).

**T3 — DDL & admin ops:** `retention` on create/alter-table admin DTO with
**patch semantics** (set/clear one axis, leave others); `PurgeHistory` admin
op (both predicates); `authorize` (retention/purge → Manage; reads → Read).
On-the-fly retention change wired (policy swap, no reshape).

**T4 — Execution** (`shamir-engine`): `as_of` read (resolve at the user
version via the substrate; ts→version resolve for `Timestamp`), `history`
scan (key timeline list, ranged/limited/ordered, retention-bounded),
`change-since` (log tail / one-shot list), retention vacuum loop. Wire
through `read_exec` / `table_manager` / the batch executor; honour isolation
interplay; the default `Latest` path untouched.

**T5 — Builder & macros** (`shamir-query-builder` + macros, guest re-export):
builder `.as_of()` / `.history()` / `.with_version()`; `retention!{ … }` and
`alter_retention!(t).set_max_age(…)` mirroring the **orthogonal named-optional
struct** (any subset; partial alter never resets siblings), not a positional
either/or. Dotted/guest re-export per the thin-waist (QW3/B2). Macro-expand
tests.

**T6 — Tests + e2e:**
- *Temporal reads:* `as_of(v)` returns that version; `as_of(latest) ≡ Latest`;
  `history` returns the ordered timeline; range/limit/order honoured.
- *Retention matrix (orthogonality):* age-only, count-only, min_count-only;
  age∩count (tighter prunes); **min_count overrides age**; patch-independence
  (change one axis, others unchanged); `None…`=Forever; `max_count:0`=
  CurrentOnly; `min ≤ max` validation.
- *Sacred invariants:* current never pruned; live-snapshot floor beats every
  knob and beats manual purge; on-the-fly change is forward-only.
- *Purge:* `OlderThan(date)` and `OlderThanAge(age)` both reclaim correctly,
  skip pinned/current.
- *Change-since:* versions `> V`, gap-free (CF-1); cursor-past-window → gap.
- *Backward-compat (per-slice invariant):* no temporal field ⇒ today's result.
- *Macros:* expansion → correct DTOs (trybuild/expand).
- **E2E (full stack):** client → server → engine → version-log substrate,
  exercising create-with-retention → write history → `as_of` → `history` list
  → `PurgeHistory` → `subscribe change-since`, end to end on a real (disk)
  engine.

**Dependencies / sequence:**
```
T0 (contract) ✅
   ├─ T1 substrate (loom) ──────────┐   gates real history data
   └─ T2 OQL DTOs (pure data) ──────┤   parallel — just types
                                    ↓
                          T3 DDL/admin  (needs Retention DTO)
                                    ↓
                          T4 execution  (needs substrate + DTOs)
                                    ↓
                          T5 builder/macros (needs DTOs)
                                    ↓
                          T6 tests + e2e (TDD throughout; full integration after T4/T5)
```

---

## §6 — Honest framing

- **Large arc, build-on-need.** Heavy/risky piece = **T1** (substrate +
  loom). OQL/DDL/macros/tests (T2/T3/T5/T6) are additive, low-risk, but
  useless without T1.
- **#232 (MVCC-2)** closes *inside* T1 (the version-log dissolves it — not a
  seqlock patch). **#237** re-scoped: not "extract a `WriteStrategy` trait"
  (there are not two layouts) but "build the version-log substrate +
  retention". **#201 (subscriptions)** shares the change-since machinery (T4)
  and the CF-1/CF-2 foundation.
- **OQL discipline throughout:** optional typed additive fields, default
  "now/current", `skip`-serialized — no text, no parse, no "v2", old clients
  untouched.
- **Cost owned honestly:** always-append substrate (≈free on LSM via
  compaction; felt on B-tree); ts-per-version storage; wall-clock skew
  caveat; the floor/current invariant must be loom-grade.

---

## §7 — Architect's orchestration runbook (driving /crush)

For me, the orchestrator. The WHAT is §5; this is the HOW-I-drive-agents so
each step is easy and safe. Empirical base: this session's S2 (lost-wakeup
caught on re-delegation) and R1 (test-module-untouched invariant).

### 7.0 The standing loop — every slice, no exceptions
1. **I pre-decide the architecture** (agents cannot ask → they guess → MVCC
   bugs). Bake every decision + the acceptance invariant + the named hazards
   into a prompt FILE `.crush/stdin/<session>.prompt`.
2. **crush does NOT commit.** Prompt always ends: "leave changes in the
   working tree, run the gate, report; do NOT commit."
3. Launch: `crush sessions reap` → `crush run --role smart --session
   temporal-<phase> --timeout 60m < prompt > .out 2> .err`, background.
4. **Liveness watchdog** ~10 min (`crush sessions locks <id>`); on a stale-
   lock collision reap + re-run same id; on my-timeout re-run same id larger.
5. **Zero-trust on completion (MINE, never the envelope):** full `git diff`;
   re-run the gate myself (`fmt --all --check` · `clippy --workspace
   --all-targets -D warnings` · `test --workspace --lib`); check the slice's
   **specific invariant** (below); tests non-vacuous (fail without the
   change); no out-of-scope edits; the **named hazard** handled.
6. Off → **re-delegate into the SAME session** with a tight "FIX X" prompt
   naming exactly what's wrong. Hand-fix only true one-liners.
7. Clean → **I commit** (verified message) + update the task.

### 7.1 Slicing law
Each crush run = **one landable, genuinely-USED, verifiable unit**. No
types-only slice (dead code fails `-D warnings`). If a slice can't compile-
clean used end-to-end, it's mis-cut — merge or re-slice.

### 7.2 Never delegate (I do these myself)
Architecture decisions · contract/doc writing (chat-context-bound) · loom-
result interpretation & concurrency design · slicing decisions · the commit
& zero-trust gate.

### 7.3 Hazard catalog — name the relevant ones in every prompt
- **Floor/current invariant** (T1/T4 vacuum): never reclaim a version a live
  snapshot needs; never prune `current`. (The new MVCC-core hazard.)
- **Lost-wakeup** (any new wait/notify): register `Notified` via `enable()`
  BEFORE dropping the guard. (S2 bit us here.)
- **Wall-clock vs monotonic** (T1c retention ts): retention = wall time;
  document skew; never monotonic for it.
- **Proc-macro guest re-export** (T5): absolute-path issue (QW3/B2) — macros
  must dep `shamir-query-builder` directly in guest.
- **Vacuous / pass-against-the-bug tests** (universal): a test must fail
  without the change.
- **Default-path byte-identical** (universal §0 law): a query/table with no
  temporal field ⇒ today's exact result — assert it in every slice.

### 7.4 Per-phase orchestration

| Slice | session | I pre-decide / bake | Acceptance invariant I verify |
|---|---|---|---|
| **T2** OQL DTOs (do first — pure data, parallel to T1) | `temporal-t2` | exact shapes: `Temporal`/`At`/`Retention`(3 orthogonal Opt)/`PurgeHistory`/`Subscribe`; all `#[serde(default)]`+skip | serde roundtrip; **absent field ≡ Latest/CurrentOnly byte-identical**; `min≤max` validation |
| **T1a** version-log substrate (CurrentOnly default) | `temporal-t1` | append layout; cell→latest; read=scan ≤ snap; eager vacuum keeps ~current | all mvcc tests green **except** `mvcc2_*`/`covering_delete_window_*` which **flip to correct** (written reason); current-read identical |
| **T1b** retention knobs + vacuum | `temporal-t1` (continue) | keep/prune rule; **floor/current invariant**; patch semantics | retention matrix (age/count/min, tighter-prunes, floor-overrides-age); floor never crossed |
| **T1c** ts-per-version | `temporal-t1` | commit wall-time per version; reuse changefeed stamp | age/date predicates resolve; wall-clock caveat documented |
| **T1d** loom sweep | I author the loom model | interleavings: write∥open-snapshot∥vacuum∥read | model-checker green; MVCC-2 cannot occur |
| **T3** DDL/admin | `temporal-t3` | retention on create/alter w/ **patch**; `PurgeHistory`; authorize(Manage) | on-the-fly retention change = policy swap, no reshape; purge skips pinned/current |
| **T4a** as_of read | `temporal-t4` | resolve at user version via substrate; ts→version | `as_of(v)` exact; `as_of(latest)≡Latest` |
| **T4b** history scan | `temporal-t4` (continue) | key-timeline list, ranged/limited/ordered, retention-bounded | ordered timeline; bounds honoured |
| **T4c** vacuum loop | `temporal-t4` (continue) | background; gate watermark; LSM=tombstone+compaction | floor/current invariants under concurrency (loom-backed) |
| **T4d** change-since | `temporal-t4` (continue) | log tail / one-shot; CF-1 gap on out-of-window | gap-free; cursor-past-window → gap-marker |
| **T5** builder + macros | `temporal-t5` | `.as_of/.history/.with_version`; `retention!`/`alter_retention!` orthogonal named-optional + partial alter; **guest re-export** | macro-expand → correct DTO; builds in guest (wasm32) |
| **T6** tests + **e2e** | `temporal-t6` | full-stack: client→server→engine→substrate scenario | e2e green on a real disk engine; full retention/temporal matrix |

### 7.5 Lock & re-delegation hygiene
One session id per phase; `continue` runs reuse the id (T1a→T1b→…). On
collision: `crush sessions kill <id> --wait 10s` then `reap`. Re-delegation
prompt = "the working tree has <state>; fix ONLY <named defect>; do not
restart; gate; report." (Exactly how S2's lost-wakeup was closed.)

### 7.6 Commit cadence
One commit per verified slice (T2; T1a; T1b; …) with the gate-verified
message; push only on the user's explicit word. Each commit message states
the slice, the invariant verified, and any flipped characterization test
with its reason.

---

### 7.7 Decision policy at forks (standing directive for this arc)
At every fork under this goal, **decide toward perfection by construction,
not the pragmatic compromise** — as we chose the universal version-log
(correct everywhere) over the cheaper retention-gated path. Perfection
*includes* loom-grade verification — reach the clean end-state via verified
slices, never a reckless leap. Concrete elevations this implies:
- **Collapse `main` into the single version-log** is a TARGET (not a
  perpetually-deferred "optional"); T1a's keep-`main`-as-cache is the
  verified stepping-stone, the single clean log is the destination.
- **loom on every concurrency-touching slice**, not only T1d.
- the sacred floor/current invariants enforced rigorously — no "good enough".
I decide forks myself (no asking) and report each decision transparently.

### 7.8 Status (2026-06-08) — plan T0–T6 implemented; loom finding

**Delivered + e2e-validated** (each slice gate-verified by the orchestrator):
T0 contract · T2 OQL DTOs · **T1a version-log substrate — MVCC-2 dissolved by
construction** (`fbd2af5`) · T1b.1 eager vacuum · T1b.2 orthogonal count
knobs · T1c ts→max_age · T3 DDL retention + on-the-fly · T4-history · T4-asof
· T4-purge · T4-change-since (#201 foundation) · T5 builder · T6 lifecycle
e2e. #232 (MVCC-2) is closed inside T1a.

**Loom finding (honest revision of §7.7's "loom-grade").** True `loom`
verification is **not applicable** to this substrate: loom instruments
`std::sync::{atomic, Mutex, Arc}`, but the MVCC core is built on
`scc::HashMap` + `tokio::sync::Mutex` (+ `AtomicU8/U64`), which loom does not
model. loom would require a `cfg(loom)`-swappable primitive layer and
loom-compatible replacements for scc/tokio — a large restructure for no gain
here. The **deterministic `PausableStore` interleaving harness IS the
verification standard** for this code, and it is *stronger* than loom for our
purposes: it reproduces the exact contended interleavings (it caught the real
MVCC-2 window, the eager-vacuum-vs-snapshot race, the lost-wakeup) rather than
sweeping a primitive-level state space the code doesn't expose. So "T1d loom"
is satisfied in spirit by the per-slice deterministic race tests; the
`loom-backed` notes above should read **`PausableStore`-deterministic-backed**.

**Remaining (optional / separate, build-on-need):**
- **collapse `main` into the single version-log** (§7.7 elegance/perf target;
  the keep-`main` layout is correct, this removes the redundant cache + ~1
  write). MVCC-core refactor → its own verified slice.
- the larger **#201 live server-push** (transport-level subscriptions; the
  one-shot `ChangesSince` queryable foundation shipped in T4-change-since).
- optional `q!`-macro sugar for temporal reads (the fluent builder shipped).

---

_Plan revision 2026-06-08 — time is now an optional dimension over the
version-log substrate (T0–T6 implemented + e2e-validated; MVCC-2 dissolved),
governed by the "simple by default" law. Supersedes `MVCC_CELL.md` §6's
two-impl write-strategy framing and §7's R2. §7.8 records the loom-infeasibility
finding (deterministic `PausableStore` is the standard here) and the optional
remainder (collapse-main, #201 live-push)._
