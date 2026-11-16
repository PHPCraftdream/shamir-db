# Online Backup: Consistent Copy Without Stopping the Server

- Status: design (release-1 direction R2a)
- Brief: `docs/dev-artifacts/prompts/release/12-r2a-online-backup-design.md`
- Plan: `docs/dev-artifacts/release-1/README.md` (wave 2), `docs/dev-artifacts/release-1/02-online-backup-and-encryption.md` (sections 1, 2; its section 6 defaults are the release plan's — the Q1-Q8 below are defaults chosen here; release-plan section 6 items cross-checked in the Review)
- Related: `docs/dev-artifacts/design/format-stamp.md` (sections 3.5, 5.2 — backup/restore stamp behaviour), `docs/dev-artifacts/design/engine-metrics.md` (metric names)
- This document changes no code, test, CI or `.rust-guard` file. OBSERVED = a fact in the current code, cited with `path:line`. PROPOSED = a decision of this document. Paths without a `docs/` prefix are relative to `crates/`.
- Boundaries: point-in-time restore is R2b, encryption is R2c, scheduling is R2d. This document only leaves their extension points.

## 0. Summary and decisions

Today backup is stop-and-copy: the operator stops the server and runs
`backup --to`, which recursively copies `data_dir` with no lock and no
coordination with a writer (`server/shamir-sync/src/sync/durability/backup.rs:5-6`,
`backup.rs:218`; no `maintenance_lock::acquire` in the file — the lock helper
exists at `server/shamir-sync/src/sync/durability/maintenance_lock.rs:9-30`
but is not called from backup). The module doc states why: fjall exposes no
file-level backup API, and its journal recovery
(`RecoveryMode::TolerateCorruptTail`) recovers to the last complete batch, so
a copy taken while the writer is paused is consistent (`backup.rs:13-27`).
That argument does not hold while a writer runs: a concurrently copied
directory can mix files from different instants, and the WAL — the only
component that knows the commit order — is rotated and truncated under the
copy.

This design adds an online backup built from the pieces the code already
has: a **short write barrier** (per-repo flush + drain quiesce), a **WAL
truncation pin** (NEW — `RetentionGate` (`engine/shamir-engine/src/repo/journal/backend/durability_handle/retention_gate.rs:12-27`)
only refuses writes and is not a truncation pin; the pin must be added, 4.4), a **file copy** of
store directories plus retained WAL segments, and a **restore-time WAL
replay** through the existing boot-recovery path (`recover_inflight_v2`, not the warm drainer — 5). The writer-block bound is two short
barriers (point capture, point seal), not the copy duration.

Decisions (owner-level ones repeated in section 8):

| # | Decision | Where |
|---|---|---|
| D1 | Recommended option: copy + WAL replay (option C of section 3) | 3.6 |
| D2 | Two short write barriers (capture, seal); writers blocked only for flushes, never for the copy | 4.2 |
| D3 | Consistency point = per-repo (and system store) highest fully-captured WAL version recorded in the manifest | 4.3 |
| D4 | WAL truncation pinned below the seal point for the whole copy via a NEW truncation pin (`RetentionGate` is a write-refusal flag, not a truncation pin — see 4.4) | 4.4 |
| D5 | Online backup is a server-side operation; the CLI `backup --to` stays offline-only and is not given a `--online` flag in R2a | 4.7 |
| D6 | Entry point: new admin `BatchOp` variants dispatched by `execute_admin` (wire-protocol change), gated by the existing `is_superuser` bridge | 4.7 |
| D7 | Manifest `format_version` bumps to `2`; v1 snapshots keep restoring | 4.5 |
| D8 | Snapshot layout gains a top-level `payload/` marker field (`encryption: "none"`) as the R2c extension point; no encryption is designed | 4.6 |
| D9 | A second concurrent online backup is refused (one permit per server); an offline `backup --to` on a live dir is not detected in R2a (documented residual risk) | 4.4 |
| D10 | Restore of an online copy replays WAL; restore of an offline v1 copy is byte-identical to today (no replay) | 5.1 |

Owner-level deviation from the release plan, stated explicitly: `02-online-backup-and-encryption.md` §2 recommends a LOGICAL streaming copy (variant Б, reusing the seed-artifact codec) for R2a and defers physical copy (variant А); this design chooses physical copy + WAL replay (D1). Release-plan R2a requirements this design does not cover are listed in the Review (read-rate limiting; pin-duration metric + bound-by-setting; per-db schema version in the manifest — added to 4.5; secrets/TLS-token-key exclusion; "restore only into an empty data dir" — intentionally NOT taken, swap-over-existing kept per D10).

What R2a does versus defers: online backup protocol, manifest v2, online
restore + replay, offline `verify` of both kinds, tests, metrics names,
guide section. Defers: PITR to an arbitrary instant (R2b), encryption at
rest (R2c, layout extension point only), scheduled/continuous backup
(R2d), backup of repositories whose catalogue path lies outside `data_dir`
(same scope as the stamp's external-path deferral — the refuse-external-path default is stamp Q2; the "stamp per data dir vs external repo dir" question is stamp Q1).

## 1. Observed facts

Everything in this section is OBSERVED.

### 1.1 Current backup

* Entry: `backup(from, to)` (`server/shamir-sync/src/sync/durability/backup.rs:218`). It rejects a destination inside the source (`backup.rs:225`, helper `:272`), observes the format stamp read-only via `observe_stamp_for_backup` (`backup.rs:229`), creates `to/<YYYYMMDD_HHMMSS>/` (`backup.rs:239` — `create_dir_all`, NOT atomic `fs::create_dir`; refuses existing dirs `:190-192`; timestamp via `utc_timestamp_at` `:553-556`, format string `:555`), copies recursively, writes the manifest, fsyncs the destination directory once (`backup.rs:246-256`).
* Copy: `copy_dir_recursive` (`backup.rs:591`, `pub(crate)` — reused by restore). Skips the top-level `cache/` subtree only (`backup.rs:603-604`); symlinks/sockets/fifos are skipped with a warning (`backup.rs:632-634`). Every copied file is re-opened and `sync_all`'d (`backup.rs:625-628`); every directory written gets a directory fsync, log-only on failure (`backup.rs:652`). `fs::copy` on a file that a writer is appending to copies whatever bytes are visible at that moment — there is no coordination with any writer.
* Manifest: `Manifest { format_version: u32, created_at_unix_ns, files }` (`backup.rs:169-173`), const version `1` (`backup.rs:176`), written by `write_manifest` (`backup.rs:330`) after hashing every just-copied file with streaming SHA-256 (`hash_file_streaming`, `backup.rs:58`; entries `ManifestFileEntry { path, sha256, size_bytes }`, `backup.rs:156-165`; collection `:366`). Paths are forward-slash normalized for cross-platform verify (`relative_slash_path`, `backup.rs:398`).
* Verify: `verify_manifest` (`backup.rs:411`) rejects a missing/invalid manifest, a wrong `format_version` (`:421-426`), cache paths (`:433`, helper `:490`), unsafe (`..`/absolute) paths (`reject_unsafe_manifest_path`, error variant `backup.rs:114-119`), duplicate entries (`:123-127`), checksum/size mismatch (`:97-103`), and files on disk not listed in the manifest (`:104-108`). It re-hashes every listed file — the offline verifiability primitive this design reuses.
* Stamp observation: `BackupStampObservation` attached to `BackupReport` (`backup.rs:150-151`); backup never refuses and never stamps (design `format-stamp.md` 3.5, decision D10).
* The file copy brings `format.stamp` along as an ordinary file (no special handling; `backup.rs:227-229`).

### 1.2 Current restore

* Entry: `restore_with_probe(from, data_dir, force, probe)` (`server/shamir-sync/src/sync/durability/restore.rs:158`). Steps, all observed in the function body and its doc comment (`restore.rs:8-57`):
  1. Liveness probe unless `--force` (`restore.rs:164-166`) — the wrapper opens the live `server_meta` store and relies on fjall's own exclusive lock (`restore.rs:9-17`).
  2. `verify_manifest(from)` before touching `data_dir` (`restore.rs:174`).
  3. Stamp gate `restore_snapshot_gate(from, &EpochPolicy::compiled())` (`restore.rs:180`) — newer/corrupt/newer-layout refuse; absent/older allowed (design `format-stamp.md` 3.5, 5.2).
  4. Stage to a sibling `<data_dir>.restore_tmp_<stamp>` created with atomic `fs::create_dir` (TOCTOU-safe, `restore.rs:186-216`, atomic create at `:203`); copy failure cleans the stage dir (`cleanup_staged_temp_dir`, `restore.rs:422`).
  5. Session tickets invalidated **in the staged copy** via `open_and_invalidate_users` (`restore.rs:230-243`, call `:234`, helper `:407`); failure cleans the stage dir.
  6. Restore marker `.shamir_restore_epoch` (const `restore.rs:331`, writer `:334`) written into the staged copy before any swap.
  7. Atomic swap: `data_dir` → `.pre_restore_backup_<ts>` sibling (kept as the rollback path, `restore.rs:251-256`), then staged dir → `data_dir`, with directory fsyncs (`fsync_dir` `restore.rs:475`, Unix-only no-op fallback `:500`; strict private variant `sync_restore_directory` `:387`, `#[cfg(unix)]`). Failure of the second rename triggers best-effort rollback and returns `SwapPartialFailure` or `SwapFailedRollbackSucceeded` (`restore.rs:275-312`).
* Restore never opens the engine for replay: the copy is trusted as-is; WAL recovery happens later inside the next server boot (`server/shamir-server/src/engine/server/server_launcher/launch.rs:415` calls `ShamirDb::init_with_durability`, and per-repo WAL recovery runs inside `add_repo_with_durability` — `engine/shamir-db/src/shamir_db/shamir_db/core.rs:461` per `format-stamp.md` 0/1.3).
* CLI wiring: `backup` at `server/shamir-server/src/main.rs:372`, `restore` at `main.rs:383`; both dispatch without booting the server (`main.rs:327` comment).

### 1.3 Storage facts an online copy depends on

**Fjall backends.** All durability goes through one seam: `persist_sync_all(db)` → `Database::persist(PersistMode::SyncAll)` (`storage/shamir-storage/src/stores/storage_fjall/persist.rs:25-27`). Repo commits apply an atomic `Database::batch()` and persist once (`storage/shamir-storage/src/stores/storage_fjall/repo.rs:114-119`); `persist_once` is one `SyncAll` covering every keyspace of that repo database (`repo.rs:201-210`). Per-store `flush`/`flush_drain` exist (`storage/shamir-storage/src/stores/storage_fjall/store.rs:289,303`). The repo backend opens the database directory directly (`storage/shamir-storage/src/stores/storage_fjall/repo.rs:126`) — the directory tree of journal + LSM segment files described in `backup.rs:5-6` is the thing a file copy must capture. Fjall owns compaction internally; the engine never sees file-level replacement, which is why no engine-level file list exists today.

**WAL.** One directory per repo: `<repo path>.shamirwal/` (`engine/shamir-engine/src/tx/commit/core/durability_access.rs:99-117`; doc `:99-107`, impl `:108-117`), segments `NNNNNNNN.wal` with `WMT3` (legacy `WMT2`) sidecars `NNNNNNNN.meta` and temp `.meta.tmp` (`storage/shamir-wal/src/entry/segment_meta.rs:10-39` per `format-stamp.md` 1.1). `SegmentSet` holds one active + sealed segments behind one mutex; sealing happens on rotation when `max_bytes` is exceeded (`storage/shamir-wal/src/segment_set/segment_set.rs:4-9,467 seal_and_rotate`); appends are batched (`append_batch`, `segment_set.rs:328`); `sync`/`sync_active` (`:706,:715`) fsync the active segment; `replay` (`:562`) reads all segments; `truncate_below(durable_version)` deletes whole sealed segments once every record in them is below the durable horizon (`:590`) — this is the retention mechanism; `has_truncatable` (`:680`) and `lock_truncation` (`:687`) exist, and truncation-failure injection seams are already in place (`:291,:296,:309`). `max_committed` (`:730`) exposes the highest appended version.

**Drain (WAL → stores) and retention.** The drainer applies WAL entries to stores in `drain_step` (`engine/shamir-engine/src/tx/drain/drainer/drain_step.rs:51`) and persists a SINGLE monotonic last-committed watermark + next_tx_id snapshot into the repo's `__tx__` store (`drain_step.rs:880` `persist_markers`, `commit_phases.rs:851-892`; `tx_access.rs:112-114`) — there are NO per-version applied markers. `RepoWalManager::commit` is a no-op (`storage/shamir-tx/src/tx/repo_wal_manager.rs:174-179`). Boot recovery replays EVERY retained WAL entry unconditionally (`recover` = `SegmentSet::replay` over all segments, `repo_wal_manager.rs:182-184`; `recover_inflight_v2` sorts by commit_version and applies all, `tx/recovery/v2.rs:339-383,440-447`); the watermark only seeds the version floor (`v2.rs:357-372`). Orphan repair repairs missing changefeed events only (`drainer/orphan_repair/repair.rs`; events dedupe by version) — it is NOT a replay-idempotency mechanism. The repo's `RetentionGate` (`engine/shamir-engine/src/repo/journal/backend/durability_handle/retention_gate.rs:12-27`; accessor `:89`) is a write-refusal `AtomicBool` (`wal_retention_refuse_writes`, consulted at `tx/commit/pipeline/phase/commit/entry.rs:643`) plus counters/metrics; its only floor API is `reserve_recovery_floor` (`:101`, used by `tx/recovery/clean_horizon/recovery_floor.rs:8`). It plays NO role in the drainer's truncation ceiling, which comes from the `durable_watermark`/`pending_unsafe`/repair clamp (`tx/drain/drainer/truncate.rs:76-104`); `lock_truncation` (`segment_set.rs:687`) is `pub(super)` internal. The commit layer holds the gate (`engine/shamir-engine/src/tx/commit/core/durability_access.rs:5`).

**System store, registry, launcher order.** The system store is a fjall directory `shamir_db_meta.redb/` under `data_dir` (layout comment `launch.rs:398`, `meta_path` join `:406`; `engine/shamir-db/src/shamir_db/system_store/store.rs:62-67,153-214` per `format-stamp.md` 1.1); it carries the database/repository/table catalogue rows (`core.rs:398-413`). Its flush entry `flush_buffers_with_quiesce_timeout` (`system_store/store.rs:328-333`) exists but is `pub(crate)`. The wire-tables registry is a single file written by `write_atomic` — tempfile + `persist`, **no fsync** (`server/shamir-server/src/engine/tables_registry.rs:159-170`; opened `launch.rs:468`). Boot order (`launch.rs`): `create_dir_all(data_dir)` `:111` → data-dir lock `:120` (`acquire_server`) → `ServerMetaStore::open_or_init` (server_meta) `:147` → users `:151-154`, counters `:155-158` → audit → `ShamirDb::init_with_durability` `:415` (WAL recovery inside) → `consume_restore_marker` `:424` → default repo (`ensure_default_database`/`add_repo("default")`) `:453-460` → `TablesRegistry::open` `:468`.

**Data-dir lock.** `.shamir.lock`, `fs4` exclusive try-lock, PID written (`server/shamir-sync/src/sync/durability/maintenance_lock.rs:9-30`, open `:22`, `try_lock_exclusive` `:23`). It serializes server instances and maintenance opens (`boot/policy/token_cli/commands/create.rs:179`), but `backup` never takes it (`backup.rs` has no `acquire` call) and neither does `doctor`/`access-tree` (finding 1 of `format-stamp.md` review).

**Torn-write hazards a concurrent copy sees.** While a copy runs, a writer can: append a partial batch to the active fjall journal (recovered by `TolerateCorruptTail` per `backup.rs:13-27` — per-Database, `storage/shamir-storage/src/stores/storage_fjall/repo.rs:201-204`: a torn journal tail drops that database's partial batch boundary) compact/rename/delete fjall SST files between the copy's `read_dir` and `fs::copy` (a file deleted mid-copy fails the copy; a file created after its directory was visited is silently missed); rotate and then truncate WAL segments the copy has not yet reached (silent hole in the copied history — the worst case, because it removes commits the store files may not contain); replace `NNNNNNNN.meta` sidecars and the registry file (registry: non-fsynced overwrite, `tables_registry.rs:159-170`). None of these is detected by today's backup; today they cannot happen because the server is stopped.

**Existing quiesce points.**
* Per-table writer drain: `WriterDrainBarrier` (`engine/shamir-engine/src/table/support/guards/writer_drain_barrier.rs`) with `enter_writer` `:309` and `drain` `:355` as the production primitives; `begin_drain` `:325` and `is_drained` `:330` are `#[cfg(loom)]`-only. Held per table manager (`engine/shamir-engine/src/table/table_manager.rs:290,712`). There is no engine-wide barrier object today — a cross-repo barrier is composition, not a primitive.
* Flush: `flush_all_history` drains every table's history version log and persists the repo database once (`engine/shamir-engine/src/tx/commit/core/durability_access.rs:263-283`, per-store flush `:272`, `persist_once` call `:280` — `:281` is the log line).
* Clean shutdown: `serve` awaits shutdown then `handle.shutdown_with_report` under a deadline (`server/shamir-server/src/boot/runtime.rs:21-66`; `server_handle.rs:180,193`). The server scheduler runs background GC/persist ticks with a shutdown `Notify` (`server/shamir-server/src/boot/scheduler.rs:11-12,32-42`).

**Admin/auth surface.** `execute_admin` dispatches `BatchOp` variants (create/drop/rename db/repo/table, indexes, …) (`engine/shamir-db/src/shamir_db/execute/core/admin_dispatch.rs:24,33-50`). The server-side permission bridge requires `is_superuser` for every `BatchOp::is_admin()` op (`server/shamir-server/src/engine/db_handler/handler.rs:35-53` doc-comment gate; enforcement `handler.rs:670` — `entry.op.is_admin() && !is_coarse_admin_gate_exempt`). The backup module doc already anticipated this route: an in-process snapshot "requires a wire-protocol change" (`backup.rs:29-32`; `:32` for the exact phrase). No HTTP admin endpoint for maintenance operations was observed.

**Metrics.** `shamir_backup_*` names are reserved in the metrics design (`docs/dev-artifacts/design/engine-metrics.md:160`); conventions: `shamir_` prefix, `_total` counters, `_seconds`, `_bytes` (`engine-metrics.md:29`).

## 2. Requirements (testable)

Each requirement is stated so a test can decide pass/fail. "Copy" = the online backup operation; "restore" = section 5's procedure.

* **R1 Consistency point.** Restoring a completed copy yields the state in which all repositories and the system store were consistent at one instant: for every repo, the restored store state plus replayed WAL equals the live state exactly at that repo's recorded consistency version (manifest field, 4.5); catalogues, repos and the registry agree with each other. Weaker guarantee NOT taken: per-repo independent instants (rejected — cross-repo references via the catalogue would dangle).
* **R2 No lost acknowledged write.** Every write whose acknowledgement reached the client before the capture barrier (step B, 4.2) survives restore. Verified with an acknowledged-writes oracle (6.1): acked writes with timestamp < capture instant ⊆ restored state; acked writes after capture are NOT required (R2b may relax further; here the seal barrier pins the upper edge too).
* **R3 Writer-block bound.** Writers are blocked only inside the two barriers; each barrier lasts at most the per-repo flush time plus a constant, measured and asserted (`shamir_backup_writer_block_seconds`, 4.8). The file copy itself never holds the write barrier. Default bound to assert in tests: p99 barrier < 2 s on CI-sized data (6.4).
* **R4 Failure isolation.** A failed or cancelled copy leaves the source untouched (no mutation, no truncation, truncation pin released) and removes its partial output directory; a crash leaving partial output leaves a directory that `verify_manifest` refuses and restore never adopts. Same discipline as restore's staged temp dir (`restore.rs:216-222,422`).
* **R5 Offline verifiability.** `verify` of a copy directory (online or offline kind) runs without a server, re-hashes every manifest file, checks the manifest self-consistency (file set, versions, WAL positions) and reports the stamp observation — today's `verify_manifest` (`backup.rs:411`) extended, not replaced.
* **R6 No source degradation.** During and after a copy, the live server meets its existing durability guarantees (WAL sync on commit, `segment_set.rs:706`; per-commit persist, `repo.rs:114-119`) and WAL retention does not lose recoverable history beyond the pre-copy retention policy.

## 3. Options

Every option is judged against the facts of section 1. "Change surface" names the crates whose internals must change; the engine is being cut into crates, so changes inside `repo`/`table`/`tx` (`shamir-engine`) are weighted heaviest.

### 3.1 Option A — consistent file copy under one short write barrier

Hold a write barrier for the whole copy: quiesce writers, flush all stores, copy every file, release. Simplest correctness argument (exactly the one `backup.rs:13-27` makes for the stopped server).

* Needs: an engine-wide barrier composed from the per-table `WriterDrainBarrier` (`writer_drain_barrier.rs:309-355`) plus `flush_all_history` (`durability_access.rs:263`). No WAL, no replay, no manifest positions.
* Cost: writers blocked for the entire copy — minutes on a real directory. Violates R3 as stated. Fjall compaction running inside the barrier is unknown territory (fjall internals are not ours to pause).
* Failure modes: a stuck flush blocks writers indefinitely; cancellation mid-copy blocks writers until abort.
* OS: none particular — no concurrent mutation by construction.
* Verdict: **rejected for R2a** (bound). Acceptable only as the degenerate "small directory" case; not chosen even there — one code path.

### 3.2 Option B — storage-level snapshot / hard links

Take an OS-level snapshot or hard-link every file, then copy the links at leisure.

* Needs from `shamir-storage`: a file-listing + "replace files only by rename" contract from the fjall backend. Fjall exposes no such API (`backup.rs:13-27`: module-doc text on the missing backup API and `TolerateCorruptTail`) Journal appends mutate files in place, and compaction rewrites trees inside the vendored/backend code — hard links to in-place-mutated files are torn by construction, not by race.
* OS: hard links are reliable only if files are immutable and replaced by rename; NTFS hard links exist but a `FILE_SHARE_DELETE`-less writer holding a file open (Windows default for fjall's owned handles) makes link-and-replace semantics fragile; APFS/ext4/btrfs differ in snapshot support (none portable without a new dependency — a volume-manager integration is explicitly out of scope and would be an owner decision with a no-dependency alternative: none exists).
* Failure modes: silent torn files; no consistent point provable from the manifest.
* Verdict: **rejected** — requires changing/implementing backend file semantics we do not own (fjall), plus OS-specific volume features. Highest change surface in exactly the crates the release plan says to leave alone.

### 3.3 Option C — copy + WAL replay (RECOMMENDED)

Two short barriers bracket the copy; WAL segments retained across the whole window are copied alongside the store files; restore replays.

* Needs from `shamir-wal`: segments, sidecars, `replay`, `truncate_below` and `max_committed` exist (`segment_set.rs:562,590,730`), but a REAL truncation-pin API is required — new, small: `RetentionGate` (`retention_gate.rs:12-27,89`; `durability_access.rs:5`) only refuses writes (`entry.rs:643`); the drainer's truncation ceiling ignores it (`truncate.rs:76-104`). The pin must be added in `shamir-wal` and wired into `shamir-engine`'s drainer truncate path (a change inside the engine's tx layer — listed in the comparison table and 7).
* Needs from `shamir-engine`: expose a small point-capture/quiesce entry (flush all + hold the new truncation pin + report per-repo `max_committed`) and the restore-side replay entry — the existing boot-recovery path (`repo_wal_manager.rs:182-184`; `recover_inflight_v2` `v2.rs:339-447`), not the warm drainer, pointed at the restored directory. These live in the `db_instance` layer INSIDE `shamir-engine` (its layer order puts `db_instance` above `instance`/`tx`), not in `shamir-db`; they require widening a few `pub(crate)` items to `pub` (retention-gate accessor `retention_gate.rs:89`, `flush_all_history` `durability_access.rs:263`, the new pin) — a visibility change to declare to rust-guard. Still no change inside `repo`/`table` internals.
* Needs from `shamir-sync`: the copy engine, manifest v2, verification — the crate that already owns backup/restore.
* Cost: copy is full-size (no dedup); restore adds replay time proportional to retained WAL. Barrier cost = flush time (R3 satisfied).
* Failure modes and their answers: WAL hole → the new truncation pin (4.4); torn journal tail → per-database `TolerateCorruptTail` (`backup.rs:13-27`; `repo.rs:201-204`) + replay convergence (4.2, load-bearing requirement L1); mid-copy file deletion/creation → copy retries the directory pass and the manifest's final hash pass catches inconsistency (4.2 step D).
* OS: Windows — files held open by the live server cannot be copied unless opened with share flags; the copy must open sources with read + `FILE_SHARE_READ|FILE_SHARE_WRITE` (std `fs::read`/`File::open` already requests read-share on Windows; nothing new needed) and must tolerate `ERROR_SHARING_VIOLATION` with a bounded retry (same class as the stamp's Q13, `format-stamp.md` section 8). Directory fsync exists on Unix only — reuse the existing log-only contract (`restore.rs:475,500`). macOS: same as Linux for this protocol (no dir-fsync guarantees needed beyond today's).
* Verdict: **recommended** (D1) — smallest change inside `repo`/`table`/`tx`, reuses the retention and drain machinery that already exists, and its restore path is the same recovery code the server already runs at boot.

### 3.4 Option D — replication to a file

Stream commits through the replication path into a file-backed sink.

* Needs from `shamir-engine`: a new replication sink backend (today's seed exporter writes a one-shot artifact, not a continuous sink — `server/shamir-sync/src/sync/replication_seed/export/exporter.rs:24-31`); a file-store implementation of the sink trait; handshake/epoch surface is R1d territory (`format-stamp.md` 0 deferrals).
* Cost: new sink trait implementation + continuous state machine; a second durable stream to maintain; recovery of the sink itself.
* Verdict: **rejected for R2a** — the most new machinery for no additional guarantee over option C; revisit if R2d wants continuous shipping.

### 3.5 Comparison table

| Option | Engine (`repo/table/tx`) change | storage/wal change | new dependency | writer block | restore | verdict |
|---|---|---|---|---|---|---|
| A full-barrier copy | barrier composition only | none | none | whole copy (R3 ✗) | as today | rejected (bound) |
| B snapshot/hard links | none | fjall file contract (not ours) | OS/volume features | none | as today | rejected (ownership + OS) |
| C copy + WAL replay | 2 facade entries (shamir-engine `db_instance` layer) + truncation pin wiring in the drainer truncate path (tx layer); `pub` widening of a few `pub(crate)` items; no repo/table internals | truncation pin (new, small) | none | 2 short barriers | replay | **recommended** |
| D replication to file | new sink backend | sink store | none | none (async) | sink replay | rejected (scope) |

## 4. Design of the recommended option (PROPOSED)

### 4.1 Components and placement

* `shamir-sync`, new folder `src/sync/durability/backup/online/`, after a mechanical regroup: `shamir-sync/src/` (7 entries: `config_error.rs`, `format_stamp/`, `lib.rs`, `meta_port/`, `subscriptions/`, `sync/`, `version.rs`), `sync/` (7) and `sync/durability/` (7: `backup.rs`, `config.rs`, `maintenance_lock.rs`, `mod.rs`, `restore.rs`, `tests/`, `tx_registry.rs`) are all at the 7-entry limit, and `sync/connection/`, `sync/replication/`, `sync/replication_seed/` are full too, so no existing folder takes a new entry. The regroup moves `backup.rs` and `restore.rs` into a new `sync/durability/backup/` (folder `durability/` goes 7 → 6 entries; the old public paths stay valid through re-exports in `durability/mod.rs`), and the online engine lives in `backup/online/`: `mod.rs`, `protocol.rs` (state machine + cancellation), `copy_engine.rs` (share-tolerant recursive copy + hash pass), `manifest.rs` (v2 encode/decode), `wal_capture.rs` (segment listing/pinning calls), `progress.rs`, `tests/` (7 entries). The regroup is a pure move with no behaviour change; it needs a `[[moved]]`-only campaign delta (Q10). The offline `backup.rs`/`restore.rs` gain only call sites, exactly like the stamp wiring (`format-stamp.md` 7 step 2).
* `shamir-engine`, the `db_instance` layer (inside shamir-engine's layer order — `repo<table<validator<tx<query<instance<db_instance`; NOT a `shamir-db` layer): `capture_point()` and `seal_point()` — flush + pin + position report; `replay_to(repo_path, wal_dir)` — boot-recovery entry against an offline directory. Requires widening `pub(crate)` items to `pub` (retention-gate accessor `retention_gate.rs:89`, `flush_all_history` `durability_access.rs:263`, the new truncation pin, system-store `flush_buffers_with_quiesce_timeout` `system_store/store.rs:328-333`) — a visibility change to declare to rust-guard — plus the pin wiring in the drainer truncate path (a tx-layer change). No change inside `repo`/`table` internals.
* `shamir-server`: admin-op glue (4.7), progress reporting, metrics emission.

### 4.2 Protocol

State machine: `Preparing → Captured → Copying → Sealing → Verifying → Done | Failed(stage)`.

* **A. Prepare.** Acquire the single in-process backup permit (D9). Acquire the NEW truncation pin on every catalogue repo (held from now; `RetentionGate` is not sufficient — 4.4). Create the destination `<to>/<timestamp>/`: today's offline backup uses `create_dir_all` (`backup.rs:239`), which is NOT atomic-create; the atomic-create TOCTOU discipline is restore's precedent (`restore.rs:203`). PROPOSED: atomic `fs::create_dir` following that precedent, or `create_dir_all` + refuse-existing (`backup.rs:190-192`).
* **B. Capture barrier (short).** Per repo: hold the writer-drain barrier, run `flush_all_history` (`durability_access.rs:263`; `persist_once` call `:280`), read `max_committed` (`segment_set.rs:730`) → `wal_position[repo] = V_r`; flush the system store (`flush_buffers_with_quiesce_timeout`, `system_store/store.rs:328-333` — `pub(crate)`, needs widening) and record its version. Release the write barrier per repo as soon as that repo is flushed. Barrier cost = flush time only (R3). Record the stamp observation and binary/schema versions here.
* **C. Copy WAL first.** Copy every WAL segment file + sidecar (sealed and active) per repo, plus a small `wal_manifest.json` listing segment seqs and hashes. Copying WAL before stores means the history pinned at B is on disk before the store state that may depend on it. The active segment is copied as-is; its torn tail is recoverable per `backup.rs:13-27` (per-Database, `repo.rs:201-204`).
* **D. Copy stores (writers running).** Copy each store directory tree with the share-tolerant copy engine: per-directory `read_dir` pass, per-file copy with bounded retry on sharing violations (Windows), and a **second hash pass at the end** (`hash_file_streaming` semantics, `backup.rs:58`). A file that changed between copy and hash is re-copied once; still-changing files are re-copied in step E. A file deleted mid-copy is re-listed; the final manifest is built from what the last pass actually hashed, so the manifest never describes a file set that no longer exists.
* **E. Seal barrier (short).** Per repo again: hold barrier, flush, read `V_r'` ≥ any version whose effects may already be inside the copied store files; copy any WAL segments/sidecars created or rotated since step C (they are sealed by now or copied as active tails); record the consistency point `V* = V_r'`; release the pin only after the WAL of that repo is confirmed on disk. **Load-bearing requirement L1 (corrected):** replay idempotency does NOT come from per-version markers — none exist; the drainer persists a single watermark (`drain_step.rs:880`; 1.3). Idempotency rests on version-keyed LWW writes: Put/Delete → history row `key‖commit_version` (`v2.rs:655-718`; `drain_step.rs:444-462`), unattached tables absolute set/remove (`v2.rs:85-146`), IndexPut/IndexDel LWW full-value (`v2.rs:160-262`; `drain_step.rs:349-398,640-700`), interner `touch_with_id` fixed name→id (`v2.rs:617-625,262-300`; `drain_step.rs:947-970`); changefeed events dedupe by version. The genuinely non-convergent case is a copied store file containing drain effects ABOVE the copied WAL tail V* (replay stops at V*): the seal barrier must therefore **re-copy and hash-verify every mutable repo-database file after the barrier flush** (extends step D's second pass). The system store and tables registry have NO replay path — they MUST be captured under the same seal barrier discipline, after all repos (catalogue state older than the repo WAL would silently drop replayed ops: `v2.rs:97-110,170-181,220-232` resolve the table token or warn+Ok; restore-time replay fails closed on unresolved tokens, 5). The acceptance test of 6.1 exercises exactly this.
* **F. Verify + manifest.** Build `manifest.json` (4.5) from the last hash pass; write it last with fsync (`backup.rs:330-357` precedent); fsync destination directories (Unix; log-only, `restore.rs:475` contract). `verify_manifest`-style self-check of the finished snapshot before reporting success (R5).
* **G. Release.** Release truncation pins, drop the permit, emit metrics.

Cancellation: checked at every stage boundary and between files; cleanup = remove the destination directory (it was created atomically and never swapped into anything), release pins, mark `Failed(stage)`. A crash mid-copy leaves a partial directory with no manifest; restore refuses it exactly as today refuses an unmanifested snapshot (`restore.rs:174`).

### 4.3 Interaction with checkpoints, retention, compaction

* **WAL retention/truncation:** `RetentionGate` is NOT a truncation pin — as shipped, a truncation racing the copy is DETECTED (missing segment → hard `Failed(Copying)`), not PREVENTED, so every backup racing retention would fail. D4 therefore requires the NEW truncation-pin API (3.3) wired into the drainer's truncate path (`truncate.rs:76-104` — a tx-layer change, listed in 7). With the pin held (A→G), `truncate_below` must not delete segments at or above the pinned floor; the copy additionally re-checks segment presence before hashing (a missing segment is a hard `Failed(Copying)`).
* **Compaction:** fjall compacts concurrently with D; the design never assumes file stability — the final hash pass (step D/F) defines the snapshot. Extra I/O load is bounded by the copy engine's sequential per-directory order and is a metric (`shamir_backup_bytes_copied`), not a correctness concern.
* **Drainer:** runs normally throughout. Its progress moves the live store ahead of V*; irrelevant to the copy, which only reads files and re-hashes.
* **Registry/system store:** the registry file's non-fsynced overwrite (`tables_registry.rs:159-170`) means the copied registry may be a torn file. Mitigation: copy the registry **inside the seal barrier** (it is tiny), after a flush; a torn copy is detected by hash and re-copied under the barrier. Marked as a finding on `format-stamp.md`/storage: `write_atomic` without fsync remains a latent torn-write source for any copy scheme (this is the registry finding, not a stamp open question; `format-stamp.md` 1.6 notes the same).

### 4.4 Concurrency: lock and second backup

* The data-dir lock (`.shamir.lock`, `maintenance_lock.rs:22-23`) is held by the live server; the online backup runs **inside** the server process and does not re-acquire it. The in-process permit (one `tokio::sync::Mutex<()>` / atomic flag in `ServerLauncher`) refuses a second concurrent online backup with a distinct error (D9).
* An offline `backup --to` run by an operator while the server is live is *not* detected in R2a (today's code takes no lock, `backup.rs:218`). The guide must say "stop the server or use the admin operation". Detection (e.g. attempting `try_lock_exclusive` and refusing on success-with-data) is listed as an open question (Q5).

### 4.5 Manifest v2

PROPOSED shape (file `manifest.json` in the snapshot root, `format_version: 2`; v1 files keep parsing with `kind = "offline"` defaulted):

```
Manifest {
  format_version: 2,           // bump from 1 (backup.rs:176)
  kind: "online" | "offline",  // online ⇒ wal + consistency fields present
  created_at_unix_ns: u128,
  format_epoch: u64,           // from the stamp observation (backup.rs:229)
  binary_version: String,      // CARGO_PKG_VERSION at capture
  consistency: {
    per_repo: { <repo name>: { wal_position: u64, store_dir: String, schema_version: u32 } },   // schema_version NEW: release-plan R2a requires a per-db schema version; manifest v1/v2 shape lacked it
    system_store_version: u64,
  },
  wal: { dirs: [ { repo, dir, segments: [ { seq, file, sidecar, sha256, size } ] } ] },
  files: [ ManifestFileEntry ],   // unchanged shape (backup.rs:156-165)
}
```

Unknown keys are preserved on decode (the stamp codec's rule, `format-stamp.md` D3) so R2b/R2c can add fields without a version bump. `format_version` mismatch refusal semantics are unchanged (`backup.rs:421-426`). The epoch in the manifest fulfils the R2a requirement of `format-stamp.md` 0 ("epoch in the backup manifests" was deferred to R2 — this is that delivery); no stamp decision changes.

### 4.6 On-disk layout and the R2c extension point

```
<to>/<YYYYMMDD_HHMMSS>/          # unchanged outer naming (backup.rs:239)
├── manifest.json                # v2, written last
├── format.stamp                 # copied as today
├── <data_dir contents>          # system store, repos, WAL dirs, registry, audit…
└── backup.meta.json             # NEW: { encryption: "none", engine: "copy+wal-replay" }
```

R2c extension point: `manifest.json` gains a preserved-unknown `encryption` object and, when encryption lands, the payload moves under `payload/` with `backup.meta.json` describing it — a *reader* rule, not a format change: v2 readers must treat the directory as opaque payload plus manifest. No encryption is designed here (boundary R2c).

### 4.7 Command and server-side entry point

* Decision D6: the online backup is dispatched through the admin executor — new `BatchOp::StartBackup { to }` / `BatchOp::BackupStatus` / `BatchOp::CancelBackup` variants in `execute_admin`'s dispatch (`admin_dispatch.rs:24,33-50`), which the server already gates with `is_superuser` (`handler.rs:670`). This is the route the module doc anticipated (`backup.rs:29-32`). A wire-protocol (DTO) change is required; the alternative — a server-local control file or separate admin listener — adds an unauthenticated-or-separately-authed surface and is rejected. Flagged to the owner because it touches the client DTO (section 8, Q3).
* The CLI keeps exactly two subcommands (`main.rs:372,383`) in R2a: offline `backup` and offline `restore`. Online backup is requested through a client connected to the running server; `backup status`/`backup cancel` map to the status/cancel ops. Restore remains an offline CLI action against a stopped server (it swaps directories; it cannot run inside the live server — the probe's lock logic, `restore.rs:9-17`, stays authoritative).
* Progress: `BackupStatus` returns stage, per-repo bytes/files hashed-so-far (from `progress.rs`), started-at, and the last error if `Failed`. Cancellation is cooperative (4.2).

### 4.8 Metrics (names only; design in `engine-metrics.md`)

`shamir_backup_started_total`, `shamir_backup_completed_total`, `shamir_backup_failed_total` (labels: `kind=online|offline`, `result`, `stage`), `shamir_backup_active` (gauge), `shamir_backup_bytes_copied` (`_bytes`), `shamir_backup_duration_seconds`, `shamir_backup_writer_block_seconds` (histogram, labels `phase=capture|seal`), `shamir_backup_wal_segments_copied_total`. Plus the release-plan-required pin-duration metric (`shamir_backup_pin_seconds` or a bound-by-setting on `writer_block_seconds` — owner choice, see Review). All within the reserved `shamir_backup_*` space (`engine-metrics.md:160`; the "delivered with R2d" scheduling is confirmed as its open question 6, `engine-metrics.md:317`) and the naming conventions (`engine-metrics.md:29`). Emission goes through shamir-server's existing observability render path (the `durability_export.rs`/`visibility_export.rs` pattern, `engine-metrics.md:25-31`); `shamir-metrics` is a PROPOSED new crate (`engine-metrics.md` §3), not an existing dependency — this design takes no new dependency.

## 5. Restore and verification for online copies

* **Replay or none (D10).** `kind = "offline"`: today's path, unchanged, no replay (`restore.rs:158-329`). `kind = "online"`: after the swap's step-3 copy into the staged dir (`restore.rs:186-222`), run the replay entry of 4.1 against the staged copy: replay goes through the BOOT-RECOVERY path (`add_repo_with_durability`), which replays the ENTIRE retained WAL unconditionally — `recover` = `SegmentSet::replay` over all segments (`repo_wal_manager.rs:182-184`), `recover_inflight_v2` sorts by commit_version and applies all (`v2.rs:339-383,440-447`) — NOT "from the recorded `wal_position`": the watermark only seeds the version floor (`v2.rs:357-372`). Replay MUST use cold recovery semantics — `CounterDelta` is NOT applied on the cold path (`v2.rs:155-186`), never the warm drainer counter path (`drain_step.rs:786-812` increments). Replay fails closed on unresolved table tokens (restore-mode flag; live code warns+Ok, `v2.rs:97-110,170-181,220-232`). Replay failure aborts before the swap (`restore.rs:230-243` discipline) with `data_dir` untouched.
* **Consistency check.** After replay, the staged copy is opened once through `ShamirDb::init` (the same init the next boot would do, `launch.rs:415`) with a read-only consistency pass: catalogue rows ↔ repo directories ↔ the captured repo-set (Q9, not the live one); a mismatch refuses the restore. The check must TOLERATE a post-V* store state — store files may legitimately contain drain effects above V* (4.2 E). Replay must be run with cold-recovery semantics (above). (Same fail-closed bias as the stamp gate, `restore.rs:180`.)
* **Stamp refusal.** Unchanged: `restore_snapshot_gate` (`restore.rs:180`) on the snapshot's copied stamp; newer epoch refused, older allowed for rollback (`format-stamp.md` 5.2). The manifest `format_epoch` is cross-checked against the copied stamp — a mismatched manifest/stamp pair is refused as corrupt.
* **Clean-machine restore.** `data_dir` absent: steps 3-7 of `restore.rs` already handle the fresh target (`restore.rs:303-310`). For an online copy the replay step runs on the staged dir before the single rename; the machine needs no prior state.
* **Offline verify (R5).** `verify` extends `verify_manifest` (`backup.rs:411`) to: parse v2; check `kind`; re-hash every `files` entry and every WAL segment; check WAL seq contiguity per repo against the `wal` listing; report `format_epoch`, `binary_version`, `consistency` positions; refuse unmanifested files as today (`backup.rs:104-108`). No engine open — pure file inspection, runs on any machine with the binary.

## 6. Tests and acceptance (CI only)

All tests run in the PR's CI via `./scripts/test.sh` scopes (`@server`, `@storage`, `@e2e`); nothing runs locally (CLAUDE.md policy).

* **6.1 Property test under continuous write load (the gate test).** Boot a server (e2e harness style), run N writer clients that continuously insert and record each **acknowledged** write in an in-memory oracle, start an online backup mid-stream, keep writing until it completes, stop the server, `restore` the snapshot to a fresh `data_dir`, boot, and read back: (a) every oracle entry acked before the capture barrier is present with its acked value (R2); (b) the restored state equals the live state at the recorded `wal_position` for a probe set of keys (R1); (c) no torn/partial record anywhere. Replay-convergence requirement L1 is exercised here by construction (the copy races the drainer): the property tested is convergence under UNCONDITIONAL replay over a cross-instant file set, plus no silent table-token skips (Q3, inverted). Crate: `shamir-e2e` (server boots live there per CLAUDE.md) or `shamir-server` integration tests; sizes per 6.4.
* **6.2 Writer-block bound.** Same harness: measure capture/seal barrier durations via the metrics histogram; assert p99 < bound on CI-sized data (R3). Includes a compaction-pressure variant (larger batches forcing fjall flushes during the barrier).
* **6.3 Failure injection per protocol step.** Injected at each stage of 4.2 using the existing seam style (`segment_set.rs:291,296,309` precedent): fail during A (pin held → assert released on drop), B (flush error → no output dir), C/D (copy error mid-file, deleted source file, Windows sharing violation, hash mismatch on re-pass → source untouched, output removed), E (seal error → Failed(Sealing), WAL consistent), F (manifest write failure → no success report), G (cancel at each boundary → cleanup). Each asserts R4 with a before/after directory snapshot.
* **6.4 OS differences and CI feasibility.** Full-tier matrix runs Ubuntu/Windows/macOS (CLAUDE.md CI tiers; weekly schedule already covers `shamir-server` + `shamir-e2e` on all three). Windows-specific: sharing-violation retry path and rename-over-open-file behaviour; Linux: directory fsync paths; macOS: no additional assertion beyond the shared protocol. CI-sized budgets: data directory ≤ 200 MB, WAL ≤ 32 MB retained, whole gate test ≤ 3 minutes per OS (within the 180 s per-test default scaled by the harness splitting boot/backup/restore phases into separate tests sharing a fixture — if a single test exceeds the budget it is split; the nextest budget stays untouched).
* **6.5 Restore/verify acceptance.** v2 snapshot restores on a clean machine (no `data_dir`); `verify` refuses: checksum mismatch, WAL seq gap, manifest/stamp epoch mismatch, unmanifested file; v1 offline snapshot still restores and verifies unchanged (back-compat, D7).

## 7. Implementation plan (≤ 4 PR-sized steps)

1. **Point capture + replay facade in `shamir-engine`** (`db_instance` layer — inside shamir-engine's layer order, NOT `shamir-db`): `capture_point`/`seal_point`/`replay_to` composing `flush_all_history` (`durability_access.rs:263`), the NEW truncation pin and `max_committed` (`segment_set.rs:730`); unit tests with the existing drain/retention seams. Costs declared to rust-guard: widening `pub(crate)` items to `pub` (retention-gate accessor `retention_gate.rs:89`, `flush_all_history`, system-store `flush_buffers_with_quiesce_timeout` `system_store/store.rs:328-333`) and the pin wiring in the drainer truncate path (`truncate.rs:76-104`, a tx-layer change). No `shamir-sync`, no protocol change, no change inside repo/table internals.
2. **Online copy engine in `shamir-sync`** (`src/sync/durability/backup/online/` per 4.1; the first commit of this step is the mechanical regroup of `backup.rs` and `restore.rs` into `sync/durability/backup/` with re-exports, then the new module): protocol state machine, share-tolerant copy engine, manifest v2 encode/decode with unknown-key preservation, progress/cancellation; extend `verify_manifest` for v2 and keep v1 green. Pure library; offline `backup` untouched.
3. **Server wiring + admin ops + restore replay** (`shamir-server`, `shamir-db` DTO, `shamir-connect` DTO): `BatchOp::StartBackup/BackupStatus/CancelBackup`, dispatcher arms (`admin_dispatch.rs:33-50` area), permission reuse (`handler.rs:670`), in-process permit (D9), `restore` kind-aware replay + consistency check (section 5), metrics emission (4.8). This is the PR with the wire change (Q3).
4. **Tests + guide**: the gate property test and failure-injection suite (6.1-6.3) in `shamir-e2e`/`shamir-server`; `docs/guide-docs/guide/07-operations.md`, new subsection under `## 8. Backup` — when to use online vs offline backup, the admin-operation procedure, what the manifest's consistency point means, `verify` usage, Windows sharing-violation note, "an offline `backup --to` on a running server is not supported" (Q5), and the R2b/R2c boundary notes.

No new dependencies anywhere (D-decision check: `RetentionGate`, `SegmentSet`, drainer, `sha2`, `serde_json`, `fs4` are all already in the dependency graphs of the touched crates — `format-stamp.md` 1.6). Metrics are emitted through shamir-server's existing observability render path (`engine-metrics.md:25-31`); `shamir-metrics` is a proposed NEW crate (`engine-metrics.md` §3), not an existing dependency, and is not added here.

## 8. Open questions (each with a default)

* **Q1. Admin `BatchOp` DTO change (D6).** Adding `StartBackup/BackupStatus/CancelBackup` touches the client DTO and wire protocol. Default: accept the DTO change; alternative without a protocol change — reuse the existing generic admin batch envelope with string op names, deferring typed variants.
* **Q2. Retention pin floor.** Pin at the pre-copy retention floor (copy carries full retained history — simplest, bigger snapshots) vs pin exactly at `V_capture` (smaller, but requires proving no store file copied in D reflects a version below the floor). Default: full retained history (floor = current retention floor); the smaller pin is an optimization after L1 is proven by 6.1.
* **Q3. Replay-convergence proof (L1, inverted).** The property to prove is NOT per-version marker idempotency (no markers exist — 1.3): it is convergence of UNCONDITIONAL boot-recovery replay over a cross-instant copied file set, plus no silent table-token skips. Default: 6.1 is the proof obligation; if a non-convergent write kind is found, extend the seal barrier's re-copy scope for that kind (bounded, still R3).
* **Q4. System-store version source.** The system store's monotonic version for `consistency.system_store_version` — a dedicated counter vs reuse of the catalogue write horizon. Default: reuse the existing horizon the catalogue already exposes; add a counter only if none is readable at the facade layer.
* **Q5. Offline `backup --to` on a live server.** Detect and refuse (attempt `try_lock_exclusive`; success + data present ⇒ a live server exists ⇒ refuse) vs keep silently allowed. Default: refuse with a pointed error in the same PR as step 3; the silent behaviour is today's documented residual risk until then.
* **Q6. Snapshot retention/rotation of the destination.** Keep `--to` append-only (`backup.rs:190-192` refuses existing timestamp dirs) vs add prune flags. Default: append-only in R2a; pruning is R2d (schedule) territory.
* **Q7. Registry `write_atomic` without fsync** (`tables_registry.rs:159-170`) — a torn registry copy is only caught by the hash pass. Default: copy the registry inside the seal barrier (4.3) and list the missing fsync as a storage-side finding for a separate task; no behaviour change in this design.
* **Q8. In-memory and hybrid repos.** `in_memory` repos have no files; `hybrid` repos use paths like `fjall` (`format-stamp.md` 1.2). Default: online backup covers file-backed catalogue repos under `data_dir`; `in_memory` repos are reported in the manifest as `in_memory` (not recoverable from a file copy — same scope line as the stamp's external-path deferral, Q2).
* **Q9. Catalogue mutation during copy (NEW).** Drop/rename are in-memory re-key + catalogue row update only; the repo dir on disk is never deleted (`engine/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:544-578`; `db_instance.rs:192-194`). Consequences: a repo dropped mid-copy leaves an inert orphan dir (benign — copied, never restored as live); a repo CREATED mid-copy may miss its store dir → the consistency check would spuriously refuse the whole backup; renamed mid-copy: unstated. Default: snapshot the catalogue repo-set at the capture barrier and back up ONLY that set (created-after-capture excluded and reported; dropped-after-capture still copied as inert orphan); the consistency check compares against the captured set, not the live one.
* **Q10. Placement of the online engine in `shamir-sync` (NEW).** No folder in `shamir-sync` has a free slot (4.1). Default: regroup `backup.rs` and `restore.rs` into `sync/durability/backup/` and put the engine in `backup/online/`; the regroup is a pure move under a `[[moved]]`-only campaign delta, re-exports keep the public paths. Alternative: a new crate in the `server` group (3 of 7 slots used) that depends on `shamir-sync` for the manifest types; it costs a dependency edge and splits manifest v1/v2 across crates, so it is not the default.

## Findings against other designs

1. `format-stamp.md` decisions: none changed. The R2 deferral "epoch in the backup manifests" (`format-stamp.md` 0) is fulfilled by 4.5 exactly as that document anticipated; stamp check/gate semantics (`backup.rs:229`, `restore.rs:180`) are reused unchanged.
2. `backup.rs` module doc claims a copy "during a quiescent window between commits" recovers consistently (`backup.rs:13-27`) — true per-database (`repo.rs:201-204`), but section 1.3's hazards show it does not extend to concurrent multi-store copies; this design supersedes that reasoning for the online case without changing the offline path.
3. `engine-metrics.md:160` reserves `shamir_backup_*` "delivered with R2d" — confirmed by that document's open question 6 (`engine-metrics.md:317`); this design asks for the counters at R2a; a naming-schedule deviation, flagged to the metrics owner (names only, 4.8).
4. `format-stamp.md:101` (1.6, entry counts) says `shamir-sync/src` has 6 entries; it has 7 since `format_stamp/` landed. Not edited here; the count there is stale and must not be reused for placement.

## Review

Independent code review findings applied to this document. Each: severity, what was wrong, evidence (`path:line`, crates/-relative), what was changed.

1. **Blocker — retention-pin premise false.** The design treated `RetentionGate` as a truncation pin. It is a write-refusal `AtomicBool` (`wal_retention_refuse_writes`, `retention_gate.rs:12-27`, consulted at `tx/commit/pipeline/phase/commit/entry.rs:643`) with counters; its only floor API is `reserve_recovery_floor` (`retention_gate.rs:101`, used by `tx/recovery/clean_horizon/recovery_floor.rs:8`). The drainer's truncation ceiling comes from the `durable_watermark`/`pending_unsafe`/repair clamp (`tx/drain/drainer/truncate.rs:76-104`); `lock_truncation` (`segment_set.rs:687`) is `pub(super)`. Consequence: a truncation racing copy step D is DETECTED (`Failed(Copying)`), not PREVENTED — every backup racing retention fails. Changed: D4, 1.3 Drain bullet, 3.3 needs, comparison table row C, 4.2 A/G, 4.3 first bullet. The design choice is kept; the new-pin cost is stated.
2. **Blocker — replay rationale wrong.** "Per-version markers" do not exist: `persist_markers` (`drain_step.rs:880`, `commit_phases.rs:851-892`) writes one monotonic watermark + next_tx_id into the repo's `__tx__` store (`tx_access.rs:112-114`); `RepoWalManager::commit` is a no-op (`repo_wal_manager.rs:174-179`); boot recovery replays EVERY retained entry unconditionally (`repo_wal_manager.rs:182-184`; `recover_inflight_v2` `v2.rs:339-383,440-447`), the watermark only seeds the version floor (`v2.rs:357-372`). Idempotency rests on version-keyed LWW writes (Put/Delete `v2.rs:655-718`, unattached tables `v2.rs:85-146`, indexes `v2.rs:160-262`, interner `v2.rs:617-625`; `CounterDelta` NOT applied cold, `v2.rs:155-186` — restore must use cold semantics, not the warm counter path `drain_step.rs:786-812`); orphan repair is changefeed repair only. Changed: 1.3 Drain bullet, 3.3, 4.2 step E (L1 rewritten), section 5 replay/check (replay through boot recovery over the whole WAL; consistency check tolerates post-V*), Q3 (inverted), 6.1.
3. **Blocker — system store/registry silent op-skips.** They have no replay path; catalogue state older than the repo WAL silently drops replayed ops (`v2.rs:97-110,170-181,220-232` warn+Ok on unresolved tokens). Changed: seal barrier MUST capture system store + registry after all repos (4.2 E); restore-time replay fails closed on unresolved tokens (restore-mode flag, 5).
4. **Major — post-V* store files non-convergent.** A copied store file may contain drain effects ABOVE the copied WAL tail V* (replay stops at V*). Changed: seal barrier re-copies and hash-verifies every mutable repo-database file after the barrier flush (4.2 E).
5. **Major — release-plan mismatch.** The release plan §2 recommends logical streaming copy (variant Б) for R2a; this design chooses physical copy + WAL replay. Stated as an owner-level deviation in section 0. Release-plan R2a items not covered, now listed: read-rate limiting; pin-duration metric + bound-by-setting (added to 4.8); per-db schema version in the manifest (added to 4.5 as `schema_version`); secrets/TLS-token-key exclusion; "restore only into an empty data dir" — intentionally not taken (D10 swap-over-existing kept). Header claim adjusted: Q1-Q8 are this document's defaults; release-plan §6 defaults differ and are cross-checked here only.
6. **Major — layout contradiction (fixed twice).** `sync/` and `sync/durability/` are both at 7 entries, so `sync/backup_online/` would be an 8th. The first fix placed the module at `crates/server/shamir-sync/src/backup_online/` on the claim that `src/` has 6 entries; that claim is stale (it was copied from `format-stamp.md:101`, written before `format_stamp/` existed): `src/` has 7 entries, so the first fix was itself an 8th-entry violation, found by a second review. Fixed: 4.1 and plan step 2 now state the real counts of every candidate folder and place the module at `sync/durability/backup/online/` after a mechanical regroup of `backup.rs` and `restore.rs` (Q10 holds the alternative). Evidence: listing of `crates/server/shamir-sync/src/`, `.../sync/` and `.../sync/durability/`.
7. **Major — layer/crate attribution.** `db_instance` is a shamir-engine layer (repo<table<validator<tx<query<instance<db_instance), not shamir-db. The facade entries need widening of `pub(crate)` items to `pub` (retention-gate accessor `retention_gate.rs:89`, `flush_all_history` `durability_access.rs:263`, system-store flush `system_store/store.rs:328-333`, the new pin) — a visibility change declared to rust-guard; plus the pin wiring in the drainer truncate path (tx-layer change). Changed: 4.1, plan steps 1-3, 4.2 B, comparison table.
8. **Major — catalogue mutation during copy unhandled.** Drop/rename are in-memory re-key + catalogue row update only; the repo dir is never deleted on disk (`db/db_management.rs:544-578`; `db_instance.rs:192-194`). Drop mid-copy = benign inert orphan; create mid-copy can spuriously fail restore's consistency check; rename mid-copy unstated. Added Q9 with default: snapshot the catalogue repo-set at the capture barrier; back up only that set; consistency check compares against the captured set.
9. **Minor — citation corrections applied.** Classes: launch.rs boot order ×9 (`:111,:120,:147,:151-158,:398,:406,:415,:424,:453-460,:468` replacing `:109,:118,:120,:125-131,:381-390,:389,:398,:427-435,:442`); restore.rs (`:174`, `:230-243` call `:234`, `:275-312`, `:158-329`; kept `:186-216` with `:203`, `:216-222/:422`, `:251-256`, `:331,:334,:387` (+`#[cfg(unix)]` note), `:407,:475,:500`); backup.rs (module text `:5-6` and `:13-27`, wire-protocol phrase `:29-32`, timestamp `:553-556`/`:555`, dest create `:239` non-atomic + refuse-existing `:190-192`, cache-skip `:603-604`, re-open+sync `:625-628`, dir fsync `:652`, format_version check `:421-426`); durability_access.rs (`:99-117` for the `.shamirwal` naming; `persist_once` call `:280`); handler.rs `:136`→`:670` (kept `:35-53`); token_cli path → `boot/policy/token_cli/commands/create.rs:179`; format-stamp cross-refs (registry finding ≠ stamp Q7; stamp Q2 for external-path deferral, Q1 reworded); TolerateCorruptTail per-Database per `repo.rs:201-204` (1.3, 3.3, findings 2).
10. **Minor — `begin_drain`/`is_drained` are `#[cfg(loom)]`-only** (`writer_drain_barrier.rs:325,:330`); production primitives are `enter_writer` (`:309`) and `drain` (`:355`). 1.3 updated.
11. **Note — verified-OK (no change):** all `segment_set.rs` citations exact; backup/restore/maintenance-lock/registry-no-fsync (`tables_registry.rs:159-170`, `write_atomic` lacks fsync); manifest struct `backup.rs:168-176`; entry counts of `sync/` and `sync/durability/` (both 7; `src/` is corrected in finding 6); fjall 3.1.13 usage-surface claim (Cargo.lock); one-shot seed exporter (`exporter.rs:24-31`); no engine-wide barrier; no HTTP admin maintenance endpoint; metrics names vs `engine-metrics.md:29/:160` (and `:317`); stamp 3.5/5.2/0-deferral/Q13.
12. **Note — stale count in another design (listed, not edited).** `format-stamp.md:101` says `shamir-sync/src` has 6 entries; with `format_stamp/` it has 7. The stamp design is not edited here (see "Findings against other designs", item 4).

**Verdict.** The architecture — option C, two short barriers, manifest v2, stamp reuse — is sound and retained, contingent on the corrected contracts: the truncation pin must be NEWLY built (finding 1) and replay must be specified as unconditional boot-recovery convergence with same-barrier catalogue capture (findings 2-4). Open items Q3 (inverted), Q9 and Q10 (new) and the release-plan deviations (finding 5) remain owner decisions.
