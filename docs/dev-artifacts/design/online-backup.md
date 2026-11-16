# Online Backup: Consistent Copy Without Stopping the Server

- Status: design (release-1 direction R2a)
- Brief: `docs/dev-artifacts/prompts/release/12-r2a-online-backup-design.md`
- Plan: `docs/dev-artifacts/release-1/README.md` (wave 2), `docs/dev-artifacts/release-1/02-online-backup-and-encryption.md` (sections 1, 2, 6)
- Related: `docs/dev-artifacts/design/format-stamp.md` (sections 3.5, 5.2 — backup/restore stamp behaviour), `docs/dev-artifacts/design/engine-metrics.md` (metric names)
- This document changes no code, test, CI or `.rust-guard` file. OBSERVED = a fact in the current code, cited with `path:line`. PROPOSED = a decision of this document. Paths without a `docs/` prefix are relative to `crates/`.
- Boundaries: point-in-time restore is R2b, encryption is R2c, scheduling is R2d. This document only leaves their extension points.

## 0. Summary and decisions

Today backup is stop-and-copy: the operator stops the server and runs
`backup --to`, which recursively copies `data_dir` with no lock and no
coordination with a writer (`server/shamir-sync/src/sync/durability/backup.rs:3-6`,
`backup.rs:218`; no `maintenance_lock::acquire` in the file — the lock helper
exists at `server/shamir-sync/src/sync/durability/maintenance_lock.rs:9-30`
but is not called from backup). The module doc states why: fjall exposes no
file-level backup API, and its journal recovery
(`RecoveryMode::TolerateCorruptTail`) recovers to the last complete batch, so
a copy taken while the writer is paused is consistent (`backup.rs:6-24`).
That argument does not hold while a writer runs: a concurrently copied
directory can mix files from different instants, and the WAL — the only
component that knows the commit order — is rotated and truncated under the
copy.

This design adds an online backup built from the pieces the code already
has: a **short write barrier** (per-repo flush + drain quiesce), a **WAL
retention pin** (`RetentionGate`, `engine/shamir-engine/src/repo/journal/backend/durability_handle/retention_gate.rs:12`), a **file copy** of
store directories plus retained WAL segments, and a **restore-time WAL
replay** through the existing drainer. The writer-block bound is two short
barriers (point capture, point seal), not the copy duration.

Decisions (owner-level ones repeated in section 8):

| # | Decision | Where |
|---|---|---|
| D1 | Recommended option: copy + WAL replay (option C of section 3) | 3.6 |
| D2 | Two short write barriers (capture, seal); writers blocked only for flushes, never for the copy | 4.2 |
| D3 | Consistency point = per-repo (and system store) highest fully-captured WAL version recorded in the manifest | 4.3 |
| D4 | WAL retention pinned below the seal point for the whole copy via `RetentionGate` | 4.4 |
| D5 | Online backup is a server-side operation; the CLI `backup --to` stays offline-only and is not given a `--online` flag in R2a | 4.7 |
| D6 | Entry point: new admin `BatchOp` variants dispatched by `execute_admin` (wire-protocol change), gated by the existing `is_superuser` bridge | 4.7 |
| D7 | Manifest `format_version` bumps to `2`; v1 snapshots keep restoring | 4.5 |
| D8 | Snapshot layout gains a top-level `payload/` marker field (`encryption: "none"`) as the R2c extension point; no encryption is designed | 4.6 |
| D9 | A second concurrent online backup is refused (one permit per server); an offline `backup --to` on a live dir is not detected in R2a (documented residual risk) | 4.4 |
| D10 | Restore of an online copy replays WAL; restore of an offline v1 copy is byte-identical to today (no replay) | 5.1 |

What R2a does versus defers: online backup protocol, manifest v2, online
restore + replay, offline `verify` of both kinds, tests, metrics names,
guide section. Defers: PITR to an arbitrary instant (R2b), encryption at
rest (R2c, layout extension point only), scheduled/continuous backup
(R2d), backup of repositories whose catalogue path lies outside `data_dir`
(same scope as the stamp's D1/Q1 — `format-stamp.md` section 8 Q1).

## 1. Observed facts

Everything in this section is OBSERVED.

### 1.1 Current backup

* Entry: `backup(from, to)` (`server/shamir-sync/src/sync/durability/backup.rs:218`). It rejects a destination inside the source (`backup.rs:225`, helper `:272`), observes the format stamp read-only via `observe_stamp_for_backup` (`backup.rs:229`), creates `to/<YYYYMMDD_HHMMSS>/` (`backup.rs:231-238`, timestamp format `:546-568`), copies recursively, writes the manifest, fsyncs the destination directory once (`backup.rs:246-256`).
* Copy: `copy_dir_recursive` (`backup.rs:591`, `pub(crate)` — reused by restore). Skips the top-level `cache/` subtree only (`backup.rs:599-601`); symlinks/sockets/fifos are skipped with a warning (`backup.rs:632-634`). Every copied file is re-opened and `sync_all`'d (`backup.rs:620-627`); every directory written gets a directory fsync, log-only on failure (`backup.rs:637-641`). `fs::copy` on a file that a writer is appending to copies whatever bytes are visible at that moment — there is no coordination with any writer.
* Manifest: `Manifest { format_version: u32, created_at_unix_ns, files }` (`backup.rs:169-173`), const version `1` (`backup.rs:176`), written by `write_manifest` (`backup.rs:330`) after hashing every just-copied file with streaming SHA-256 (`hash_file_streaming`, `backup.rs:58`; entries `ManifestFileEntry { path, sha256, size_bytes }`, `backup.rs:156-165`; collection `:366`). Paths are forward-slash normalized for cross-platform verify (`relative_slash_path`, `backup.rs:398`).
* Verify: `verify_manifest` (`backup.rs:411`) rejects a missing/invalid manifest, a wrong `format_version` (`:425-430`), cache paths (`:433`, helper `:490`), unsafe (`..`/absolute) paths (`reject_unsafe_manifest_path`, error variant `backup.rs:114-119`), duplicate entries (`:123-127`), checksum/size mismatch (`:97-103`), and files on disk not listed in the manifest (`:104-108`). It re-hashes every listed file — the offline verifiability primitive this design reuses.
* Stamp observation: `BackupStampObservation` attached to `BackupReport` (`backup.rs:150-151`); backup never refuses and never stamps (design `format-stamp.md` 3.5, decision D10).
* The file copy brings `format.stamp` along as an ordinary file (no special handling; `backup.rs:227-229`).

### 1.2 Current restore

* Entry: `restore_with_probe(from, data_dir, force, probe)` (`server/shamir-sync/src/sync/durability/restore.rs:158`). Steps, all observed in the function body and its doc comment (`restore.rs:8-57`):
  1. Liveness probe unless `--force` (`restore.rs:164-166`) — the wrapper opens the live `server_meta` store and relies on fjall's own exclusive lock (`restore.rs:9-17`).
  2. `verify_manifest(from)` before touching `data_dir` (`restore.rs:169`).
  3. Stamp gate `restore_snapshot_gate(from, &EpochPolicy::compiled())` (`restore.rs:180`) — newer/corrupt/newer-layout refuse; absent/older allowed (design `format-stamp.md` 3.5, 5.2).
  4. Stage to a sibling `<data_dir>.restore_tmp_<stamp>` created with atomic `fs::create_dir` (TOCTOU-safe, `restore.rs:186-216`); copy failure cleans the stage dir (`cleanup_staged_temp_dir`, `restore.rs:422`).
  5. Session tickets invalidated **in the staged copy** via `open_and_invalidate_users` (`restore.rs:248-267`, helper `:407`); failure cleans the stage dir.
  6. Restore marker `.shamir_restore_epoch` (const `restore.rs:331`, writer `:334`) written into the staged copy before any swap.
  7. Atomic swap: `data_dir` → `.pre_restore_backup_<ts>` sibling (kept as the rollback path, `restore.rs:251-256`), then staged dir → `data_dir`, with directory fsyncs (`fsync_dir` `restore.rs:475`, Unix-only no-op fallback `:500`; strict private variant `sync_restore_directory` `:387`). Failure of the second rename triggers best-effort rollback and returns `SwapPartialFailure` or `SwapFailedRollbackSucceeded` (`restore.rs:264-306`).
* Restore never opens the engine for replay: the copy is trusted as-is; WAL recovery happens later inside the next server boot (`server/shamir-server/src/engine/server/server_launcher/launch.rs:389` calls `ShamirDb::init_with_durability`, and per-repo WAL recovery runs inside `add_repo_with_durability` — `engine/shamir-db/src/shamir_db/shamir_db/core.rs:461` per `format-stamp.md` 0/1.3).
* CLI wiring: `backup` at `server/shamir-server/src/main.rs:372`, `restore` at `main.rs:383`; both dispatch without booting the server (`main.rs:327` comment).

### 1.3 Storage facts an online copy depends on

**Fjall backends.** All durability goes through one seam: `persist_sync_all(db)` → `Database::persist(PersistMode::SyncAll)` (`storage/shamir-storage/src/stores/storage_fjall/persist.rs:25-27`). Repo commits apply an atomic `Database::batch()` and persist once (`storage/shamir-storage/src/stores/storage_fjall/repo.rs:114-119`); `persist_once` is one `SyncAll` covering every keyspace of that repo database (`repo.rs:201-210`). Per-store `flush`/`flush_drain` exist (`storage/shamir-storage/src/stores/storage_fjall/store.rs:289,303`). The repo backend opens the database directory directly (`storage/shamir-storage/src/stores/storage_fjall/repo.rs:126`) — the directory tree of journal + LSM segment files described in `backup.rs:9-11` is the thing a file copy must capture. Fjall owns compaction internally; the engine never sees file-level replacement, which is why no engine-level file list exists today.

**WAL.** One directory per repo: `<repo path>.shamirwal/` (`engine/shamir-engine/src/tx/commit/core/durability_access.rs:56-64`), segments `NNNNNNNN.wal` with `WMT3` (legacy `WMT2`) sidecars `NNNNNNNN.meta` and temp `.meta.tmp` (`storage/shamir-wal/src/entry/segment_meta.rs:10-39` per `format-stamp.md` 1.1). `SegmentSet` holds one active + sealed segments behind one mutex; sealing happens on rotation when `max_bytes` is exceeded (`storage/shamir-wal/src/segment_set/segment_set.rs:4-9,467 seal_and_rotate`); appends are batched (`append_batch`, `segment_set.rs:328`); `sync`/`sync_active` (`:706,:715`) fsync the active segment; `replay` (`:562`) reads all segments; `truncate_below(durable_version)` deletes whole sealed segments once every record in them is below the durable horizon (`:590`) — this is the retention mechanism; `has_truncatable` (`:680`) and `lock_truncation` (`:687`) exist, and truncation-failure injection seams are already in place (`:291,:296,:309`). `max_committed` (`:730`) exposes the highest appended version.

**Drain (WAL → stores) and retention.** The drainer applies WAL entries to stores in `drain_step` (`engine/shamir-engine/src/tx/drain/drainer/drain_step.rs:51`), persists per-version markers (`drain_step.rs:880`, `persist_markers`), and repairs orphans — the engine already tolerates "entry applied to a store but marker missing" (`drainer/mod.rs:17-39` doc; `drainer/orphan_repair/repair.rs`). The repo exposes a retention gate (`engine/shamir-engine/src/repo/journal/backend/durability_handle/retention_gate.rs:12`; accessor `:89`), and the commit layer holds it (`engine/shamir-engine/src/tx/commit/core/durability_access.rs:5`).

**System store, registry, launcher order.** The system store is a fjall directory `shamir_db_meta.redb/` under `data_dir` (`server/shamir-server/src/engine/server/server_launcher/launch.rs:381-390`; `engine/shamir-db/src/shamir_db/system_store/store.rs:62-67,153-214` per `format-stamp.md` 1.1); it carries the database/repository/table catalogue rows (`core.rs:398-413`). The wire-tables registry is a single file written by `write_atomic` — tempfile + `persist`, **no fsync** (`server/shamir-server/src/engine/tables_registry.rs:159-170`; opened `launch.rs:442`). Boot order (`launch.rs`): `create_dir_all(data_dir)` `:109` → data-dir lock `:118` → `ServerMetaStore::open_or_init` `:120` → users/counters `:125-131` → audit → `ShamirDb::init_with_durability` `:389` (WAL recovery inside) → `consume_restore_marker` `:398` → default repo `:427-435` → registry `:442`.

**Data-dir lock.** `.shamir.lock`, `fs4` exclusive try-lock, PID written (`server/shamir-sync/src/sync/durability/maintenance_lock.rs:9-30`, open `:22`, `try_lock_exclusive` `:23`). It serializes server instances and maintenance opens (`token_cli/commands/create.rs:179`), but `backup` never takes it (`backup.rs` has no `acquire` call) and neither does `doctor`/`access-tree` (finding 1 of `format-stamp.md` review).

**Torn-write hazards a concurrent copy sees.** While a copy runs, a writer can: append a partial batch to the active fjall journal (recovered by `TolerateCorruptTail` per `backup.rs:15-24` — per-keyspace only); compact/rename/delete fjall SST files between the copy's `read_dir` and `fs::copy` (a file deleted mid-copy fails the copy; a file created after its directory was visited is silently missed); rotate and then truncate WAL segments the copy has not yet reached (silent hole in the copied history — the worst case, because it removes commits the store files may not contain); replace `NNNNNNNN.meta` sidecars and the registry file (registry: non-fsynced overwrite, `tables_registry.rs:159-170`). None of these is detected by today's backup; today they cannot happen because the server is stopped.

**Existing quiesce points.**
* Per-table writer drain: `WriterDrainBarrier` (`engine/shamir-engine/src/table/support/guards/writer_drain_barrier.rs`) with `enter_writer` `:309`, `begin_drain` `:325`, `is_drained` `:330`, `drain` `:355`; held per table manager (`engine/shamir-engine/src/table/table_manager.rs:290,712`). There is no engine-wide barrier object today — a cross-repo barrier is composition, not a primitive.
* Flush: `flush_all_history` drains every table's history version log and persists the repo database once (`engine/shamir-engine/src/tx/commit/core/durability_access.rs:263-283`, per-store flush `:272`, `persist_once` `:281`).
* Clean shutdown: `serve` awaits shutdown then `handle.shutdown_with_report` under a deadline (`server/shamir-server/src/boot/runtime.rs:21-66`; `server_handle.rs:180,193`). The server scheduler runs background GC/persist ticks with a shutdown `Notify` (`server/shamir-server/src/boot/scheduler.rs:11-12,32-42`).

**Admin/auth surface.** `execute_admin` dispatches `BatchOp` variants (create/drop/rename db/repo/table, indexes, …) (`engine/shamir-db/src/shamir_db/execute/core/admin_dispatch.rs:24,33-50`). The server-side permission bridge requires `is_superuser` for every `BatchOp::is_admin()` op (`server/shamir-server/src/engine/db_handler/handler.rs:35-53,136`). The backup module doc already anticipated this route: an in-process snapshot "requires a wire-protocol change" (`backup.rs:28-30`). No HTTP admin endpoint for maintenance operations was observed.

**Metrics.** `shamir_backup_*` names are reserved in the metrics design (`docs/dev-artifacts/design/engine-metrics.md:160`); conventions: `shamir_` prefix, `_total` counters, `_seconds`, `_bytes` (`engine-metrics.md:29`).

## 2. Requirements (testable)

Each requirement is stated so a test can decide pass/fail. "Copy" = the online backup operation; "restore" = section 5's procedure.

* **R1 Consistency point.** Restoring a completed copy yields the state in which all repositories and the system store were consistent at one instant: for every repo, the restored store state plus replayed WAL equals the live state exactly at that repo's recorded consistency version (manifest field, 4.5); catalogues, repos and the registry agree with each other. Weaker guarantee NOT taken: per-repo independent instants (rejected — cross-repo references via the catalogue would dangle).
* **R2 No lost acknowledged write.** Every write whose acknowledgement reached the client before the capture barrier (step B, 4.2) survives restore. Verified with an acknowledged-writes oracle (6.1): acked writes with timestamp < capture instant ⊆ restored state; acked writes after capture are NOT required (R2b may relax further; here the seal barrier pins the upper edge too).
* **R3 Writer-block bound.** Writers are blocked only inside the two barriers; each barrier lasts at most the per-repo flush time plus a constant, measured and asserted (`shamir_backup_writer_block_seconds`, 4.8). The file copy itself never holds the write barrier. Default bound to assert in tests: p99 barrier < 2 s on CI-sized data (6.4).
* **R4 Failure isolation.** A failed or cancelled copy leaves the source untouched (no mutation, no truncation, retention pin released) and removes its partial output directory; a crash leaving partial output leaves a directory that `verify_manifest` refuses and restore never adopts. Same discipline as restore's staged temp dir (`restore.rs:216-222,422`).
* **R5 Offline verifiability.** `verify` of a copy directory (online or offline kind) runs without a server, re-hashes every manifest file, checks the manifest self-consistency (file set, versions, WAL positions) and reports the stamp observation — today's `verify_manifest` (`backup.rs:411`) extended, not replaced.
* **R6 No source degradation.** During and after a copy, the live server meets its existing durability guarantees (WAL sync on commit, `segment_set.rs:706`; per-commit persist, `repo.rs:114-119`) and WAL retention does not lose recoverable history beyond the pre-copy retention policy.

## 3. Options

Every option is judged against the facts of section 1. "Change surface" names the crates whose internals must change; the engine is being cut into crates, so changes inside `repo`/`table`/`tx` (`shamir-engine`) are weighted heaviest.

### 3.1 Option A — consistent file copy under one short write barrier

Hold a write barrier for the whole copy: quiesce writers, flush all stores, copy every file, release. Simplest correctness argument (exactly the one `backup.rs:15-24` makes for the stopped server).

* Needs: an engine-wide barrier composed from the per-table `WriterDrainBarrier` (`writer_drain_barrier.rs:309-355`) plus `flush_all_history` (`durability_access.rs:263`). No WAL, no replay, no manifest positions.
* Cost: writers blocked for the entire copy — minutes on a real directory. Violates R3 as stated. Fjall compaction running inside the barrier is unknown territory (fjall internals are not ours to pause).
* Failure modes: a stuck flush blocks writers indefinitely; cancellation mid-copy blocks writers until abort.
* OS: none particular — no concurrent mutation by construction.
* Verdict: **rejected for R2a** (bound). Acceptable only as the degenerate "small directory" case; not chosen even there — one code path.

### 3.2 Option B — storage-level snapshot / hard links

Take an OS-level snapshot or hard-link every file, then copy the links at leisure.

* Needs from `shamir-storage`: a file-listing + "replace files only by rename" contract from the fjall backend. Fjall exposes no such API (`backup.rs:6-10`: public surface is insert/get/range/persist/batch; its `Snapshot` is a seqno read snapshot, not file-level). Journal appends mutate files in place, and compaction rewrites trees inside the vendored/backend code — hard links to in-place-mutated files are torn by construction, not by race.
* OS: hard links are reliable only if files are immutable and replaced by rename; NTFS hard links exist but a `FILE_SHARE_DELETE`-less writer holding a file open (Windows default for fjall's owned handles) makes link-and-replace semantics fragile; APFS/ext4/btrfs differ in snapshot support (none portable without a new dependency — a volume-manager integration is explicitly out of scope and would be an owner decision with a no-dependency alternative: none exists).
* Failure modes: silent torn files; no consistent point provable from the manifest.
* Verdict: **rejected** — requires changing/implementing backend file semantics we do not own (fjall), plus OS-specific volume features. Highest change surface in exactly the crates the release plan says to leave alone.

### 3.3 Option C — copy + WAL replay (RECOMMENDED)

Two short barriers bracket the copy; WAL segments retained across the whole window are copied alongside the store files; restore replays.

* Needs from `shamir-wal`: nothing new — segments, sidecars, `replay`, `truncate_below` and `max_committed` exist (`segment_set.rs:562,590,730`); only a *pin* on truncation is required, and `RetentionGate` already mediates retention (`retention_gate.rs:12,89`; `durability_access.rs:5`).
* Needs from `shamir-engine`: expose a small point-capture/quiesce entry (flush all + hold the retention gate + report per-repo `max_committed`) and the restore-side replay entry, which is the existing drainer (`drain_step.rs:51`) pointed at the restored directory. Both are layer-*above* changes (db_instance / a new facade), not changes inside `repo`/`table`/`tx` internals.
* Needs from `shamir-sync`: the copy engine, manifest v2, verification — the crate that already owns backup/restore.
* Cost: copy is full-size (no dedup); restore adds replay time proportional to retained WAL. Barrier cost = flush time (R3 satisfied).
* Failure modes and their answers: WAL hole → retention pin (4.4); torn journal tail → per-keyspace `TolerateCorruptTail` (`backup.rs:15-24`) + replay convergence (4.2, load-bearing requirement L1); mid-copy file deletion/creation → copy retries the directory pass and the manifest's final hash pass catches inconsistency (4.2 step D).
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
| C copy + WAL replay | 2 facade entries, no internals | retention pin only (exists) | none | 2 short barriers | replay | **recommended** |
| D replication to file | new sink backend | sink store | none | none (async) | sink replay | rejected (scope) |

## 4. Design of the recommended option (PROPOSED)

### 4.1 Components and placement

* `shamir-sync`, new folder `sync/backup_online/` — `sync/` and `sync/durability/` are both at the 7-entry limit (`format-stamp.md` 1.6), so no entry is added to either: `mod.rs`, `protocol.rs` (state machine + cancellation), `copy_engine.rs` (share-tolerant recursive copy + hash pass), `manifest.rs` (v2 encode/decode), `wal_capture.rs` (segment listing/pinning calls), `progress.rs`, `tests/`. The offline `backup.rs`/`restore.rs` gain only call sites, exactly like the stamp wiring (`format-stamp.md` 7 step 2).
* `shamir-db` (the `db_instance` facade layer, above `repo`/`table`/`tx`): `capture_point()` and `seal_point()` — flush + gate + position report; `replay_to(repo_path, wal_dir, version)` — drainer entry against an offline directory. No change inside `repo`, `table`, `tx` modules.
* `shamir-server`: admin-op glue (4.7), progress reporting, metrics emission.

### 4.2 Protocol

State machine: `Preparing → Captured → Copying → Sealing → Verifying → Done | Failed(stage)`.

* **A. Prepare.** Acquire the single in-process backup permit (D9). Pin WAL retention on every catalogue repo (gate held from now; `RetentionGate`, `retention_gate.rs:12`). Create the destination `<to>/<timestamp>/` with atomic create semantics (same TOCTOU discipline as `restore.rs:186-216`).
* **B. Capture barrier (short).** Per repo: hold the writer-drain barrier, run `flush_all_history` (`durability_access.rs:263`), read `max_committed` (`segment_set.rs:730`) → `wal_position[repo] = V_r`; flush the system store and record its version. Release the write barrier per repo as soon as that repo is flushed. Barrier cost = flush time only (R3). Record the stamp observation and binary/schema versions here.
* **C. Copy WAL first.** Copy every WAL segment file + sidecar (sealed and active) per repo, plus a small `wal_manifest.json` listing segment seqs and hashes. Copying WAL before stores means the history pinned at B is on disk before the store state that may depend on it. The active segment is copied as-is; its torn tail is recoverable per `backup.rs:15-24`.
* **D. Copy stores (writers running).** Copy each store directory tree with the share-tolerant copy engine: per-directory `read_dir` pass, per-file copy with bounded retry on sharing violations (Windows), and a **second hash pass at the end** (`hash_file_streaming` semantics, `backup.rs:58`). A file that changed between copy and hash is re-copied once; still-changing files are re-copied in step E. A file deleted mid-copy is re-listed; the final manifest is built from what the last pass actually hashed, so the manifest never describes a file set that no longer exists.
* **E. Seal barrier (short).** Per repo again: hold barrier, flush, read `V_r'` ≥ any version whose effects may already be inside the copied store files; copy any WAL segments/sidecars created or rotated since step C (they are sealed by now or copied as active tails); record the consistency point `V* = V_r'`; release the pin only after the WAL of that repo is confirmed on disk. **Load-bearing requirement L1:** every store file copied in D reflects only effects of versions ≤ V*, and replay of WAL versions beyond the restored store's own journal-tail boundary converges — the drainer's existing contract (per-version markers + orphan repair, `drain_step.rs:880`, `drainer/mod.rs:17-39`) is what makes replay idempotent per version. The acceptance test of 6.1 exercises exactly this.
* **F. Verify + manifest.** Build `manifest.json` (4.5) from the last hash pass; write it last with fsync (`backup.rs:330-357` precedent); fsync destination directories (Unix; log-only, `restore.rs:475` contract). `verify_manifest`-style self-check of the finished snapshot before reporting success (R5).
* **G. Release.** Drop retention pins, drop the permit, emit metrics.

Cancellation: checked at every stage boundary and between files; cleanup = remove the destination directory (it was created atomically and never swapped into anything), release pins, mark `Failed(stage)`. A crash mid-copy leaves a partial directory with no manifest; restore refuses it exactly as today refuses an unmanifested snapshot (`restore.rs:169`).

### 4.3 Interaction with checkpoints, retention, compaction

* **WAL retention/truncation:** while the pin is held (A→G), `truncate_below` must not delete segments at or above the pinned floor. The gate is held through the engine's own retention path (`durability_access.rs:5`), so the drainer's normal truncation respects it; the copy additionally re-checks segment presence before hashing (a missing segment is a hard `Failed(Copying)`).
* **Compaction:** fjall compacts concurrently with D; the design never assumes file stability — the final hash pass (step D/F) defines the snapshot. Extra I/O load is bounded by the copy engine's sequential per-directory order and is a metric (`shamir_backup_bytes_copied`), not a correctness concern.
* **Drainer:** runs normally throughout. Its progress moves the live store ahead of V*; irrelevant to the copy, which only reads files and re-hashes.
* **Registry/system store:** the registry file's non-fsynced overwrite (`tables_registry.rs:159-170`) means the copied registry may be a torn file. Mitigation: copy the registry **inside the seal barrier** (it is tiny), after a flush; a torn copy is detected by hash and re-copied under the barrier. Marked as a finding on `format-stamp.md`/storage: `write_atomic` without fsync remains a latent torn-write source for any copy scheme (section 8, Q7).

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
    per_repo: { <repo name>: { wal_position: u64, store_dir: String } },
    system_store_version: u64,
  },
  wal: { dirs: [ { repo, dir, segments: [ { seq, file, sidecar, sha256, size } ] } ] },
  files: [ ManifestFileEntry ],   // unchanged shape (backup.rs:156-165)
}
```

Unknown keys are preserved on decode (the stamp codec's rule, `format-stamp.md` D3) so R2b/R2c can add fields without a version bump. `format_version` mismatch refusal semantics are unchanged (`backup.rs:425-430`). The epoch in the manifest fulfils the R2a requirement of `format-stamp.md` 0 ("epoch in the backup manifests" was deferred to R2 — this is that delivery); no stamp decision changes.

### 4.6 On-disk layout and the R2c extension point

```
<to>/<YYYYMMDD_HHMMSS>/          # unchanged outer naming (backup.rs:231-238)
├── manifest.json                # v2, written last
├── format.stamp                 # copied as today
├── <data_dir contents>          # system store, repos, WAL dirs, registry, audit…
└── backup.meta.json             # NEW: { encryption: "none", engine: "copy+wal-replay" }
```

R2c extension point: `manifest.json` gains a preserved-unknown `encryption` object and, when encryption lands, the payload moves under `payload/` with `backup.meta.json` describing it — a *reader* rule, not a format change: v2 readers must treat the directory as opaque payload plus manifest. No encryption is designed here (boundary R2c).

### 4.7 Command and server-side entry point

* Decision D6: the online backup is dispatched through the admin executor — new `BatchOp::StartBackup { to }` / `BatchOp::BackupStatus` / `BatchOp::CancelBackup` variants in `execute_admin`'s dispatch (`admin_dispatch.rs:24,33-50`), which the server already gates with `is_superuser` (`handler.rs:136`). This is the route the module doc anticipated (`backup.rs:28-30`). A wire-protocol (DTO) change is required; the alternative — a server-local control file or separate admin listener — adds an unauthenticated-or-separately-authed surface and is rejected. Flagged to the owner because it touches the client DTO (section 8, Q3).
* The CLI keeps exactly two subcommands (`main.rs:372,383`) in R2a: offline `backup` and offline `restore`. Online backup is requested through a client connected to the running server; `backup status`/`backup cancel` map to the status/cancel ops. Restore remains an offline CLI action against a stopped server (it swaps directories; it cannot run inside the live server — the probe's lock logic, `restore.rs:9-17`, stays authoritative).
* Progress: `BackupStatus` returns stage, per-repo bytes/files hashed-so-far (from `progress.rs`), started-at, and the last error if `Failed`. Cancellation is cooperative (4.2).

### 4.8 Metrics (names only; design in `engine-metrics.md`)

`shamir_backup_started_total`, `shamir_backup_completed_total`, `shamir_backup_failed_total` (labels: `kind=online|offline`, `result`, `stage`), `shamir_backup_active` (gauge), `shamir_backup_bytes_copied` (`_bytes`), `shamir_backup_duration_seconds`, `shamir_backup_writer_block_seconds` (histogram, labels `phase=capture|seal`), `shamir_backup_wal_segments_copied_total`. All within the reserved `shamir_backup_*` space (`engine-metrics.md:160`) and the naming conventions (`engine-metrics.md:29`).

## 5. Restore and verification for online copies

* **Replay or none (D10).** `kind = "offline"`: today's path, unchanged, no replay (`restore.rs:158-306`). `kind = "online"`: after the swap's step-3 copy into the staged dir (`restore.rs:186-222`), run the replay entry of 4.1 against the staged copy: for each repo, open the WAL directory from the snapshot's `wal/` listing and drain entries from the lowest retained segment through the recorded `wal_position` (the drainer's normal ordered application, `drain_step.rs:51`). Replay failure aborts before the swap (`restore.rs:248-267` discipline) with `data_dir` untouched.
* **Consistency check.** After replay, the staged copy is opened once through `ShamirDb::init` (the same init the next boot would do, `launch.rs:389`) with a read-only consistency pass: catalogue rows ↔ repo directories ↔ recorded `per_repo` positions agree; a mismatch refuses the restore. (Same fail-closed bias as the stamp gate, `restore.rs:180`.)
* **Stamp refusal.** Unchanged: `restore_snapshot_gate` (`restore.rs:180`) on the snapshot's copied stamp; newer epoch refused, older allowed for rollback (`format-stamp.md` 5.2). The manifest `format_epoch` is cross-checked against the copied stamp — a mismatched manifest/stamp pair is refused as corrupt.
* **Clean-machine restore.** `data_dir` absent: steps 3-7 of `restore.rs` already handle the fresh target (`restore.rs:303-310`). For an online copy the replay step runs on the staged dir before the single rename; the machine needs no prior state.
* **Offline verify (R5).** `verify` extends `verify_manifest` (`backup.rs:411`) to: parse v2; check `kind`; re-hash every `files` entry and every WAL segment; check WAL seq contiguity per repo against the `wal` listing; report `format_epoch`, `binary_version`, `consistency` positions; refuse unmanifested files as today (`backup.rs:104-108`). No engine open — pure file inspection, runs on any machine with the binary.

## 6. Tests and acceptance (CI only)

All tests run in the PR's CI via `./scripts/test.sh` scopes (`@server`, `@storage`, `@e2e`); nothing runs locally (CLAUDE.md policy).

* **6.1 Property test under continuous write load (the gate test).** Boot a server (e2e harness style), run N writer clients that continuously insert and record each **acknowledged** write in an in-memory oracle, start an online backup mid-stream, keep writing until it completes, stop the server, `restore` the snapshot to a fresh `data_dir`, boot, and read back: (a) every oracle entry acked before the capture barrier is present with its acked value (R2); (b) the restored state equals the live state at the recorded `wal_position` for a probe set of keys (R1); (c) no torn/partial record anywhere. Replay-convergence requirement L1 is exercised here by construction (the copy races the drainer). Crate: `shamir-e2e` (server boots live there per CLAUDE.md) or `shamir-server` integration tests; sizes per 6.4.
* **6.2 Writer-block bound.** Same harness: measure capture/seal barrier durations via the metrics histogram; assert p99 < bound on CI-sized data (R3). Includes a compaction-pressure variant (larger batches forcing fjall flushes during the barrier).
* **6.3 Failure injection per protocol step.** Injected at each stage of 4.2 using the existing seam style (`segment_set.rs:291,296,309` precedent): fail during A (pin held → assert released on drop), B (flush error → no output dir), C/D (copy error mid-file, deleted source file, Windows sharing violation, hash mismatch on re-pass → source untouched, output removed), E (seal error → Failed(Sealing), WAL consistent), F (manifest write failure → no success report), G (cancel at each boundary → cleanup). Each asserts R4 with a before/after directory snapshot.
* **6.4 OS differences and CI feasibility.** Full-tier matrix runs Ubuntu/Windows/macOS (CLAUDE.md CI tiers; weekly schedule already covers `shamir-server` + `shamir-e2e` on all three). Windows-specific: sharing-violation retry path and rename-over-open-file behaviour; Linux: directory fsync paths; macOS: no additional assertion beyond the shared protocol. CI-sized budgets: data directory ≤ 200 MB, WAL ≤ 32 MB retained, whole gate test ≤ 3 minutes per OS (within the 180 s per-test default scaled by the harness splitting boot/backup/restore phases into separate tests sharing a fixture — if a single test exceeds the budget it is split; the nextest budget stays untouched).
* **6.5 Restore/verify acceptance.** v2 snapshot restores on a clean machine (no `data_dir`); `verify` refuses: checksum mismatch, WAL seq gap, manifest/stamp epoch mismatch, unmanifested file; v1 offline snapshot still restores and verifies unchanged (back-compat, D7).

## 7. Implementation plan (≤ 4 PR-sized steps)

1. **Point capture + replay facade in `shamir-db`** (engine facade layer, above `repo/table/tx`): `capture_point`/`seal_point`/`replay_to` entries composing `flush_all_history` (`durability_access.rs:263`), the retention gate (`retention_gate.rs:89`) and `max_committed` (`segment_set.rs:730`); unit tests with the existing drain/retention seams. No `shamir-sync`, no protocol change.
2. **Online copy engine in `shamir-sync`** (`sync/backup_online/` per 4.1): protocol state machine, share-tolerant copy engine, manifest v2 encode/decode with unknown-key preservation, progress/cancellation; extend `verify_manifest` for v2 and keep v1 green. Pure library; offline `backup` untouched.
3. **Server wiring + admin ops + restore replay** (`shamir-server`, `shamir-db` DTO, `shamir-connect` DTO): `BatchOp::StartBackup/BackupStatus/CancelBackup`, dispatcher arms (`admin_dispatch.rs:33-50` area), permission reuse (`handler.rs:136`), in-process permit (D9), `restore` kind-aware replay + consistency check (section 5), metrics emission (4.8). This is the PR with the wire change (Q3).
4. **Tests + guide**: the gate property test and failure-injection suite (6.1-6.3) in `shamir-e2e`/`shamir-server`; `docs/guide-docs/guide/07-operations.md`, new subsection under `## 8. Backup` — when to use online vs offline backup, the admin-operation procedure, what the manifest's consistency point means, `verify` usage, Windows sharing-violation note, "an offline `backup --to` on a running server is not supported" (Q5), and the R2b/R2c boundary notes.

No new dependencies anywhere (D-decision check: `RetentionGate`, `SegmentSet`, drainer, `sha2`, `serde_json`, `fs4` are all already in the dependency graphs of the touched crates — `format-stamp.md` 1.6).

## 8. Open questions (each with a default)

* **Q1. Admin `BatchOp` DTO change (D6).** Adding `StartBackup/BackupStatus/CancelBackup` touches the client DTO and wire protocol. Default: accept the DTO change; alternative without a protocol change — reuse the existing generic admin batch envelope with string op names, deferring typed variants.
* **Q2. Retention pin floor.** Pin at the pre-copy retention floor (copy carries full retained history — simplest, bigger snapshots) vs pin exactly at `V_capture` (smaller, but requires proving no store file copied in D reflects a version below the floor). Default: full retained history (floor = current retention floor); the smaller pin is an optimization after L1 is proven by 6.1.
* **Q3. Replay idempotency proof (L1).** Is per-version replay idempotency guaranteed for *every* WAL entry kind the drainer applies (interner deltas, index batches, markers), or only for the kinds the crash-recovery path exercises today? Default: assume the drainer's orphan-repair contract generalizes, and make 6.1 the proof obligation; if a non-idempotent kind is found, extend the seal barrier to that kind's flush (bounded, still R3).
* **Q4. System-store version source.** The system store's monotonic version for `consistency.system_store_version` — a dedicated counter vs reuse of the catalogue write horizon. Default: reuse the existing horizon the catalogue already exposes; add a counter only if none is readable at the facade layer.
* **Q5. Offline `backup --to` on a live server.** Detect and refuse (attempt `try_lock_exclusive`; success + data present ⇒ a live server exists ⇒ refuse) vs keep silently allowed. Default: refuse with a pointed error in the same PR as step 3; the silent behaviour is today's documented residual risk until then.
* **Q6. Snapshot retention/rotation of the destination.** Keep `--to` append-only (`backup.rs:190-192` refuses existing timestamp dirs) vs add prune flags. Default: append-only in R2a; pruning is R2d (schedule) territory.
* **Q7. Registry `write_atomic` without fsync** (`tables_registry.rs:159-170`) — a torn registry copy is only caught by the hash pass. Default: copy the registry inside the seal barrier (4.3) and list the missing fsync as a storage-side finding for a separate task; no behaviour change in this design.
* **Q8. In-memory and hybrid repos.** `in_memory` repos have no files; `hybrid` repos use paths like `fjall` (`format-stamp.md` 1.2). Default: online backup covers file-backed catalogue repos under `data_dir`; `in_memory` repos are reported in the manifest as `in_memory` (not recoverable from a file copy — same scope line as the stamp's external-path deferral).

## Findings against other designs

1. `format-stamp.md` decisions: none changed. The R2 deferral "epoch in the backup manifests" (`format-stamp.md` 0) is fulfilled by 4.5 exactly as that document anticipated; stamp check/gate semantics (`backup.rs:229`, `restore.rs:180`) are reused unchanged.
2. `backup.rs` module doc claims a copy "during a quiescent window between commits" recovers consistently (`backup.rs:15-24`) — true per-keyspace, but section 1.3's hazards show it does not extend to concurrent multi-store copies; this design supersedes that reasoning for the online case without changing the offline path.
3. `engine-metrics.md:160` reserves `shamir_backup_*` "delivered with R2d" — this design asks for the counters at R2a; a naming-schedule deviation, flagged to the metrics owner (names only, 4.8).
