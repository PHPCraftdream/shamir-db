בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Forward Plan — Consolidate → Measure → Build the "I"

**Status:** living plan, revision **2026-06-10**. The single actionable
"what we just did and what's next" document. Per-area deep-dives stay in
their own files (linked below); this is the spine that orders them.

Companion snapshots: [`../PROJECT_STATE.md`](../PROJECT_STATE.md) (what
ShamirDB *is*), [`NEXT_PHASES.md`](./NEXT_PHASES.md) (the transactional
forward index, now historical), [`PERF_OPPORTUNITIES.md`](./PERF_OPPORTUNITIES.md)
(the measured perf backlog).

---

## 0. Where we are — the write-lifecycle arc is closed

This cycle completed a full arc: **how a record is written, governed, and
propagated** — *DDL surface (write it) → access fabric (govern it) → write
lifecycle BEFORE→commit→AFTER (validate, sequence, broadcast it)*.

| Phase | What | Commit |
|---|---|---|
| 0  | DDL reads like DML (first-class Batch methods) | `016d68b` |
| 1a | DDL↔DML ordering (`after` edges) | `f8660c0` |
| 1b A+B | Idempotent create + referential-integrity drop | `473d6a5` |
| 1b D+C | Function-folder meta persistence (#118) + introspection | `f4795e0` |
| 2a | Real DML enforcement (table-level) | `c5528f0` |
| 2b A+B | Authorize admin-ops + owner delegation | `aaf1ad7` |
| 2b C | Getter-only data-firewall via setuid | `61efb79` |
| 3a | Canonical record hash + CAS sequenced writes | `7844666` |
| 3b | Changefeed (hybrid live-push + durable journal) | `6028559` |
| #177 | Changefeed for non-transactional writes | `91820ad` |
| #178 | Structured DDL error codes | `79b5ded` |
| #179 | Changefeed version == MVCC version (P2P-ready) | `a620115` |

Resting points reached: *DDL is complete, ergonomic, safe* (1) ·
*Access is real, delegable, with a procedure firewall* (2) · *Full write
lifecycle + the foundation for "I"* (3).

---

## 0.1 Since the arc — stored procedures, WASM perf, SDK slimming

Three follow-on tracks landed (all committed, each zero-trust verified):

| Track | What | Commits |
|---|---|---|
| **Stored procedures** | `BatchOp::Call` (callable getter-functions): wire+core (`value` result, FunctionInvoker), dependency-graph participation (params+result as `$query`), `Batch::call` + `q!(call …)`. See [`STORED_PROCEDURES.md`](./STORED_PROCEDURES.md). | `0c2d468` `be3ad62` `5bad954` `446d41d` |
| **WASM runtime perf** | InstancePre (per-call **−40%**), AOT disk cache (**~2×** restart compile), pooling allocator + CoW (**+12%** concurrent). | `94d2392` `a934c59` `b53fcba` (+ bench `66e09a0`) |
| **WASM/SDK slimming** | size profile (compiled functions **−34%**), host↔guest wire-conformance test, graceful `wasm-opt`; SDK typed kinds `#[scalar]`/`#[procedure]`, per-kind examples + reachable-API docs, query-types crypto feature-gate (guest-lean-capable). See [`WASM_SLIMMING.md`](./WASM_SLIMMING.md), [`SDK_AUTHORING.md`](./SDK_AUTHORING.md). | `e05f87a` `2a901fa` `44371a3` `c434dd8` `c0c27fe` |
| **Thin-waist + builder-in-guest (B2)** | `shamir-collections` leaf (TMap/TSet, re-exported); `query-types`/`builder` severed from heavy `shamir-types` (`server` feature + optional dep) → guest-lean (`cargo tree --no-default-features` proves it; builder compiles to wasm32); `db_execute` host import (generalises db_get/insert/query) + SDK `query-builder` feature + `ctx.db().execute(&Batch)`. **Retired the deferred P4** — its premise (`QueryValue` in heavy `shamir-types`) was false. Example guest 337 KB. | `52be3b3` `e934a2f` `d71f9a0` `ca79b1f` |

Resting point: *procedures are first-class batch citizens; the function
engine is faster (hot/restart/concurrent); the guest SDK is lean and typed
per kind.* All the **cheap, safe, no-wide-refactor** work is done.

---

## 0.2 Since §0.1 — temporal, nested batches, duplex transport, TS client

Four major tracks landed between the §0.1 resting point and today (152
commits); each is fully shipped and zero-trust verified.

| Track | What | Key issues |
|---|---|---|
| **Temporal arc** | Single-log MVCC refactor: dual-write eliminated; one append-only version-log; `current` / `history` / `temporal` all resolve from it. Temporal OQL — `History` / `AsOf` / `PurgeHistory` / `ChangesSince` (additive, default-invisible). Builder `as_of` / `history` / `with_version`. Retention: per-table `SetRetention` + per-key `max_count` / `min_count` / `max_age` + eager vacuum. See [`TEMPORAL.md`](./TEMPORAL.md), [`MVCC_CELL.md`](./MVCC_CELL.md). | #237 #238 |
| **Nested batches** | Sub-batches with own tx scope, recursive execution with bind / `$param` (incl. `$param` in write-op values), nesting-depth guard, builder `sub_batch` + `param`. | #282 |
| **Duplex / multiplexing** | Async `RequestHandler`, splittable `Framer`, duplex request loop, rid-demux in both clients, resume fast-path (skip Argon2id on reconnect). **This is the transport foundation for push subscriptions.** | #292–#298 |
| **Client-TS** (TS-T0..T17) | Full pure-TypeScript client: builders, batch, interactive tx, `$query` / `$ref`, rid-demux, live-server e2e. | — |
| **Internal refactoring** | Extracted `shamir-wasm-host` + `shamir-index` from the engine; moved legacy `IndexManager` / `SortedIndexManager` into `shamir-index`; split monoliths; tests → `tests/` dirs. Engine ~64 k → ~46 k lines. Workspace now **21 crates** (was 18 at §0.2). | — |

Additionally:
- **Subscriptions design (#201)** — the client-facing live-subscriptions /
  server-push design doc ([`LIVE_SUBSCRIPTIONS.md`](./LIVE_SUBSCRIPTIONS.md))
  is **written** (no code yet). This is the design on-ramp to Movement C.

Resting point: *the DB speaks a live, temporal, nested-transactional query
language; the transport is full-duplex; a TypeScript client is feature-
complete. The foundation for push subscriptions is fully laid.*

---

## 0.3 Snapshot — the inflection (2026-06-10)

**Where we stand.** The foundation, engine pillars, and transport are
closed. Movements A and B are done (see §2.1). The single live frontier is
**Movement C — the "I"** (Interconnected): network changefeed → live
subscriptions → replication → P2P. Everything needed to start C is in
place.

**Workspace health.** 21 crates, ~2912 lib tests, green gate (fmt +
clippy `--workspace --all-targets` + lib). `.git-blame-ignore-revs` +
style-sweep discipline intact.

**Narrative thread.** *"How a record is written, governed, propagated"*
(the arc, §0) → *"how authors write code that runs inside the DB"*
(§0.1) → *"temporal history, nested transactions, full-duplex transport,
TypeScript client"* (§0.2) → **next: "how clients subscribe to live data
and replicas follow a leader"** (Movement C).

---

## 1. Maturity map

**Closed and solid (foundation + engine + transport):**
storage (6 backends, one dumb-KV trait) · transactions (SI → SSI → true
serializability + interactive + crash recovery + Level-3 pessimistic wound-
wait locking) · query engine (WHERE / SELECT / GROUP BY / ORDER BY /
pagination / cross-refs) · secondary / sorted / functional / HNSW-vector /
FTS indexes · **covering index (Opt O)** · WASM functions ("M") · access
fabric (Shomer, enforced) · DDL (complete) · validators + CAS · changefeed
(durable-journal + watermark + gap signal; version == commit_version) ·
temporal OQL + retention + MVCC single-log · nested batches + `$param` ·
stored procedures · wire security (TLS 1.3 + SCRAM-Argon2id + Ed25519,
tickets, audit-HMAC, RBAC, rate-limit) · duplex TCP / WS transports +
rid-demux + resume tickets · TypeScript client · ~2912 lib tests + property
tests + **29 benchmarks**.

**Open (charter pillar + usability + on-demand):** the **"I"**
(network changefeed → live subscriptions → replication → P2P) · browser
WASM client · QUIC / UDP / Unix transports · auth v1.1+ / PQ identity ·
vectors / FTS hardening · backup / restore tooling.

---

## 2. Forward order — Movement C is the live frontier

Movements A and B are fully done (§2.1). The foundation for Movement C is
fully laid. The remaining work is building the "I".

### Movement A — Consolidate (quality) · ✅ DONE
Adversarial 5-lens arc review; all findings remediated:
- **SEC** — 13 unauthorised admin / DDL ops gated; fail-closed
  `effective_fn_actor`; test-only bypass gated.
- **COR-1** — canonical_hash round-trip for Dec / Big.
- **MVCC-1** — non-tx writes join the SSI ledger.
- **CF-1 / CF-2** — durable-journal watermark + gap signal.
- Trust-boundary truncations.
- **H₂** Persistable trait + PersistRegistry.
- Miri-able framing tests.

### Movement B — Measure → Accelerate (perf) · ✅ DONE
- R / P shipped pre-arc.
- M1 (single-column columnar ORDER BY) + M2 (streaming JSON projection for
  SELECT *, 3.4×) + B1 overhead guard bench landed.
- **Opt O (covering index) — ✅ DONE** (see P3 below). DDL `include:[...]` +
  per-index meta; covered-projection write-side; versioned covering-posting
  envelope; RecordCell high-water mark (bump-first); validated covering
  index-only read reusing M2; A/B bench + Opt O verdict. Eliminates the
  N-random-read disk penalty for covered range queries.

### Movement C — Build the "I" (the live frontier)

The foundation is fully in place:
- changefeed event-version == `commit_version` — a replica can apply by version;
- `ChangesSince` one-shot temporal query — the pull precursor;
- durable journal + watermark + gap signal — resumability primitives;
- duplex transport + rid-demux + resume tickets — the push channel;
- subscriptions design doc (#201) written.

**Natural ladder:**

1. **Network changefeed (pull-API)** — wire-stream the journal over the
   existing `read_changelog_from`; `ChangesSince` is the one-shot precursor.
   Cheapest next step; no new subsystem.
2. **Live subscriptions / server-push (#201)** — design written; the duplex
   channel is the pipe. Hanging connections, filtered live-queries, resumable
   via `last_seen_version`, non-blocking fan-out, per-subscriber access
   filtering (events pass through `authorize_access`).
3. **Leader-follower replication** — a follower subscribes to the leader's
   changefeed and applies changes by `commit_version`. Write `REPLICATION.md`
   when this starts; don't author it ahead of need.
4. **P2P / gossip → chat** — the decentralised end-state of the name.
   *(Sharding is a separate, later direction — not a prerequisite.)*

### Parallel — on demand, not ahead of need
Browser WASM client ([`BROWSER_WASM_PLAN.md`](./BROWSER_WASM_PLAN.md)) ·
transports QUIC / UDP / Unix · auth v1.1+ & PQ identity
([`ROADMAP.md`](./ROADMAP.md)) · vectors / FTS hardening
([`EMBEDDINGS_AND_VECTORS.md`](./EMBEDDINGS_AND_VECTORS.md),
[`FULL_TEXT_SEARCH.md`](./FULL_TEXT_SEARCH.md)) · backup / restore tooling ·
non-blocking batched namespaced logging.

### Still deferred — on real need only
- **SDK function-packs (Stage D)** — multiple functions per wasm; only when
  libraries of related procedures actually appear. Default stays one fn/wasm.

---

## 2.1 Status of the movements

| Movement | Status | Notes |
|---|---|---|
| **A — Consolidate** | ✅ **DONE** | Adversarial 5-lens review; all 8 findings remediated (SEC-1/2/3, COR-1, MVCC-1, CF-1/2, truncations, H₂/gitignore) + Miri-able framing test. |
| **B — Measure/Accelerate** | ✅ **DONE** | R/P shipped pre-arc; M1/M2 + H₂ + overhead guards landed; Opt O (covering index) done — the last open perf item. |
| **C — the "I"** | **LIVE FRONTIER** | Foundation fully laid; design doc written (#201). Step 1 (network changefeed pull-API) is the cheapest entry point. |

---

## 2.2 Status of P1–P4 (the "solving the rest" wave)

All four items are resolved. The wave is closed.

### P1 — SSI-footprint overhead gate (#233) · ✅ DONE
Gated non-tx SSI footprint on active Serializable txns via
`active_serializable_count: AtomicU64` (++ when a Serializable snapshot
opens, −− in `SnapshotGuard` Drop). `record_nontx_ssi_footprint` early-
returns when the count is 0 (one relaxed load). B1 bench confirmed the
non-tx write baseline is restored when no Serializable tx is alive.

### P2 — MVCC-2 TOCTOU (#232) · ✅ RESOLVED BY CONSTRUCTION
**Not fixed via `set_versioned` always-bump** as originally planned.
The TOCTOU dissolved structurally: the temporal arc (T1a / #238) refactored
the MVCC store to a single append-only version-log (dual-write eliminated).
With one log, no read-modify-write interleaving is possible — the flaw is
architecturally impossible in the new design. The planned `set_versioned`
patch was made moot by a better design. Noted explicitly so the original
fix is not re-attempted. See [`TEMPORAL.md`](./TEMPORAL.md),
[`MVCC_CELL.md`](./MVCC_CELL.md).

### P3 — Covering index (Opt O / #218, #236) · ✅ DONE
DDL `include:[...]` + per-index meta; covered-projection write-side;
versioned covering-posting envelope; RecordCell high-water mark
(bump-first); validated covering index-only read reusing M2; A/B bench +
Opt O verdict. Eliminates the N-random-read disk penalty for covered range
queries. Write-amplification measured and reported alongside the read win.

### P4 — Level-3 pessimistic locking (#234, keystone #235) · ✅ BUILT
Originally "design now, build on real need" — it shipped. Wound-wait on the
monotonic version; deadlock-free by construction (the version is a total
order; no wait-cycle can form; no deadlock detector needed). Block-
conflictors-instead-of-aborting. The design was folded into
[`MVCC_CELL.md`](./MVCC_CELL.md) (the record cell unifies P2/P3.2/P4); the
implementation follows the wound-wait (Rosenkrantz et al.) protocol.

---

## 3. Resolved forks (decided this cycle)
- **Enforcement model:** *no global open/strict flag.* Permissions are
  per-resource (POSIX `mode`); default mode stays open (`0o777`), so
  enforcement is real but breaks nothing until a resource is `chmod`-ed.
  System bypasses as root.
- **DDL↔DML ordering:** *variant 1* — an op declares an explicit `after`
  dependency on another alias (no implicit auto-ordering of admin ops).
- **Query language is OQL — forever; no text language, no SQL, no "v2".**
  A query is a typed object (DTO: `Filter`/`ReadQuery`/`BatchRequest`),
  carried as msgpack/JSON, built by the typed builder / `q!` / `filter!`.
  This is *by principle*, not by lack of a parser. Queries-as-text is the
  single root mistake that spawns SQL injection, parser/grammar bugs &
  DoS, prepared-statement/bind ceremony, dialect drift, and parse/plan
  caches. OQL doesn't *solve* those — it makes them **structurally
  impossible**:
  - injection (CWE-89): data lives in `value` fields, never concatenated
    into a command string — there is no context where data becomes code;
  - "parsing" is total, deterministic msgpack deserialisation into typed
    structs — no grammar attack surface;
  - every query is already parameterised — no prepared statements;
  - the DTO **is** the wire **is** the AST — one representation, not three,
    so no re-parse cost to cache (this also retires PERF Opt N).
  OQL may *grow* (more operators, `$fn`, richer filters) — that is
  evolving the same object language, never a textual frontend. A textual
  "v2" would only parse back into these DTOs and would fracture the
  "one language, one builder, three callers" symmetry. **Do not build it.**

---

## 4. Discipline
*Don't over-build.* Pull each slice by real need, not ahead of it. Each
step taken the project way: research → implement → **zero-trust verify**
(diffs + an independent green gate, never agent claims) → separate clean
commit. Spiral docs (#124, Phase 4) remain deferred until requested.

---

## Open follow-ups
None blocking. The perf backlog lives in
[`PERF_OPPORTUNITIES.md`](./PERF_OPPORTUNITIES.md). The next actionable
item is Movement C step 1: network changefeed pull-API
(`read_changelog_from` over the wire). The subscriptions design
([`LIVE_SUBSCRIPTIONS.md`](./LIVE_SUBSCRIPTIONS.md)) and replication design
(`REPLICATION.md`, to be written when replication starts) are the two
pending design docs for Movement C steps 2 and 3.

### Bench-debt — WAVE CLOSED

The six-wave bench-coverage push (subscriptions hot paths + e2e
throughput + interactive-tx + journal-read + record-size axis + FTS
indexed + concurrent tx + subscription fan-out + durability axis +
SCRAM connect/resume + wound-wait) zeroed every gap whose dependency
already exists. 41 bench files at landing. Magistral focus returns to
Movement C — replication.

**Headline findings across the push:**

- Bridge de-intern fix necessary — discovered during e2e run, fixed,
  pinned by a RED→GREEN regression test (commits `0f1a645` + e2e).
- Interner is O(fields), NOT O(bytes): payload bulk is essentially
  free; field-key count is what costs (`record_size_axis` headline).
- Subscription fan-out is sub-linear: 100× more subscribers → only
  2.7× wall-clock per event (`subscription_fanout`).
- SSI aborts under Serializable scale honestly with N: 0.49 → 1.33
  → 3.01 aborts/commit at N=2/4/8 (`hot_key_serializable`).
- Snapshot isolation structurally cannot abort blind writes — the
  read-set is empty under Snapshot, validate_read_set is a no-op.
- Wound-wait path measurement required reading the **holder's**
  `wounded.load()` after the CS, NOT the contender's `lock_key`
  return value — the latter returns `Ok` to the loser AFTER the
  holder's tx is killed. With the right signal source: 56.8% wound
  rate at N=8 (`pess_lock_contended_in_cs_barrier`).
- Resume vs full SCRAM ≈ 7× (50 ms Argon2id-dominated vs 7 ms
  TLS+ticket-validate) — first measurement of the resume feature's
  claimed value (`wire_latencies::handshake_paths`).
- Synced vs Buffered durability cost: ~1.97× @ N=1 amortising to
  ~1.52× @ N=100 — fsync delta is ~10 ms regardless of batch size
  on redb, so larger batches make synced cheaper per row.

**Three items remain blocked on dependencies** — pick up only after
the dependency lands; not bench-debt, just future work tied to
larger milestones:

- **`reactive_call` delivery mode** (`subscription_delivery.rs`).
  Blocked on: a registered in-memory stored function (funclib / WASM
  module) reachable from the bench harness. `DeliverMode::Batch`
  already covers the shared `$event.*` injection and reactive wrapper
  hot path; `Call` adds only the `BatchOp::Call` swap.

- **HNSW insert / index-build cost.** `vector_search` benches the
  query side; build cost matters at the vectors/embeddings milestone
  ([`EMBEDDINGS_AND_VECTORS.md`](./EMBEDDINGS_AND_VECTORS.md)).

- **TS-client microbenches** (`SubscriptionRouter.route`, push-frame
  msgpack decode). Cheap operations; grows in value at the
  browser-WASM milestone
  ([`BROWSER_WASM_PLAN.md`](./BROWSER_WASM_PLAN.md)).

---

_Plan revision 2026-06-10 — Movements A and B fully done; P1–P4 wave
closed (P2 dissolved by construction via the temporal single-log refactor;
P4 shipped despite "deferred" label). §0.2 captures the 152-commit wave
since §0.1: temporal arc, nested batches, duplex transport, TS client,
internal refactoring. Movement C (the "I") is the single live frontier,
with its foundation fully laid and design doc written. Updated §2.1/§2.2
reflect truth; §3/§4 preserved._
