בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Forward Plan — Consolidate → Measure → Build the "I"

**Status:** living plan, revision **2026-06-05**. The single actionable
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

## 1. Maturity map

**Closed and solid (foundation + engine):**
storage (6 backends, one dumb-KV trait) · transactions (SI → SSI → true
serializability + interactive + crash recovery) · query engine
(WHERE/SELECT/GROUP BY/ORDER BY/pagination/cross-refs) · secondary /
sorted / functional / HNSW-vector / FTS indexes · WASM functions ("M") ·
access fabric (Shomer, now enforced) · DDL (now complete) · validators +
CAS · changefeed · wire security (TLS 1.3 + SCRAM-Argon2id + Ed25519,
tickets, audit-HMAC, RBAC, rate-limit) · TCP / WS transports · ~2667 lib
tests + property tests + **27 benchmarks**.

**Open (charter pillars + usability + perf-ceiling):** the **"I"**
(P2P / replication / chat) · query-language v2 · browser WASM client ·
QUIC/UDP/Unix transports · auth v1.1+ / PQ identity · vectors/FTS
hardening · backup tooling · the disk perf ceiling (covering index).

---

## 2. Forward order — three movements, then on-demand

Before laying the next floor (P2P), reinforce what was just built — "take
down the scaffolding." Cheap, removes regression risk, then accelerate,
then build the new pillar.

### Movement A — Consolidate (quality) · cheap, do first
- **Adversarial code-review of this cycle's 12 commits** (agent-built under
  a green gate, but never human-reviewed as a whole). Focus the hot/subtle
  spots:
  - changefeed concurrency on the commit-path (non-blocking guarantee,
    broadcast `Lagged` / journal-channel overflow behaviour);
  - CAS canonicalisation determinism (security-sensitive — byte-stable
    hash across key orderings);
  - the enforcement gate (no bypass path; every data + admin op gated);
  - the `set_versioned` always-bump invariant (#179 touched a deep MVCC
    primitive — confirm SI/SSI unaffected; tests pass, eyes still wanted).
- **`H₂` — `Persistable` trait + registry** (PERF §Opt H₂): end the
  write-amplification recurrence pattern (interner + counter fixed twice
  by hand). ~1 day.
- **Housekeeping debt:** `.gitignore` (`server-cert.pem`,
  `crates/shamir-client-node/target/`); nightly `cargo-fuzz` on the
  version codec (proptest covers the bulk).

### Movement B — Measure → Accelerate (perf)
- **Bench the new features first** — there are *no* measurements yet of the
  overhead we added: changefeed emission on the commit-path, `authorize_access`
  per DML op, the validator pass, CAS `canonical_hash`. Prove we did **not**
  slow the hot paths before building further. (New benches alongside the
  existing 27; reuse the `/opti` before→after discipline.)
- **Sprint γ (the biggest untapped win)** — see PERF_OPPORTUNITIES:
  - `Opt R` — reverse iteration on `Store` (unblocks MAX / `ORDER BY DESC
    LIMIT`).
  - `Opt O` — **covering index** — eliminates the N-random-read penalty on
    disk range queries (**100–1000× on disk**; the path to Postgres-class
    range latency). The single most impactful item left.
  - `Opt P` — vectored `Store::get_many` for non-covered queries.
  - Then `M1` (ORDER BY single-column columnar, ~37→10ms) and `M2`
    (streaming JSON serializer, −30% on the SELECT wire path) — already
    profiled and bench-fixtured.

### Movement C — Build the "I" (the open charter pillar)
The changefeed (3b) is the foundation; #179 aligned event version with the
data version exactly so a replica can apply by version. Natural ladder:
1. **Network changefeed (pull-API)** — a wire request `changefeed from
   version V` over the existing journal (`read_changelog_from`). Resumable
   cursor; the deferred Variant-2 of 3b.
2. **Live business subscriptions (server-push)** — *design task, see below.*
   Long-lived client connections that **hang waiting** for "new data /
   updated data" notifications — the push-to-client application layer on
   top of (1). The client-facing consumer of the changefeed.
3. **Leader-follower replication** — a follower subscribes to the leader's
   changefeed and applies changes by `commit_version`.
4. **P2P / gossip → chat** — the decentralised end-state of the name.
   *(Sharding is explicitly a separate, later direction — not a prerequisite
   for replication.)*

#### Task — design "hanging" business subscriptions (live-queries / push)
**Status: PROPOSED — design first, no code yet.** A business client opens a
connection and *waits* to be told when data it cares about appears or
changes (the inverse of polling). This is the user-visible payoff of the
whole changefeed arc. Open questions to settle in a `SUBSCRIPTIONS.md`
design doc before building:
- **Subscription granularity** — whole-repo / per-table / per-query
  (filtered live-query, "tell me about rows matching WHERE …") / per-record.
  A filtered live-query is the valuable-but-hard end (needs predicate
  evaluation on each emitted event).
- **Transport / protocol** — over which channel does the push flow? WS is
  the natural fit (already a transport); TCP needs a server→client frame.
  A wire op like `subscribe(filter, from_version)` → a stream of events.
- **Resumability** — client reconnects with `last_seen_version`; bridge the
  live broadcast + the durable journal (exactly the 3b hybrid) so a slow /
  disconnected client misses nothing (`read_changelog_from`).
- **Backpressure & fan-out** — many hanging clients, one commit-path. MUST
  stay non-blocking (`try_send` + per-subscriber journal cursor, never block
  the writer — concurrency invariants). What happens to a `Lagged` slow
  consumer (drop → resync from journal vs disconnect).
- **Access control** — a subscriber only sees events for resources it may
  read; the changefeed event must be filtered through `authorize_access`
  per subscriber (don't leak governed data via the push channel).
- **Initial-state semantics** — "snapshot + then live" (current matching
  rows, then the delta stream) vs "deltas only from now". Consistent cutoff
  via `commit_version` (the snapshot read version == the stream start).
- **Lifecycle** — unsubscribe, connection drop cleanup, server-side resource
  cap (max subscriptions / client), idle timeout.
> Depends on (1) network changefeed landing first. Write `SUBSCRIPTIONS.md`
> when this task starts; don't author ahead of need.
> A dedicated `REPLICATION.md` design doc is **PROPOSED** — to be written
> when Movement C starts (don't author it ahead of need).

### Parallel — on demand, not ahead of need
Query-language v2 (SQL/OQL frontend over the finished engine) · browser
WASM client ([`BROWSER_WASM_PLAN.md`](./BROWSER_WASM_PLAN.md)) · transports
QUIC/UDP/Unix · auth v1.1+ & PQ identity ([`ROADMAP.md`](./ROADMAP.md)) ·
vectors/FTS hardening ([`EMBEDDINGS_AND_VECTORS.md`](./EMBEDDINGS_AND_VECTORS.md),
[`FULL_TEXT_SEARCH.md`](./FULL_TEXT_SEARCH.md)) · backup/restore tooling.

### Done since (was "deferred — expensive chains")
- **SDK builder-in-guest (Stage B2)** — ✅ **DONE** (§0.1). The "costly
  chain" dissolved: contemplation found P4's premise false (`query-types`
  has no `QueryValue` — DTOs use `serde_json::Value` + self-contained
  `FilterValue`). The **thin-waist** (a `shamir-collections` leaf + a
  `server` feature-gate) made the builder guest-lean with **no `Value`
  refactor**; B2 was then a small `db_execute` shim + SDK feature. See
  `SDK_AUTHORING.md` Stage B.
- **WASM `shamir-value` ABI crate (P4)** — ⊘ **RETIRED** (premise false;
  not a weight win — see `WASM_SLIMMING.md` Phase 4 banner).

### Still deferred — on real need only
- **SDK function-packs (Stage D)** — multiple functions per wasm; only when
  libraries of related procedures actually appear. Default stays one fn/wasm.

## 2.1 Status of the movements
Movements A/B/C below were the **original** forward order; the recent
stored-proc / WASM / SDK tracks (§0.1) ran first by request. A/B/C are
**still ahead** and unstarted:
- **A (consolidate)** — adversarial review of the arc + `H₂` + gitignore/
  fuzz debt. Cheap; recommended next.
- **B (perf)** — bench the new features' overhead, then sprint γ (covering
  index `Opt O`, `R`, `P`, `M1`, `M2`).
- **C (the "I")** — network changefeed → **live business subscriptions
  (server-push, design task)** → leader-follower replication → P2P.

---

## 3. Resolved forks (decided this cycle)
- **Enforcement model:** *no global open/strict flag.* Permissions are
  per-resource (POSIX `mode`); default mode stays open (`0o777`), so
  enforcement is real but breaks nothing until a resource is `chmod`-ed.
  System bypasses as root.
- **DDL↔DML ordering:** *variant 1* — an op declares an explicit `after`
  dependency on another alias (no implicit auto-ordering of admin ops).

---

## 4. Discipline
*Don't over-build.* Pull each slice by real need, not ahead of it. Each
step taken the project way: research → implement → **zero-trust verify**
(diffs + an independent green gate, never agent claims) → separate clean
commit. Spiral docs (#124, Phase 4) remain deferred until requested.

---

## Open follow-ups
None blocking — #177 / #178 / #179 closed this cycle. The perf backlog
lives in [`PERF_OPPORTUNITIES.md`](./PERF_OPPORTUNITIES.md); two known
quality items (changefeed network slice, `REPLICATION.md` design) are
folded into Movements B/C above.

---

_Plan revision 2026-06-05 — after the DDL → access → write-lifecycle arc
(`016d68b`..`a620115`) plus the stored-procedures + WASM-perf + WASM/SDK-
slimming tracks (`66e09a0`..`c0c27fe`, §0.1) and the thin-waist +
builder-in-guest B2 (`52be3b3`..`ca79b1f`; P4 retired). Next: Movement A
(consolidate) recommended. Updated as movements land._
