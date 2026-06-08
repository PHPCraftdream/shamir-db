בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# TUNABLES — magic-constant audit + centralization plan (M0)

Read-only audit of magic numbers in **production** code (inline
`#[cfg(test)]` modules filtered out), classified by **meaning** (not by
value — the same literal can be several distinct knobs), with each knob's
**owner level** for the eventual cascade (Instance → Repo → Table → Store).

> **Headline finding.** The problem is smaller than it looked. **Timeouts are
> mostly already named** (consts) or already live in config structs. The real
> raw cluster is **scan/iter batch sizes** (~25 sites) — and their values are
> *inconsistent* (a prefix-scan is `256` in some places, `1000` in others).
> Centralizing forces one principled choice and removes the inconsistency.

Tiers: **A** = invariant (never runtime; named const, leave). **B** =
build-time tunable (named const in `tunables`, `/opti`-benchmarked, rebuild to
change). **C** = runtime/deployment knob (eventually a config field with a
cascade; the const is its default).

---

## 1. Tier B — EXTRACT to `tunables` (the real work)

### 1a. Scan / iter batch sizes — the dominant raw cluster
Values today are inconsistent; centralization picks one per **purpose**.

**`FULL_SCAN_BATCH` (= 1000)** — full-table reads / index backfill:
- `shamir-engine/index/index_manager.rs:332` (`data_store.iter_stream` — index backfill adapter)
- `shamir-engine/index/index_manager.rs:1264` (unique-index backfill adapter)
- `shamir-engine/index/index_manager.rs:1390` (`info_store.scan_prefix_stream(prefix, 1000)` — index save scan) ← *inconsistent: a prefix scan at 1000*
- `shamir-engine/table/doctor.rs:100, 225, 239, 411`
- `shamir-engine/table/read_exec.rs:697, 831` (AsOf / History full-scan)
- `shamir-storage/storage_cached.rs:56, 118` (cached-store iter)

**`MAINT_SCAN_BATCH` (= 256)** — background maintenance / prefix scans / migration:
- `shamir-tx/mvcc_store.rs:391` (`history.scan_prefix_stream` — vacuum_key)
- `shamir-tx/mvcc_store.rs:1122` (gc_below), `:1208` (purge_below_ts), `:1399` (scan_history_for_version)
- `shamir-engine/index/sorted_index_manager.rs:233` (`info_store.scan_prefix_stream`)
- `shamir-engine/table/interner_manager.rs:164` (`info_store.scan_prefix_stream`)
- `shamir-engine/migration/coordinator.rs:157, 171, 325, 332, 340` (snapshot/drain/verify)
- `shamir-engine/migration/shadow_log.rs:48, 107, 127` (shadow-log scans)

> **Decision needed (M1):** keep two knobs (`FULL_SCAN_BATCH`=1000 /
> `MAINT_SCAN_BATCH`=256) and normalize the outliers (index_manager:1390
> prefix-scan → which?), OR split finer by access pattern
> (`ITER_BATCH` / `PREFIX_SCAN_BATCH` / `MIGRATION_BATCH`). Recommend the
> two-knob form for simplicity; normalize 1390 to `MAINT_SCAN_BATCH` (it is a
> prefix scan like the others).

Owner level: **Store** (backend-dependent optimum); default seeded at Instance.
24 call sites already take `batch_size` as a parameter — those are the seam;
the literal lives at the top caller and is covered above.

### 1b. Frame/IO buffer capacity
**`IO_FRAME_BUFFER_CAP` (= 4096)**:
- `shamir-transport-*/connection.rs:197` (`frame_buf`), `:198` (`write_scratch`)
- (`framing.rs:82/179/180` are **doc-comment** examples — leave.)

Owner level: **Instance** (transport). Tier B.

### 1c. Already-named build-time const to relocate (optional, low value)
- `MATERIALIZE_ATTEMPTS` (`shamir-engine/tx/commit.rs`, ×6) — already named;
  move into `tunables::instance_defaults` for one-place. Tier B.

---

## 2. Tier C — runtime knobs, but MOSTLY ALREADY MANAGED (leave / later)

These are **already named consts or config-struct fields** — Tier C is
largely *done*. Do NOT re-extract; at most relocate names later for one-place,
or promote to the cascade when a deployment genuinely needs to vary them.

- `DEFAULT_MAX_TX_LIFETIME` = 300s — `shamir-engine/tx/commit.rs:139` (const)
- `INTERACTIVE_TX_MAX_LIFETIME` = 300s — `shamir-server/db_handler.rs:118` (const)
- `DEFAULT_INTERACTIVE_TX_IDLE_TTL`, `DEFAULT_REAPER_INTERVAL` = 5s — `shamir-server/tx_registry.rs:39,44` (const)
- `LOCKOUT_SNAPSHOT_INTERVAL` = 60s — `shamir-server/server.rs:1108` (const)
- `SHUTDOWN_DEADLINE` — `shamir-server/runtime.rs:14` (const)
- `SchedulerConfig` fields = 60s — `shamir-server/scheduler.rs:50,53,54`
  (`counter_gc_period` / `session_gc_period` / `audit_checkpoint_period`) —
  **already a config struct with defaults.** This is the Tier-C/cascade
  precedent.

### Raw timeout stragglers (small — name in M2)
- `SERVER_POLL_INTERVAL` (= 50ms) — `shamir-server/server.rs:873, 935, 1004`
  (`tokio::time::sleep` poll loops). Tier C, Instance.
- a few bare `from_secs(5)` / `from_secs(30)` — `server.rs:298`,
  `windows_service.rs:123` — name on sight in M2.

> **Audit caveat (heuristic).** The test-filter excludes everything after a
> file's first `#[cfg(test)]`. Files with an early `#[cfg(test)]` helper
> (e.g. `brute_force.rs`, `vector_backend.rs` — each has `from_millis(50)`
> backoff/poll sleeps) may be **false-negatives** (real prod code filtered
> out). M1/M2 prompts must re-confirm those specific sites by reading, not
> trust the filter blindly.

---

## 3. Tier A — invariants (LEAVE; already named)
`VERSION_SEP` (0xFF), `TS_TAG` (0x00), RecordId = 16 bytes, KDF minimums
(`KDF_MIN_*`, `ARGON*`), wire/query-lang versions, `MAX_TOPK`,
`DEFAULT_CHANNEL_CAPACITY`. Compile-time laws; never runtime. No action.

`with_capacity(N)` with `N` derived from input (`items.len()`, small fixed
sizes) — local allocation hints, **not config**. Out of scope.

---

## 4. Plan (revised by the audit — smaller than the first sketch)

### Phase 1 — centralize (build-time const, behaviour-identical)
- **M1a** — create leaf crate `shamir-tunables` (zero-dep), modules by owner
  level: `store_defaults` (`FULL_SCAN_BATCH`, `MAINT_SCAN_BATCH`),
  `instance_defaults` (`IO_FRAME_BUFFER_CAP`, `MATERIALIZE_ATTEMPTS`,
  `SERVER_POLL_INTERVAL`). Wire as dep to storage/tx/engine/server. Replace
  the `shamir-storage` scan literals. Update `Cargo.toml` members + CLAUDE.md
  (18 → 19 crates). Normalize the value outliers per §1a decision.
- **M1b** ∥ **M1c** — replace scan literals in `shamir-engine` (M1b) and
  `shamir-tx` (M1c) with the named consts (disjoint crates, parallel).
- **M2** — frame buffer + server poll + bare-timeout stragglers → consts
  (re-confirm brute_force/vector 50ms by reading per the caveat).
- **M3** — finalize this doc as the index: knob → level → default → how to
  tune (now: edit const + rebuild + `/opti`; later: runtime cascade).

### Phase 2 — cascade engine (runtime; separate, designed under /oxx)
`Layer{Instance,Repo,Table,Store}` sparse `Option` fields +
`resolve(knob) = store ?? table ?? repo ?? instance ?? DEFAULT`, lock-free
`ArcSwap` per layer (as retention), resolve once per operation. The Phase-1
const becomes each promoted knob's `Default`. Promote per genuine need only
(§0: defaults work untouched). `SchedulerConfig` is the existing precedent.

---

## 5. Orchestration discipline
M0 (this doc) front-loads all discovery → @asm steps are pure mechanical
replacement by the site lists above; prompts forbid searching/expanding scope
and carry the "same value ≠ same knob" + test-filter-caveat rules. Zero-trust:
read the full diff, grep-verify new const == old literal, re-run the gate by
hand, then commit. Order: M1a → (M1b ∥ M1c) → M2 → M3 → [Phase 2].

_Audit performed read-only; counts are production-only (test-filtered, with the
§2 heuristic caveat). Nothing committed by this audit._
