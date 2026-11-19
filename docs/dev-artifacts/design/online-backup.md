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
| D2 | Capture barrier stays short (flush time only); at seal, admission closes THROUGH SETTLE per repository — the seal sample and the WAL/store re-copy run inside that window — bounded by the cooperative 8 s per-repo deadline; writers wait up to 15 s, then a typed retryable refusal | 4.2 |
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
* **R3 Writer-block bound.** The FIRST copy pass runs with writers running; writers are blocked inside the capture barrier and inside the per-repository seal window (admission closed through settle — barrier, seal sample and re-copy). The seal window is bounded by the COOPERATIVE monotonic deadline — 8 s per repository INCLUDING the barrier — checked at barrier step boundaries and during the re-copy between files, I/O chunks/read/hash, and retry waits; in-flight writes/syscalls cannot be interrupted and may overrun. There is NO byte cap by default in the server: the re-copy rewrites whole changed files, not only the delta, so the size of a mutable active WAL/journal/LSM store is not a writer latency bound. The sync protocol retains an OPTIONAL explicit byte cap (cumulative, including retries) in `SealRecopyCap`, but it is not an operator config knob unless code exposes one. Exceeding the deadline is a typed `Failed(Sealing)` plus cleanup — the gate stays closed until the failing step finishes safely and the re-copy guard releases before the gate reopens. A writer arriving during a closed seal window waits up to 15 s (`ADMISSION_REOPEN_WAIT_CAP`) and may then receive a typed retryable refusal; it is never held by the deadline mechanism itself. Measured and asserted (`shamir_backup_writer_block_seconds`, 4.8). Default bound to assert in tests: p99 barrier < 2 s on CI-sized data (6.4).
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
* **Consistency check.** After replay, the staged copy is opened once through `ShamirDb::init` (the same init the next boot would do, `launch.rs:415`) with a read-only consistency pass: catalogue rows ↔ repo directories ↔ the captured repo-set (Q9, not the live one); a mismatch refuses the restore. Strict restore refuses store contents or WAL **above** the recorded point `V*` — an above-point WAL segment or a store version above `V*` is a typed refusal with NO trim (corrective delta, 2026-11-19: supersedes the earlier "must TOLERATE a post-V* store" wording; the guide's claim that such a state is impossible by construction is likewise corrected). A store state at or below the point is NOT treated as corruption when the WAL covers it up to the point. Replay must be run with cold-recovery semantics (above). (Same fail-closed bias as the stamp gate, `restore.rs:180`.)
* **Stamp refusal.** Unchanged: `restore_snapshot_gate` (`restore.rs:180`) on the snapshot's copied stamp; newer epoch refused, older allowed for rollback (`format-stamp.md` 5.2). The manifest `format_epoch` is cross-checked against the copied stamp — a mismatched manifest/stamp pair is refused as corrupt.
* **Clean-machine restore.** `data_dir` absent: steps 3-7 of `restore.rs` already handle the fresh target (`restore.rs:303-310`). For an online copy the replay step runs on the staged dir before the single rename; the machine needs no prior state.
* **Offline verify (R5).** `verify` extends `verify_manifest` (`backup.rs:411`) to: parse v2; check `kind`; re-hash every `files` entry and every WAL segment; check WAL seq contiguity per repo against the `wal` listing; report `format_epoch`, `binary_version`, `consistency` positions; refuse unmanifested files as today (`backup.rs:104-108`). No engine open — pure file inspection, runs on any machine with the binary.

## 6. Tests and acceptance (CI only)

All tests run in the PR's CI via `./scripts/test.sh` scopes (`@server`, `@storage`, `@e2e`); nothing runs locally (CLAUDE.md policy).

* **6.1 Property test under continuous write load (the gate test).** Boot a server (e2e harness style), run N writer clients that continuously insert and record each **acknowledged** write in an in-memory oracle, start an online backup mid-stream, keep writing until it completes, stop the server, `restore` the snapshot to a fresh `data_dir`, boot, and read back: (a) every oracle entry acked before the capture barrier is present with its acked value (R2); (b) the restored state equals the live state at the recorded `wal_position` for a probe set of keys (R1); (c) no torn/partial record anywhere. Replay-convergence requirement L1 is exercised here by construction (the copy races the drainer): the property tested is convergence under UNCONDITIONAL replay over a cross-instant file set, plus no silent table-token skips (Q3, inverted). Crate: `shamir-e2e` (server boots live there per CLAUDE.md) or `shamir-server` integration tests; sizes per 6.4.
* **6.2 Writer-block bound.** Same harness: measure capture/seal barrier durations via the metrics histogram; assert p99 < bound on CI-sized data (R3). Includes a compaction-pressure variant (larger batches forcing fjall flushes during the barrier).
* **6.3 Failure injection per protocol step.** Injected at each stage of 4.2 using the existing seam style (`segment_set.rs:291,296,309` precedent): fail during A (pin held → assert released on drop), B (flush error → no output dir), C/D (copy error mid-file, deleted source file, Windows sharing violation, hash mismatch on re-pass → source untouched, output removed), E (seal error → Failed(Sealing), WAL consistent), F (manifest write failure → no success report), G (cancel at each boundary → cleanup). Each asserts R4 with a before/after directory snapshot.
* **6.4 OS differences and CI feasibility.** Full-tier matrix runs Ubuntu/Windows/macOS (CLAUDE.md CI tiers; weekly schedule already covers `shamir-server` + `shamir-e2e` on all three). Windows-specific: sharing-violation retry path and rename-over-open-file behaviour; Linux: directory fsync paths; macOS: no additional assertion beyond the shared protocol. CI-sized budgets: data directory ≤ 576 MiB at quiet shutdown (the gate fixture opens eight Fjall journals, each created at 64 MiB logical length and not shrunk on drop, plus the 16 MiB fixture pad, the retained WAL allowance and a 16 MiB margin; the figure is a ceiling that an extra journal or a nested copy would trip, not a per-write cost), WAL ≤ 32 MiB retained, whole gate test ≤ 3 minutes per OS (within the 180 s per-test default scaled by the harness splitting boot/backup/restore phases into separate tests sharing a fixture — if a single test exceeds the budget it is split; the nextest budget stays untouched).
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

### Step 1 findings (2026-11-17), revised into the settled capture/seal design

**Scope and disposition.** Documentation-only revision of the previous Step 1
findings into a settled design. Since those findings were written, TWO of the
prerequisites LANDED: (1) the WAL truncation pin
(`engine/shamir-engine/src/tx/drain/truncation_pin.rs`,
`truncation_pin_guard.rs`, `truncation_lease.rs`) with its drainer wiring
(`tx/drain/drainer/truncate.rs:145-150`), and (2) the online copy engine and
protocol in `server/shamir-sync/src/sync/durability/backup/online/` with the
port trait `OnlineBackupPort` (`wal_capture.rs:64-87`) and the full steps A-G
state machine (`protocol.rs:232-266`). The capture/seal/replay facade in
shamir-engine is NOT built; this subsection settles its two blocking hazards
and specifies it. No source, test, CI or `.rust-guard` file changes here.
Compilation, builds and tests run on GitHub CI, not on this machine; no
build or test was run for this revision. Everything below is either
SOURCE-OBSERVED (a freshly read production call site, cited) or PROPOSED
(a decision of this document); INFERRED marks a scheduling counterexample
derived from source, not a reproduced hang or corrupted artifact.

**What the landed pieces already answer (reconciliation with the old
findings and the Review).**

| Old open item | Status | Evidence |
|---|---|---|
| Truncation pin (D4, finding 1) | LANDED — CAS/ArcSwap registry, ceiling clamp, drainer wired after history flush | `tx/drain/truncation_pin.rs:47-69,71-88`; `drainer/truncate.rs:148` |
| Copy engine, manifest v2, progress, self-verify, permit (4.1, 4.4, 4.5, step F) | LANDED as library over the port | `online/copy_engine.rs:199`, `online/manifest.rs:136,156,204`, `online/progress.rs`, `online/protocol.rs:233` (`BackupPermit` `protocol.rs:25-49`) |
| Protocol steps A-G incl. seal re-copy and cancellation cleanup | LANDED, but over a port with NO implementation — safety of step E depends on the gate below | `online/protocol.rs:268-433`; cleanup `protocol.rs:251-265` |
| `shamir-metrics` "PROPOSED new crate, not an existing dependency" (4.8) | STALE — the crate now exists at `foundation/shamir-metrics` | crate listing, this revision |
| Capture/seal/replay facade + the two hazards below | STILL OPEN — the port has no shamir-engine implementation, by design until the gate lands | `wal_capture.rs:1-4` (doc: "Step 1 of the plan implements this port in `shamir-engine`") |

#### 1. Hazard analysis (kept, re-verified against the current tree)

All call chains below were re-read after the engine split into
`shamir-engine` / `shamir-engine-repo` / `shamir-engine-table`; stale
line numbers from the previous revision are corrected in place.

##### Hazard 1 — capture/seal draining while holding repo admission can self-deadlock

**SOURCE-OBSERVED chain.**

* `RepoCore::flush_buffers` calls `drain_all` at
  `tx/commit/core/durability_access.rs:383-396` (call `:394`);
  `Drainer::drain_all` loops `drain_step` at
  `tx/drain/drainer/truncate.rs:178-190` (`drain_step` call `:181`);
  `drain_step` first locks `replaying` at
  `tx/drain/drainer/drain_step.rs:52`.
* Interner branch: the non-data-op fallback calls `replay_v2_op` at
  `drain_step.rs:364-376` (call `:365`) → `tx/recovery/v2.rs:59-60` acquires
  `repo.write_domain().admit()` BEFORE dispatch (`:63-68` boxes the admitted
  body) → `engine-repo/journal/changelog_store/write_domain.rs:135-160`
  acquires the domain mutex (`:141`) **before** checking the reservation
  (`:154-159`; explicit rationale comment `:135-138`). `RepoWritePermit`
  owns that guard for its lifetime (`write_domain.rs:61-64`).
* Orphan-posting branch: `drain_step.rs:601-620` calls
  `tbl.begin_write_barrier(SCHEMA_ACTIVATION)` for `orphan_tables` members
  (call `:617-618`) →
  `engine-table/table_manager/barriers/ddl_barriers.rs:80-89` (`admit()`
  call `:84`, permit retained in the returned guard, comment `:75-79`) →
  `engine-table/table_manager/admission/domain/barrier/write_domain_barrier.rs:39-47`
  → the same domain `admit()` with mutex-before-reservation.
* Background entry: the drainer background loop calls `drain_all` at
  `truncate.rs:297` — hazard 1 is not only a call-from-capture concern.

**INFERRED counterexample (unchanged in substance).** If capture/seal
retains the repo permit and then calls the ordinary flush path with an
eligible overlay-merge WAL op pending, it awaits a second acquisition of a
mutex it already holds; the reservation check sits after the lock
(`write_domain.rs:135-138`) so it cannot turn this into prompt rejection.
Eligible orphan postings follow the second chain. Cross-cycle variant: a
background pass owning `replaying` waits for the domain permit held by
capture, while capture waits for `replaying`.

**Why the landed pin does not help.** The pin registry admits deletion
leases and clamps their ceiling (`truncation_pin.rs:47-88`); the drainer
consumes the lease only at deletion, after checkpoint/history-flush
(`truncate.rs:145-150`, lease `try_begin_truncate` `:148`). It supplies
neither a `RepoWritePermit` nor a replay-lock handoff, and commit never
touches the registry (`truncation_pin.rs:9-12` module doc).

**PROPOSED remedy — hazard 1 (settled).** The commit-admission gate of the
next section provides phase A (close leaders, await zero) without holding
replay or domain locks; the drain then runs through the ordinary production
paths AFTER leaders are zero and NO repo permit is held, and capture pauses
replay with an owned RAII pause guard over the existing `replaying` mutex
(never calling `drain_all`/`drain_step` recursively while paused). The
admitted building blocks already exist — the private admitted op dispatcher
`replay_v2_op_admitted` (`tx/recovery/v2.rs:72-85`) and the public admitted
table barrier `begin_write_barrier_admitted`
(`write_domain_barrier.rs:53-64`) — but no admitted drainer; threading a
supplied permit through every re-admitting drain/replay/barrier/repair site
remains the PROPOSED refactor, outside pin scope. Capture fails closed on
unsettled or failed replay.

##### Hazard 2 — interner-only (overlay-only) leader commits bypass every table freeze

**SOURCE-OBSERVED chain.**

* Entry: `commit_tx` → `commit_tx_with_options` at
  `tx/commit/pipeline/phase/commit/entry.rs:581-583` (import `entry.rs:9`,
  fn `tx/commit/acknowledgement/tail.rs:23-28`) → `commit_tx_with_kind`
  (`tail.rs:33`, gate initialization `:52-66`) → `commit_tx_inner` at
  `entry.rs:600` → `repo.tx_gate()` `:629` → `pre_commit_prelock` call at
  `entry.rs:671`. The prelock merges a nonempty overlay into the repo base
  interner BEFORE any table work (`tx/preparation/pre_commit/prelock.rs:84-95`).
* The table-drain loop enters writer-drain participation only for
  `touched_table_tokens(tx)` (`prelock.rs:270-289`, `enter_writer_drain`
  `:278`); the helper covers write-set, index, counter, staged-vector and
  vector-delete tokens but **not** `interner_overlay`
  (`prelock.rs:828-863`). `TxContext::is_empty` includes
  `interner_overlay.is_empty()` (`storage/shamir-tx/src/tx/tx_context.rs:642-649`),
  so an overlay-only tx is not the empty fast path.
* Only after prelock does `CommitDriver::new` enter the tail counter
  (`entry.rs:694-695`, `repo.commit_tail().enter()` `:695`) and dispatch the
  lock-free branch (`entry.rs:703-727`) → `pre_commit_locked_validate`
  (`tx/commit/pipeline/phase/commit/legacy_and_lockfree.rs:294-298`) →
  empty check, version assignment, `wal_ops_from_tx`
  (`tx/preparation/pre_commit/locked.rs:150-174`; assignment `:167-169`) →
  `InternerOverlayMerge` emission (`entry.rs:462-493`, push `:493`) → WAL
  queue/completion (`legacy_and_lockfree.rs:340-356`).
* `quiesce_commits` only times out `wait_zero()`
  (`repo/instance/repo_instance/lifecycle.rs:51-62`); `CommitTailBarrier`
  is an unconditional counter + armed-notify zero loop
  (`tx/commit/pipeline/driver/commit_tail_barrier.rs:12-32`), and its own
  doc requires admission to be stopped separately (`:21-22`).
* **Follower apply is a SEPARATE commit entry, not `commit_tx`.** The
  receipt pipeline is the only apply protocol (PR6b-3):
  `apply_replicated_batch` at `tx/replication/apply_replicated.rs:323-330`
  → `apply_batch_receipt`
  (`tx/replication/follower_durability/receipt/apply.rs:46`), which holds
  the write-domain permit for its whole run (`apply.rs:96-140`, `admit()`
  `:134-136`, reservation twin `:112-119`) — it has NO prelock, NO tail
  counter and NO table-drain participation. A follower-apply mutation is
  therefore invisible to every mechanism hazard 2's freeze relies on.

**INFERRED counterexample (unchanged).** With every table frozen, seal can
observe a zero tail count and a new overlay-only leader can still enter
prelock (uncounted — the counter is entered at `entry.rs:695`, after
prelock), change the base interner, assign a version and append WAL while
the capture/seal guard is held. On a follower, the same slip exists via the
receipt pipeline with even fewer shared mechanisms.

**Why the landed pin does not help.** The pin controls only the deletion
ceiling (`truncation_pin.rs:71-88`); neither the leader entry, the
touched-token calculation, version assignment nor the WAL append consults
it. The write-domain doc states the leader commit path acquires
nothing there (`engine-repo/journal/changelog_store/write_domain.rs:8-11`).

#### 2. Commit admission barrier (PROPOSED — settled)

**Primitive: `CommitAdmissionGate`, lock-free.** One `Arc`-shared per-repo
gate with a SINGLE `AtomicUsize` state word:

```text
bit 62      CLOSED      (gate closed for admission)
bit 63      CLOSE_TAKEN (one exclusive RAII close owner holds the right to reopen)
bits 0..62  ACTIVE      bounded active-leader count (leases)
```

* **Admission (`acquire`)**: `load(Acquire)`; if `CLOSED` set → wait per
  the waiting policy below, then retry; if `ACTIVE == ACTIVE_MAX` (reject
  overflow well below the 62-bit ceiling — `ACTIVE_MAX = i32::MAX as usize`)
  → `Err(AdmissionOverflow)`; else `compare_exchange(current, current + 1, AcqRel, Acquire)`;
  on success synchronously construct the RAII `CommitAdmissionLease` before
  any `.await` or side effect. No new `Mutex`/`RwLock` anywhere on the
  commit or truncate paths (CLAUDE.md lock-free ideology; the pin is the
  in-tree precedent, `truncation_pin.rs:5-12`). Waiters park on a `Notify`
  armed before the count recheck — the exact `CommitTailBarrier::wait_zero`
  pattern (`commit_tail_barrier.rs:22-32`); a yielding recheck is the
  fallback.
* **Waiting policy (settled).** A leader that finds the gate closed WAITS
  for reopen on the gate's `Notify` (armed before the recheck), bounded by
  the barrier cap of the step-E resolution below; past the cap it is refused
  with a typed retryable error (`AdmissionClosed`) and no side effect.
  Client commits therefore see a short stall during a barrier, not an error,
  and a nested acquisition on a task that already holds a lease (which would
  otherwise wait for a close that waits for it) ends in a bounded refusal,
  not a deadlock. The bounded wait is cancel-safe because nothing is
  mutated before the lease exists. Follower apply refuses immediately (Q13).
* **Release (`CommitAdmissionLease::drop`)**:
  `fetch_sub(1, Release)` + `notify_one` — never a "check closed, then
  increment a second atomic", which admits the check/increment race.
* **Close (`close()`)**: single owner — CAS `CLOSE_TAKEN` 0→1 (AcqRel;
  competing closer gets `Err(AdmissionBusy)`); then `fetch_or(CLOSED)`;
  then await zero via armed-notify recheck. **Reopen**: only the close
  owner (guarded by `CLOSE_TAKEN`), CAS `(CLOSED|ACTIVE)` → `ACTIVE`
  (Release), then clears `CLOSE_TAKEN`; the count is preserved across
  close/reopen. Any generation scheme must be part of the proof, not a
  second independent flag.

**(i) ENTERED on every commit path.**

| Path | Entry point | Note |
|---|---|---|
| Ordinary + overlay-only leader commits | inside `commit_tx_inner` at `entry.rs:600`, immediately after the wound/expiry early exits and BEFORE `repo.tx_gate()` (`:629`) and `pre_commit_prelock` (`:671`) — earlier than the tail counter (`:695`) | the lease must be acquired before the acknowledgement wrapper initializes `tx_gate` (`tail.rs:52-66`) so a cancelled wait has no side effect |
| DDL leaders | DDL pays write-domain admission today (`engine-table/table_manager/admission/domain/barrier/write_domain_barrier.rs:8-9`); any DDL that commits rows through the pipeline already passes `commit_tx_inner` and acquires the gate there | no separate gate acquisition at the DDL barrier |
| Follower (replicated apply) | SEPARATE entry — at the top of `apply_replicated_batch` (`apply_replicated.rs:323`), before `apply_batch_receipt`'s write-domain admission (`receipt/apply.rs:134-136`) | the receipt pipeline does NOT route through `commit_tx`; without the gate here a follower-side mutation could run during a capture barrier |

**(ii) LEFT (release points).** The lease moves WITH the continuation, never
sits on a caller stack: `CommitDriver::new` already transfers the suspended
future on drop (`commit_driver.rs:79-114`) and the AsyncIndex tail handoff
enters-before-spawn (`legacy_and_lockfree.rs:139-163`) — the lease follows
both, including the separately spawned panic cleanup (`commit_driver.rs:82-96`).
Release only after publication, every mutation-capable tail (detached and
spawned AsyncIndex materialization), and error/panic cleanup finish; on
follower apply, after receipt publication and `durable_tail`. Cancellation
after a successful CAS drops the lease at the abort point.

**(iii) Interaction with the drainer and the landed truncation pin — chosen
order (per repo).** Pins are already held from protocol step A
(`protocol.rs:292-293`, port contract `wal_capture.rs:69-71`) — they are
held from BEFORE the gate closes, so WAL deletion is impossible throughout.
Then: close gate → await zero leases (a fixed commit horizon) → with NO repo
permit held, let any in-flight background drain finish → acquire the RAII
pause guard on `replaying` → recheck pending work under the guard → final
admitted drain + flush → sample `V*` → re-copy under pause+pin (see the
step-E resolution below) → drop pause → reopen gate LAST. Why this order:
draining before pausing avoids the recursive `replaying` acquisition that
causes hazard 1; pausing before the re-copy is what makes copied store files
provably contain nothing above `V*`; reopening last keeps the horizon stable
until all capture reads finish.

**(iv) Fail-closed recovery.** The close owner is a synchronous RAII guard
created before any suspension point; it reopens on drop — ordinary error,
timeout, cancellation or unwind. A cancelled or panicked capture therefore
cannot leave a repo closed. Leases vanish with their tasks. A crashed
process releases all state by definition (atomics and the in-memory pin
registry are process-local). A restart must recheck: (a) the gate word is
open with zero leases — trivially true after a crash, asserted in tests
after graceful shutdown; (b) no partial backup destination is adopted — a
directory without a `manifest.json` is refused exactly as today's restore
refuses unmanifested snapshots; (c) cold recovery replays the full retained
WAL (unchanged), so no committed state can be lost by a mid-backup crash
because the pin never truncated below the pinned floor.

#### 3. Facade API mapping (shamir-engine `db_instance` layer ↔ landed port)

The port trait stays in `shamir-sync` (`wal_capture.rs:64-87`); it exists
precisely so `shamir-sync` never depends on `shamir-engine`. The
implementation adapter lives in `shamir-server` (already depends on both)
and calls the new `pub` facade functions in shamir-engine's `db_instance`
layer. `.rust-guard/layers.toml` declares ONLY the shamir-engine layer order
`repo < table < validator < tx < query < instance < db_instance`
(`.rust-guard/layers.toml:6`); shamir-sync is not layer-declared. Adding
`pub` facade items to the top layer of shamir-engine creates NO new edge
and NO new backward dependency — nothing to change in `layers.toml`.

| `OnlineBackupPort` method | Facade function (PROPOSED, shamir-engine `db_instance`) | Owns |
|---|---|---|
| `catalogue()` (`wal_capture.rs:67`) | `catalogue_repos() -> Vec<RepoSource>` (repo name, store dir, optional WAL dir; `in_memory` ⇒ `wal_dir: None`, Q8) | shamir-engine: catalogue read |
| `acquire_pins()` (`:71`) | `acquire_truncation_pins(repos)` — pin per repo via the landed registry | shamir-engine: pin |
| `capture_point(repo)` (`:75`) | `capture_point(repo)` — close gate, admitted flush (`flush_all_history` `durability_access.rs:275-291`, `pub(crate)` → `pub`), report `max_committed` | shamir-engine: gate + flush + position |
| `capture_system_store()` (`:79`) | `capture_system_store()` — `flush_buffers_with_quiesce_timeout` (`engine/shamir-db/src/shamir_db/system_store/store.rs:333`, `pub(crate)` → `pub`) + version | shamir-engine `db_instance` over shamir-db system store |
| `seal_point(repo)` (`:83`) | `seal_point(repo)` — §2 (iii) sequence, returns `V*` | shamir-engine: gate + pause + flush |
| `release_pins()` (`:86`) | `release_pins()` — idempotent guard drop | shamir-engine: pin |
| (no port method — restore side) | `replay_to(repo_path, wal_dir)` — cold `recover_inflight_v2` semantics against an offline staged copy | shamir-engine: recovery; called by offline `restore` (section 5), not by the online protocol |

Layer ownership summary: gate, pause guard, pins, flush, position report and
replay all live in shamir-engine (tx/repo internals below `db_instance`);
protocol, copy, manifest, verification stay in shamir-sync; the adapter and
admin wiring live in shamir-server.

#### 4. CI tests (GitHub CI only — nothing runs locally)

Placement per repo conventions: engine-level concurrency tests in
`engine/shamir-engine/tests/`; server-boot/protocol acceptance in
`devtools/shamir-e2e` (the e2e crate exists at `crates/devtools/shamir-e2e`)
and `server/shamir-server/tests/` (server boot tests live there, e.g.
`tests/seed_recovery_e2e/`).

| Test (PROPOSED) | Crate / path | Proves |
|---|---|---|
| `commit_admission_gate::close_refuses_overlay_only_leader` | `shamir-engine/tests/commit_admission_gate/` | close-then-attempt: an overlay-only leader is refused (or blocks cancel-safely) BEFORE touching the base interner, assigning a version or appending WAL; freezing tables alone is the negative control (hazard 2) |
| `commit_admission_gate::capture_completes_under_concurrent_commits` | same | continuous ordinary commits during capture/seal complete without deadlock, bounded by a test timeout; includes eligible `InternerOverlayMerge` and orphan `IndexPut`/`IndexDel` postings through the `begin_write_barrier` branch (`drain_step.rs:617-618`) (hazard 1) |
| `commit_admission_gate::lease_survives_detached_and_async_tails` | same | no zero-count gap across `CommitDriver` detach (`commit_driver.rs:79-114`) and the AsyncIndex spawn handoff (`legacy_and_lockfree.rs:139-163`) |
| `commit_admission_gate::follower_apply_respects_gate` | same | `apply_replicated_batch` refuses/blocks while the gate is closed (`apply_replicated.rs:323`) |
| `online_backup_seal::cancellation_releases_gate_and_pin` | `shamir-engine/tests/online_backup_seal/` | cancel at every protocol step: pin registry empty (`truncation_pin.rs` state), gate reopened, active count 0, no leaked close owner |
| `online_backup_seal::replay_to_v_star_double_boot` | same | bounded staged WAL (only complete entries ≤ `V*`), cold open twice reproduces exactly the sealed state; no post-V* effects resurrect on either boot |
| `online_backup_gate_e2e` | `devtools/shamir-e2e` (or `shamir-server/tests/`) | full protocol over a live server with writer clients: capture/seal succeed under load, snapshot verifies, restore replays to the recorded consistency point |

Deterministic handshakes (armed notifies at the CAS boundary, park hooks)
instead of sleeps; handshakes at the close-vs-acquire CAS boundary are also
the model-checking candidate.

#### 5. Implementation plan (two PR-sized steps)

**PR A — gate + admitted seal, in `shamir-engine` (must land FIRST, and
BEFORE the tx crate extraction).** Scope: `CommitAdmissionGate` + lease +
close owner (new `tx/commit/pipeline/admission/` module), wiring at
`commit_tx_inner` and `apply_replicated_batch`, lease transfer through
`CommitDriver`/AsyncIndex, the owned `replaying` pause guard, the
`capture_point`/`seal_point`/`replay_to` facade in the `db_instance` layer,
and the gate-aware unit tests above. Touched crates: shamir-engine only.
rust-guard: visibility widenings to `pub` — the facade entries, the pin
registry (`truncation_pin.rs`, already `pub(crate)`), `flush_all_history`
(`durability_access.rs:275`), system-store flush (`system_store/store.rs:333`);
new tx-layer module needs no layer move (still inside `tx`). **Ordering
versus the engine split:** the split design extracts `shamir-engine-tx` in
wave R4, last (`docs/dev-artifacts/design/engine-layering.md:11`, the pair
becomes the crate dependency "at R4" `engine-layering.md:89,181`); no
release-plan wave schedules it earlier (`docs/dev-artifacts/release-1/README.md`
lists R2 waves — online backup — separately). A gate living inside tx commit
code is cheaper to review and moves as one unit at R4, so PR A lands while
tx is still in shamir-engine.

**PR B — port implementation + server wiring, in `shamir-server`.** Scope:
the `OnlineBackupPort` adapter over the facade (in shamir-server, which
already depends on shamir-engine and shamir-sync), connecting the landed
`run_online_backup` (`protocol.rs:233`) to the admin ops of 4.7, restore
`replay_to` integration (section 5), metrics, and the e2e acceptance tests.
Touched crates: shamir-server, plus shamir-db only if a `BatchOp` DTO arm is
added (Q1 unchanged). rust-guard: nothing new — no new dependency edges, no
layer moves; the port pattern keeps shamir-sync engine-free. Independent of
the tx-crate split.

#### Fail-closed recovery and point validation (kept prerequisites)

The previous revision's fail-closed contract is RETAINED as a restore-side
prerequisite and updated to the current code: the production constructor
already mutates on open — `durability_repo_impl.rs:65-79` admits
(`:73-75`), reads identity and runs `recover_v2_inflight_admitted`
(`:79`) before returning; recovery builds the gate from inflight versions
BEFORE replay (`tx/recovery/v2.rs:374`) and applies every sorted entry via
`replay_v2_entry_admitted` (`v2.rs:429-432`); `RepoWalManager::recover`
delegates to group replay with no V* filter
(`storage/shamir-tx/src/tx/repo_wal_manager.rs:169-171`; `commit` stays a
no-op `:163-165`). Permissive scoping today (warn+Ok on missing nonbroadcast
table tokens `v2.rs:93-104,123-133` and index ops `v2.rs:188-199,252-263`;
broadcast table-open failures logged/skipped `v2.rs:225-239,285-293`;
interner delta scope ignored `v2.rs:606-635`; cold `CounterDelta` a no-op
`v2.rs:155-180` vs the warm orphan increment `drain_step.rs:814-838`)
mandates the strict restore contract. Zero progress is not a clean-point
proof (`drain_all` stops at a zero pass `truncate.rs:178-190`; `drain_step`
returns `Ok(0)` on an empty window `drain_step.rs:187-190`, logs/breaks on
op failures `:364-377`, stops finalization at failed tables `:735-752`,
returns the count `:959`; A5 failure conservatively retains unsafe state
`:772-806`). Consequence: restore must preflight (bounded staged WAL,
token/scope validation, post-V* rejection) BEFORE any cold constructor, and
capture must refuse on incomplete durable/settled/coverage evidence — these
remain PROPOSED work, unchanged in substance from the previous revision.

#### Step E re-copy versus R3 — resolved (owner decision, default stated)

Section 4.2 E required re-copying every mutable repo-database file inside
the seal barrier, contradicting R3's "flush time plus a constant". The
EARLIER landed behavior ran the seal re-copy with writers running
(`protocol.rs:338-368`), which was only defensible if copied stores
provably contained nothing above `V*`; the commit store, however, applies
its effects INLINE during commit (overlay merge, WAL append), so a
drain-pause alone freezes the DRAINER, not the commit path — pause-only
is therefore insufficient, and closing admission through the pause is
exactly why a valid new snapshot contains no post-point commits through
seal.
**Default (settled; corrective delta 2026-11-19).** The FIRST copy pass runs
with writers running; the seal barrier then CLOSES ADMISSION THROUGH SETTLE —
the seal sample plus the WAL/store re-copy happen inside that pause — and the
pause is dropped, after which the gate reopens (supersedes the earlier
"gate already open while the hold is handed out" ordering of Step 1d). Bound
policy, with defaults: the 8 s monotonic per-repository deadline INCLUDING
the barrier; there is NO byte cap by default in the server (the re-copy
rewrites whole changed files, not only the delta, so a mutable active
WAL/journal/LSM store's size is not a writer latency bound; the protocol
keeps an OPTIONAL explicit cumulative byte cap, including retries, in
`SealRecopyCap` — not an operator config knob unless code exposes one).
Exceeding the deadline is a typed `Failed(Sealing)` plus cleanup, not an
unbounded block. The clock is cooperative: it is checked at barrier step
boundaries and during the re-copy between files, I/O chunks/read/hash, and
retry waits — in-flight writes/syscalls cannot be interrupted and may
overrun the bound. On a trip the gate stays CLOSED until the failing step
finishes safely and the re-copy guard is released, and only then reopens.
A writer therefore waits (admission closed, no new commit), but after the
EXISTING 15 s wait it may receive a typed retryable refusal instead of
committing.
Alternative (rejected default): move re-copy outside the pause with a
convergence proof — no such proof exists (post-V* store effects are
non-convergent, Review finding 4), so it stays an open question (Q12).

#### Remaining prerequisites (updated)

Settled by this subsection: the primitive, its entry/release protocol, the
seal ordering over the landed pin, the port-to-facade mapping, the layer
placement (no new `.rust-guard` edges), the test specifications and the
two-step plan. Still open before implementation: the admitted-drain refactor
(threading a supplied permit through every re-admitting drain/replay/barrier
site — the existing admitted dispatcher `v2.rs:72-85` and admitted barrier
`write_domain_barrier.rs:53-64` are building blocks, not a drainer);
the fail-closed restore preflight and bounded-WAL staging contract; the
point/coverage certificate (explicit contract for WAL-less direct writes —
the follower receipt pipeline is one such path); fjall compaction is NOT
paused by any of this (file-set consistency still rests on the final hash
pass plus the bounded re-copy, not on a stable file set); catalogue/
system-store/registry capture ordering (Q9/Q7 unchanged); R3's amended
bound needs the cap metric. No correctness claim is made for the landed
protocol until PR A lands: as shipped, the port has no implementation and
the two hazards above are unaddressed.

#### Open questions (new, each with a default; Q11+ continues section 8's numbering)

* **Q11. Gate placement inside the commit pipeline.** Acquire in
  `commit_tx_inner` before `tx_gate()` (default, chosen) vs in
  `commit_tx_with_kind` (`tail.rs:33`). Default: `commit_tx_inner` — it is
  the single funnel both the acknowledgement wrapper and all inner entries
  pass; a wrapper-level acquisition still allows uncounted alternate inner
  entrants.
* **Q12. Re-copy placement (owner confirmation of the default).** Inside the
  drain-pause window with a hard cap and fail-closed refusal (default) vs
  outside with a convergence proof (none exists today). Default as stated
  above; R3's test (6.2) must assert the cap, not just p99.
* **Q13. Follower-apply gate semantics.** Refuse while closed (fail-closed,
  resume worker retries from the durable bookmark) vs block. Default: refuse
  with a typed error; the follower's receipt pipeline is crash-safe by
  design (`apply.rs:14-16` cancel-safety note), so refusal is recoverable.
* **Q14. `replay_to` ownership.** Put the restore-side replay entry in
  shamir-engine's `db_instance` layer (default) vs in shamir-db. Default:
  `db_instance`, consistent with 4.1 and the layer order; shamir-db only
  orchestrates (as the stamp's restore gate already does).
* **Q15. Stale `shamir-metrics` claim in 4.8.** That section says
  `shamir-metrics` is "a PROPOSED new crate"; `foundation/shamir-metrics`
  now exists. Default: a future revision of 4.8 re-points emission at the
  existing crate; no behaviour change decided here (outside this
  subsection's edit scope, flagged to the metrics owner).

#### Step 1d decisions (2026-11-17)

**(a) Seal hold (historical; superseded on gate timing by (d)).** The drain pause survives `seal_point`: the barrier
returns a `SealHold` that owns the `DrainPauseGuard` (`Send + 'static`)
and drops it exactly once, with itself — so on hold drop the gate is
already open (the close owner was dropped before the hold was handed out,
`online_backup.rs` seal_point tail) and writers run again, but the stores
stay frozen because `drain_step`/`drain_all`/the background loop cannot
acquire the drainer's `replaying` lock (`drain_pause.rs`; `state.rs:144-149`)
until the hold drops. Any error path inside the sealed async block drops
the guard there — fail-closed is preserved.

**(b) Port consequence.** Because the pause now outlives `seal_point`,
`OnlineBackupPort` needs either a new "end of seal" method or a
hold-returning `seal_point` signature; shamir-sync is NOT changed in this
step — that port change belongs to R2a-3.

**(c) Observed commit-side backpressure under the hold (historical; the
gate-reopened premise is superseded by (d)).** Commit execution
itself does NOT wait on the drainer: with the gate reopened, a commit
passes admission, performs the Phase-1 interner overlay merge outside
`commit_lock` (`preparation/pre_commit/prelock.rs:46,85-108`), and the
Phase-2.5 `enter_writer_drain` bump (`prelock.rs:270-291`) is a non-blocking
counter on the per-table DDL writer-drain barrier (cross-atomic SeqCst
against a DDL's `drain_writers` load) — it never touches the drainer's
`replaying` lock; table freezes (`MIGRATION_FREEZE` check, `prelock.rs:325-333`)
are a flag read. The commit then appends to WAL and offers the entry
lock-free — `offer` DROPS (never blocks) above the 64K window high-watermark
(`drainer/state.rs:21-24`, `pipeline/phase/commit/legacy_and_lockfree.rs:67`)
and wakes the drainer (`finalize.rs:100`). What DOES wait is the
acknowledgement tail: visibility advances only via the drainer's replay
`mark_durable` (`drainer/drain_step.rs:890`), which is blocked for the
whole hold, so `acknowledge_known_outcome` (`acknowledgement/tail.rs:82`)
observes `Pending` — a `VisibilityWait::Strict` caller parks on the gate's
durable-progress `Notify` with NO budget (`visibility_impl.rs:57-67`,
`budget = None`), i.e. it waits unboundedly for a full drain window that
cannot open until the hold drops; a `Bounded` caller returns `Pending` at
its timeout, and the soft undrained-gap brake
(`apply_backpressure_with_budget`, `tail.rs:299-372`) gives up after
`BACKPRESSURE_MAX_WAIT` = 5 s (`tail.rs:378`) and proceeds (data already
committed and visible in overlay; only the durability promise relaxes).
So: overlay-only and `Bounded` commits proceed with degraded
acknowledgement; `Strict` commits block for the duration of the hold.

**(d) Corrective delta (2026-11-19): admission closed through settle.**
Supersedes (a)/(c) on one point and is the landed note on gate timing:
the admission gate is NOT already open when the hold is handed out — it
stays CLOSED through settle (seal sample and WAL/store re-copy run inside
the pause), and the gate reopens only after the pause guard is dropped.
The bound policy (8 s per repository cooperative deadline, NO byte cap by
default in the server, cooperative clock) and the resulting typed retryable writer refusal are stated once
in Step E above — see it instead of a repeat here. The reserved
`shamir_backup_writer_block_seconds` gauge now records the latest closed
window per phase (`capture|seal`), INCLUDING failed holds, and is absent
until the first window; `shamir_backup_pin_seconds` and
`shamir_backup_wal_segments_copied_total` remain not exported.
