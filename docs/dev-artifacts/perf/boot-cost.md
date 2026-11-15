# Boot cost: what does opening the stores cost at start-up? (BOOT-COST)

Investigation record. No production change. Brief:
`docs/dev-artifacts/prompts/perf/15-boot-cost.md`.

Every claim is labelled OBSERVED (read from the code in this worktree, file
and line given) or INFERRED (reasoning about OS behaviour; not measured
here — measurement runs in CI, see §7).

## 1. How many keyspace opens and how many `persist(SyncAll)` per open?

### 1.1 What ONE fjall open consists of

- `FjallRepo::new` → `Database::builder(path).open()` — OBSERVED
  `crates/storage/shamir-storage/src/stores/storage_fjall/repo.rs:125-130`.
  One fjall Database per repo directory. `open()` recovers the database
  (manifest read + journal replay) — the cost lives inside fjall.
- Every store (keyspace) open goes through `Repo::store_get` →
  `db.keyspace(name, KeyspaceCreateOptions::default)` inside
  `spawn_blocking` — OBSERVED
  `crates/storage/shamir-storage/src/stores/storage_fjall/repo.rs:135-154`.

### 1.2 Keyspace opens per system-store open (`ShamirDb::init(SystemStoreConfig::Fjall(path))`)

Per-REPO keyspaces opened eagerly during open (all via `store_get`):

| Keyspace | Opened by | Site |
|---|---|---|
| `__tx__` | tx-gate seed | `crates/engine/shamir-engine/src/repo/instance/repo_instance/transactions.rs:56` |
| `__changelog__` | journal / clean-horizon | `crates/engine/shamir-engine/src/repo/instance/repo_instance/lifecycle/journal.rs:99`, `clean_horizon_repo_impl.rs:17` |
| `__interner__` | repo interner | `crates/engine/shamir-engine/src/repo/instance/repo_instance/lifecycle.rs:286`, `seed_metadata_impl.rs:13` |
| `__seed_schema__` | seed catalogue | `crates/engine/shamir-engine/src/repo/instance/repo_instance/lifecycle/seed_backing_impl.rs:8,23` |

Per-TABLE keyspaces: each table materialises lazily on first
`get_table` → `create_table_context`, which opens THREE stores —
`__data__<t>`, `__info__<t>`, `__history__<t>` — OBSERVED
`crates/engine/shamir-engine/src/repo/instance/repo_instance/catalog/repo_membership_impl.rs:229-232`
and `crates/engine/shamir-engine/src/table/support/model/names.rs:18-24`.
Tables NOT touched stay closed (`from_box_repo_with_wal_dir` only
registers configs/tokens; `crates/engine/shamir-engine/src/repo/instance/repo_instance.rs:88-108`).

The boot path then touches four catalogue tables, each opening its three
stores (12 keyspace opens):

| Table | Boot reader | Site |
|---|---|---|
| `databases` | `load_databases` | `crates/engine/shamir-db/src/shamir_db/shamir_db/core.rs:398` |
| `repositories` | `load_repositories` | `core.rs:408` |
| `tables` | `load_tables` | `core.rs:413` |
| `groups` | `rebuild_group_names` (end of `SystemStore::init_with_durability`) | `crates/engine/shamir-db/src/shamir_db/system_store/store.rs:212,385-412` |

The system repo is CONFIGURED with 13 catalogue tables
(`store.rs:171-184`), but only the four read at boot open at open time.
Total for one system-store open on an existing database: 1 Database open
+ 4 repo-level keyspace opens + 12 catalogue-table keyspace opens
≈ **16 keyspace opens, 0 `persist(SyncAll)`**.

On a FRESH directory the DDL layer additionally creates each catalogue
table; catalogue writes route through `flush_system_repo_durable` =
`sync_wal()` + `Store::flush()` — one `persist(SyncAll)` per durable DDL
acknowledgement — OBSERVED
`crates/engine/shamir-db/src/shamir_db/system_store/store.rs:436-440` and
`crates/storage/shamir-storage/src/stores/storage_fjall/store.rs:289-297`.
A fresh open is therefore much more expensive than a reopen; the CI
fixtures measure mostly FRESH opens (fresh temp dir per test).

### 1.3 One server boot (all stores)

Boot sequence in `crates/server/shamir-server/src/engine/server/server_launcher/launch.rs`
(all awaited inline, strictly sequential):

| # | Open | Backend | Site |
|---|---|---|---|
| 0 | `.shamir.lock` advisory file lock | OS file lock | `launch.rs:118` |
| 1 | `server_meta` (ONE keyspace `server_meta_v1`) | fjall Database | `launch.rs:121`; `crates/server/shamir-server/src/engine/server_meta.rs:227-229` (NOTE: earlier docs said redb; the code says fjall) |
| 2 | `FjallUserDirectory` (users) | fjall Database | `launch.rs:126` |
| 3 | `FjallConsumedCounters` | fjall Database | `launch.rs:130` |
| 4 | `FjallAuditAppender` | fjall Database | `launch.rs:139` |
| 5 | `ShamirDb` system store | fjall Database + ~16 keyspace opens (§1.2), plus re-attach of EVERY persisted repo (`core.rs:398-470` → `add_repo_with_durability`, each = one more `Database::open` + its recovery) | `launch.rs:389` |
| 6 | `default.main` repo | fjall Database (via `BoxRepoFactory::fjall`, `launch.rs:429-433`) | its own Database open + recovery + per-table stores on first touch |
| 7 | `TablesRegistry` | JSON file with atomic-rename persist (NOT a KV store) | `launch.rs:442`; `crates/server/shamir-server/src/engine/tables_registry.rs:115-170` |

So "7 stores" = 6 fjall Databases + 1 JSON file. Keyspace opens at boot:
≈16 (system repo) + 4 per `default.main` user table + ≥1 per auxiliary
fjall Database — order of ~25-40 `db.keyspace()` calls, plus 6 full
Database recoveries.

`persist(SyncAll)` during a QUIET boot: OBSERVED zero from the storage
layer — the count seam
(`crates/storage/shamir-storage/src/stores/storage_fjall/persist.rs:9-16,25-28`)
is bumped only by `Store::flush()`, `Repo::persist_once()` and
atomic-batch commit; none of these run on the open path (§4). Server
boot DOES SyncAll via `ServerMetaStore::persist()` on its setters
(`server_meta.rs:365-366`) and via catalogue DDL writes if tables are
(seeded|altered) at boot (`store.rs:436-440`).

## 2. Why expensive on Windows, cheap on Linux

What the OS call is, per step (INFERRED from what the code calls; the
per-call timing itself is not measured here — §7):

- **Database/keyspace open** — many small file metadata operations:
  open/read/verify the manifest, open the per-keyspace journal partition,
  create directories for a fresh keyspace. On Windows every directory /
  file creation and metadata update goes through `CreateFile` /
  `SetFileInformationByHandle` with NTFS metadata journaling, and
  directory-change metadata is not coalesced the way Linux ext4/overlayfs
  journals it; each op costs ~ms on a cloud-runner disk, ~µs on Linux
  (page-cache metadata). With ~25-40 keyspace opens × several metadata
  ops each, this dominates. INFERRED.
- **`persist(SyncAll)`** — `FlushFileBuffers` on Windows vs `fsync`/`fdatasync`
  on Linux. Same primitive class; measured CI history shows the Windows
  runner charges ~100-300 ms per sync point vs Linux ~sub-ms — OBSERVED
  numbers come from `docs/dev-artifacts/perf/win-test-runtime.md`
  (per-test constant ~6.4 s vs 0.32 s; serialisation did NOT change the
  per-test cost, so it is fixed per-open-cycle storage work, not
  contention).
- **Lock release + temp-dir delete after close** — fjall releases the
  directory lock lazily on Drop (reopen tests poll up to 100 × 50 ms) and
  recursive delete of a freshly-fsynced tree is metadata-heavy. OBSERVED
  mechanism described in `win-test-runtime.md` §"Mechanism".

Ranked hypotheses by evidence strength:

1. **Per-metadata-op cost (opens, creates, deletes), not fsync count** —
   strong: the measured cost is constant per open cycle, unchanged by
   parallelism (`win-test-runtime.md`), and the open path issues zero
   SyncAll on a quiet boot (OBSERVED, §1.3).
2. **`FlushFileBuffers` cost when DDL runs at boot (fresh stores)** —
   medium: the code path exists (§1.2), and fresh-store fixtures dominate
   CI fixtures; each catalogue-table DDL does a SyncAll.
3. **Antivirus / filter-driver scanning on the Windows runner** — weak:
   consistent with the ~20× gap and cloud-runner specifics, but no
   measurement separates it from plain NTFS cost.

## 3. Which opens are sequential and could run concurrently?

OBSERVED: every boot open is awaited inline in one `boot` function
(`launch.rs:118` → `:442`); nothing overlaps.

Ordering rules that actually constrain:

- The data-dir lock (#0) must precede ALL of them — the guard blocks a
  second server on the same `data_dir` (`launch.rs:111-118`).
- `server_meta` (#1) must precede lockout/rate-limit rehydration, which
  reads it right after (`launch.rs:266-349`).
- The audit appender (#4) must exist before the first audit event.
- `ShamirDb` (#5) must exist before `default.main` is registered into it
  (`launch.rs:429-433`) — same `DbInstance`, sequential by construction.

Concurrency-safe grouping (no durability or ordering rule weakened):
`{users, counters, audit, server_meta}` run concurrently after the lock;
`TablesRegistry` concurrently with them. `ShamirDb::init` can start in
parallel with that group too (independent Database) as long as
`default.main`'s `add_repo` stays ordered after it. The system-store
internal open sequence (recovery → certification) stays sequential — it
is the crash-recovery protocol.

## 4. Is a `SyncAll` every 500 ms needed on an idle store?

OBSERVED: NO SyncAll happens when idle. The background flusher wakes
every `flush_interval_ms` (default 500, `storage_membuffer/core.rs:55-62`),
but `drain_once_owned` returns immediately when `dirty_count == 0`
(`crates/storage/shamir-storage/src/stores/storage_membuffer/core.rs:660-668`)
— the only extra work on an idle tick is moka's `run_pending_tasks()`
(`core.rs:472`). SyncAll is issued ONLY by: `Store::flush()`
(`storage_fjall/store.rs:289-297`), `Repo::persist_once()`
(`storage_fjall/repo.rs:207-214`), and atomic-batch commit
(`storage_fjall/repo.rs:116-121`), all through the single seam
`persist_sync_all` (`storage_fjall/persist.rs:25-28`).

This CORRECTS the earlier note in `win-test-runtime.md` §Mechanism item 3
("the background flusher persists the backend … every 500 ms"): in the
current code an idle MemBuffer store issues no persist at all. The drain
itself (dirty entries → `inner.set_many`) writes to the fjall journal
without fsync; the per-500 ms cadence bounds ACK→durable lag only for
actively-dirty stores, which is the intended durability contract
(`storage_membuffer/mod.rs`, "Durability contract" section).

## 5. Options, with risk and guarding invariants

| Option | Expected win | Risk | Guarding invariant |
|---|---|---|---|
| **Lazy catalogue tables** (only open a table's 3 stores on first real access) | Already partially true (§1.2); the remaining cost is the 4 boot-read tables × 3 stores + 4 repo-level stores ≈ 16 opens. Collapsing to fewer opens saves ~12 metadata-heavy opens | Later first-query latency; error surface moves from boot to request time (a broken keyspace fails a request, not the boot) | Boot still opens `__tx__`/`__changelog__`/`__interner__`/`__seed_schema__` — recovery/certification unchanged; durability never depends on a closed keyspace (fjall journals per Database, not per keyspace) |
| **One keyspace with key prefixes** (per-table prefix instead of per-table keyspace) | Removes the N×3 keyspace opens entirely; one keyspace per repo | BIG: `scan_prefix_stream` semantics stay, but per-table store deletion/rename (drop/rename machinery, `catalog.rs:229-235`, `table_store/copy.rs`) becomes a prefix rewrite; atomic batch across "tables" becomes trivial; id/collision risk if a table name is a prefix of another (need length- or delimiter-framed keys — a wire-format change for `__data__`/`__info__` stores) | Durability is per-Database already (single journal); the invariant to re-prove is ISOLATION between tables (scan_prefix must never leak across a boundary) and DDL drop atomicity |
| **Parallel open** (§3 grouping) | Up to ~4-5× on the boot wall-clock if opens are CPU/disk-latency bound | Moderate: error aggregation, boot failure ordering; fjall `Database::open` on the same disk competes for I/O queue depth (parallelism helps on NTFS metadata ops, does not help on a single spinning disk) | No durability change at all — each open is independent; only the data-dir lock ordering (#0 before all) and `ShamirDb` before `add_repo default.main` must hold |
| **Skip idle persist** | None — already the behaviour (§4). Documentation fix only (correct `win-test-runtime.md`) | — | — |
| (for completeness) **Open the system store with fewer catalogue reads at boot**: batch `load_databases`/`load_repositories`/`load_tables`/`load_groups` into one pass over fewer keyspaces | Saves up to 3 keyspace opens if tables were merged; marginal | Catalogue layout is a format contract (`MetaEnvelope`, catalogue records) | Catalogue durability: every write already routes through `flush_system_repo_durable`; a read-side change cannot weaken it |

Recommendation (proposal, five lines):
1. Measure first in CI (§7): confirm the split between Database-open
   cost, keyspace-open cost, and SyncAll cost on windows-latest.
2. Parallelise the independent boot opens (§3) — cheapest, no durability
   surface touched.
3. Keep per-table keyspaces; widen lazy opening to whatever boot
   measurement shows is unused on a quiet boot.
4. Keep the single `persist_sync_all` seam; do not add idle persists.
5. Correct `win-test-runtime.md` §Mechanism item 3 to match §4 above.

## 6. Counters and measurement plan (runs in CI, nothing local)

Counters that already exist:

- `FJALL_PERSIST_SYNC_ALL_COUNT` +
  `reset_persist_sync_all_count()` — OBSERVED
  `crates/storage/shamir-storage/src/stores/storage_fjall/persist.rs:9-16`,
  re-exported at `crates/storage/shamir-storage/src/stores/storage_fjall/mod.rs`
  (`pub use persist::{reset_persist_sync_all_count, FJALL_PERSIST_SYNC_ALL_COUNT}`).
  Counts every `Database::persist(PersistMode::SyncAll)`.
- Timing marks: none exist for open yet; the measurement test below adds
  stdout timings (FlakeGuard boot-stage marks already exist in the three
  shared server fixtures, printing only on failure — `win-test-runtime.md`).

Measurement test added (plain test, prints to stdout so the CI log shows
the numbers): `crates/storage/shamir-storage/tests/boot_cost_measurement.rs`.

It measures, on a fresh temp dir, with the real fjall backend:

1. one `FjallRepo::new` Database open (wall time);
2. 16 sequential `store_get` keyspace opens, individually timed (the
   §1.2 shape: 4 repo-level + 12 catalogue-table stores);
3. `persist(SyncAll)` per-call wall time (via `Store::flush`);
4. an idle MemBuffer-wrapped store held for 1.6 s: asserts the persist
   counter does not move (the §4 claim, executable);
5. close (Drop) wall time.

Run it as part of the normal lib-test tier so BOTH OS matrices execute
it:

```
./scripts/test.sh -p shamir-storage -- boot_cost
```

or in CI the crate is already selected when `shamir-storage` changes;
for a one-off both-OS run: `gh workflow run ci.yml --ref <branch>` with
`crates=shamir-storage` (full tier, windows + ubuntu), then grep the
junit/log output for `BOOT-COST` lines. Expected observation:
`db_open`, `keyspace_open` and `persist` lines are ~20× larger on
windows-latest than ubuntu-latest; `idle_ticks=... persist_count_after_idle=0`.

What must be observed in CI for this document's claims:

- `persist_count_after_open=0` on a quiet reopen (claim §1.3).
- `persist_count_after_idle=0` after ≥3 flusher ticks (claim §4).
- per-keyspace-open wall time roughly constant and dominating a quiet
  reopen (hypothesis §2.1).

## 7. Measured on CI (one run, `workflow_dispatch` of `shamir-storage`, run 43725029429)

The measurement test was run once on the three runner families with a deliberate final failure so that the log shows the
output of the test. One run per OS: the numbers are indicative, not statistics (the outliers below are real).

| Step | ubuntu-latest | macos-latest | windows-latest |
|---|---|---|---|
| `FjallRepo::new` (Database open) | 3 ms | 16 ms | 20 ms |
| 16 sequential keyspace opens, sum | 43 ms | 411 ms | 1217 ms |
| keyspace open, typical single | 2-4 ms | 14-51 ms | 26-74 ms (outliers 154 ms and 446 ms) |
| `persist` count after the quiet open | 0 | 0 | 0 |
| `persist` count after 1.6 s idle (7+ flusher ticks) | 0 | 0 | 0 |
| `flush()` with nothing dirty (one SyncAll) | 0 ms | 1 ms | 0 ms |
| close and directory removal | 6 ms | 13 ms | 14 ms |

What this settles:

- Section 4 holds as an executable fact on all three OS: a quiet open and an idle store issue NO `persist(SyncAll)`; the
  500 ms flusher does nothing while `dirty_count == 0`. The earlier "SyncAll every 500 ms on an idle store" is false.
- A keyspace open is about 17 times slower on Windows than on Linux (median 44 ms against 2.5 ms); the 16 opens of one
  system-store open cost about 1.2 s on Windows against 43 ms on Linux.
- This accounts for about 1.2 s of the roughly 6.4 s that `win-test-runtime.md` attributes to one system-store open on
  Windows. About 5 s lie elsewhere in the system-store open path: not in the raw Fjall open, not in an idle flusher, and
  not in a SyncAll of an empty store (all measured near zero here). The remaining candidates are the work done while the
  catalogue is created (the DDL path persists with SyncAll per statement on a fresh store, section 2), and the
  construction around the store. A second measurement is needed before any fix is chosen: time marks inside
  `ShamirDb`'s system-store open, over the real code path, on `windows-latest`.
- Consequence for the options of section 5: lazily opening the catalogue tables or one keyspace with prefixes can save at
  most about 1.2 s per open on Windows, not the whole 6.4 s; parallel open of the 7 stores helps a server start but not a
  single system-store open. Do not pick an option before the second measurement.
