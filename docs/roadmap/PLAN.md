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
2. **Leader-follower replication** — a follower subscribes to the leader's
   changefeed and applies changes by `commit_version`.
3. **P2P / gossip → chat** — the decentralised end-state of the name.
   *(Sharding is explicitly a separate, later direction — not a prerequisite
   for replication.)*
> A dedicated `REPLICATION.md` design doc is **PROPOSED** — to be written
> when Movement C starts (don't author it ahead of need).

### Parallel — on demand, not ahead of need
Query-language v2 (SQL/OQL frontend over the finished engine) · browser
WASM client ([`BROWSER_WASM_PLAN.md`](./BROWSER_WASM_PLAN.md)) · transports
QUIC/UDP/Unix · auth v1.1+ & PQ identity ([`ROADMAP.md`](./ROADMAP.md)) ·
vectors/FTS hardening ([`EMBEDDINGS_AND_VECTORS.md`](./EMBEDDINGS_AND_VECTORS.md),
[`FULL_TEXT_SEARCH.md`](./FULL_TEXT_SEARCH.md)) · backup/restore tooling.

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
(`016d68b`..`a620115`). Updated as movements land._
