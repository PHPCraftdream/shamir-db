# RI-11: `shamir-server restore` — manifest checksums + atomic swap + session invalidation

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context — already investigated, do not re-derive

Review 2026-07-20 P0#10: `shamir-server backup --to <dest>` exists
(`crates/shamir-server/src/backup.rs`) but there is NO `restore` path at
all — an operator has a snapshot directory and no supported way to put it
back. Four concrete gaps, all in scope for this task:

1. No "is the server currently running against this data_dir" check
   before restore overwrites it.
2. No manifest (file list + checksums) written at backup time, so restore
   has nothing to verify against before trusting a snapshot.
3. No invalidation of outstanding resumption tickets after a restore — a
   ticket issued against the PRE-restore state (e.g. for a user who no
   longer exists, or whose password predates the restore point) would
   still resume successfully.
4. No restore CLI subcommand, no e2e test.

PITR / WAL-archive / logical dump are explicitly OUT of scope (beta
roadmap) — this task is stop-the-world snapshot + restore only, matching
the existing stop-and-copy backup model.

### Key finding — reuse fjall's OWN exclusive lock, don't invent a PID file

`fjall` (the storage engine, pinned at `3.1.6`) already takes an exclusive
OS-level advisory file lock per keyspace directory on open — verified by
reading fjall's own source
(`registry/src/*/fjall-3.1.6/src/locked_file.rs`): `LockedFileGuard::create_new`/
`try_acquire` call `file.try_lock()` and return `crate::Error::Locked`
(`std::fs::TryLockError::WouldBlock`) if another process already holds it.
**This means opening any of `data_dir`'s existing fjall stores (e.g.
`server_meta`) while the real server is running already fails cleanly with
a lock-contention error, with ZERO new dependencies needed.** Do NOT add a
PID-file / process-liveness-checking mechanism (crash-safe cross-platform
PID liveness is exactly the kind of fragile hand-rolled logic this
codebase avoids) — instead, `restore` probes exactly this existing lock by
attempting to open `ServerMetaStore::open_or_init(data_dir.join("server_meta"))`
(the same store `server_launcher.rs:133` opens at boot) and treats a
lock-contention failure as "server is running, refuse."

`crates/shamir-server/src/server_meta.rs`'s `MetaError` has a
`#[error("fjall: {0}")] Fjall(#[from] fjall::Error)` variant — match on
`MetaError::Fjall(fjall::Error::Locked)` specifically for the "server is
running" case; any OTHER open error (corruption, etc.) should ALSO refuse
by default (fail-closed — a probe failure could equally mean genuine
corruption, not liveness) but is bypassable via a new `--force` CLI flag
that skips the probe entirely (an explicit, rare, operator-typed opt-in —
same pattern this campaign already uses for its own peak-hours bypass
flag). If `data_dir/server_meta` does not exist yet (nothing to restore
over — a fresh target), skip the probe entirely and proceed.

### Key finding — the session-invalidation field is per-user, already has an established bump pattern

`tickets_invalid_before_ns` lives on `PersistedUser`/`UserRecord`
(`crates/shamir-server/src/user_directory.rs`), NOT as a single global
epoch. The established bump pattern (see `set_superuser`, ~line 705-742,
and the sibling `update_roles`) is: read the user's blob, bump
`tickets_invalid_before_ns = max(current, now_ns)` if `now_ns` is newer,
re-encode, `insert`, `db.persist(PersistMode::SyncAll)`, then
`update_cache` for the in-memory mirror. There is no BULK "bump every
user" method yet — add one (see Part 3). `list_all()` (~line 876) already
does the O(N) full-directory scan this needs to iterate over (mirrors how
`access_tree`/`List` already accept this cost model).

## The task — Part 1: write a manifest at backup time

Extend `crates/shamir-server/src/backup.rs`'s `backup()` function: after
`copy_dir_recursive` completes, walk the JUST-WRITTEN `dest_dir` and write
`dest_dir/manifest.json` (JSON, not `ktav` — this is the one place a JSON
dependency is justified: tooling/scripts inspecting a manifest expect
JSON, and it is not a query/config surface the workspace's builder-only
rule applies to) with this exact shape:

```json
{
  "format_version": 1,
  "created_at_unix_ns": 1234567890000000000,
  "files": [
    { "path": "server_meta/...", "sha256": "<64 hex chars>", "size_bytes": 42 }
  ]
}
```

- `path` is relative to `dest_dir` (forward-slash separated, even on
  Windows — normalize so a manifest written on Windows verifies correctly
  on Linux and vice versa).
- `sha256` — reuse `shamir_connect::common::crypto::sha256` (already a
  workspace dependency, already used this campaign in RI-9) over the
  file's full contents. This IS an extra full read of every backed-up
  file; that's an acceptable, honest cost for a correctness-critical
  manifest — do not skip large files or sample-hash.
- `manifest.json` itself is NOT included in its own `files` list
  (obviously — it's written after the list is computed).
- Add `serde_json` as a new dependency of `crates/shamir-server` (check
  the workspace's existing version pin conventions in other crates'
  `Cargo.toml` files, e.g. `serde_json = "1.0"` — match whatever pin style
  the workspace already uses elsewhere, e.g. `shamir-query-types` or
  similar, for consistency).
- Update `BackupReport` to also report `manifest_path: PathBuf`.
- Update `backup.rs`'s module doc + `main.rs`'s `Backup` subcommand doc
  comment to mention the manifest is now written.

## The task — Part 2: verification helper

Add a `verify_manifest(snapshot_dir: &Path) -> Result<ManifestVerifyReport, BackupError>`
function (same file, `backup.rs`, or a new sibling `restore.rs` if that
reads more cleanly — your call) that:

1. Reads `snapshot_dir/manifest.json`, errors clearly if missing
   (`BackupError` gains a new variant, e.g. `ManifestMissing(PathBuf)`) or
   unparseable (`ManifestInvalid(String)`).
2. For every entry, re-hashes the actual file at `snapshot_dir/<path>` and
   compares byte-for-byte against the recorded `sha256` AND `size_bytes`.
   A mismatch is a hard error (`ChecksumMismatch { path, expected, actual }`)
   — do not warn-and-continue, a corrupted snapshot must never be
   restored.
3. Also verify every file PHYSICALLY PRESENT under `snapshot_dir` (except
   `manifest.json` itself) is ACCOUNTED FOR in the manifest — an extra,
   unlisted file is suspicious (could indicate a tampered/incomplete
   snapshot) and should also be a hard error (a new `UnmanifestedFile(PathBuf)`
   variant).
4. Return a small report (files checked, total bytes) on success.

## The task — Part 3: bulk ticket invalidation

Add to `crates/shamir-server/src/user_directory.rs`:

```rust
/// Bump `tickets_invalid_before_ns` to `now_ns` for EVERY user in the
/// directory (if `now_ns` is newer than their current value) — used after
/// a restore so no resumption ticket issued before the restore point can
/// still resume against the restored state. Mirrors `set_superuser`'s
/// single-user bump pattern (read-modify-write + SyncAll + cache update),
/// just looped over every user via the same full-scan `list_all()` uses.
/// Returns the number of users actually bumped (0 = fresh/empty directory
/// or every user was already past `now_ns` — both fine, not an error).
pub fn invalidate_all_tickets(&self, now_ns: u64) -> Result<usize>
```

Implement it directly (read blob per username from `list_all()`'s
results, decode, bump if `now_ns > current`, re-encode, insert, ONE
`db.persist(PersistMode::SyncAll)` after the whole loop rather than per-user
— batch the fsync since this runs offline over potentially many users, not
on a live request path) — update the in-memory `tickets_cache` per user
too, for correctness if this is ever called on a live-open directory
(defensive; the actual restore call site below always calls it on a
freshly-opened, not-yet-serving directory, but the method itself should
be correct regardless of caller).

## The task — Part 4: the `restore` CLI subcommand

Add `Subcmd::Restore` to `crates/shamir-server/src/main.rs`, mirroring the
existing `Backup` variant's doc-comment style:

```rust
Restore {
    #[arg(long, value_name = "DIR")]
    from: PathBuf,
    /// Skip the "is the server currently running" liveness probe. Use
    /// only when you are certain no server process holds `data_dir` (e.g.
    /// recovering from an unclean shutdown where the lock file itself is
    /// stale) — bypassing this check while a real server is running WILL
    /// corrupt the live database.
    #[arg(long)]
    force: bool,
},
```

Implement the restore procedure (new `crates/shamir-server/src/restore.rs`,
mirroring `backup.rs`'s module structure — `RestoreError`, `RestoreReport`,
`restore(from: &Path, data_dir: &Path, force: bool) -> Result<RestoreReport, RestoreError>`):

1. **Liveness probe** (unless `force`): if `data_dir/server_meta` exists,
   attempt `ServerMetaStore::open_or_init(data_dir.join("server_meta"))`.
   `Err(MetaError::Fjall(fjall::Error::Locked))` → refuse with a clear
   "server appears to be running against this data_dir — stop it first,
   or pass --force if you are certain it is not" message. ANY other error
   opening it → also refuse by default (see rationale above), same
   `--force` bypass. Success → immediately drop the store (release the
   lock) and proceed.
2. **Verify the SOURCE snapshot's manifest** (`verify_manifest(from)`,
   Part 2) BEFORE touching `data_dir` at all. A checksum mismatch or
   missing manifest aborts the whole restore with no side effects.
3. **Copy to a TEMPORARY sibling directory**, not directly into
   `data_dir` — e.g. `data_dir.with_extension("restore_tmp")` or a
   `.restore_tmp_<timestamp>` suffix sibling (must be on the SAME
   filesystem as `data_dir`'s parent for the rename in step 4 to be
   atomic — document this assumption in a doc comment). Reuse
   `backup.rs`'s `copy_dir_recursive` (make it `pub(crate)` if it isn't
   already) to copy `from` → the temp dir.
4. **Atomic swap**: rename the CURRENT `data_dir` (if it exists) to a
   `.pre_restore_backup_<timestamp>` sibling (do NOT delete it — leave it
   for the operator to remove manually once they've confirmed the restore
   is good; this is the "explicit rollback path" the review asked for),
   then rename the temp dir to `data_dir`. If the second rename fails
   after the first succeeded, attempt to rename the `.pre_restore_backup_*`
   dir back to `data_dir` (best-effort rollback) and return a clear error
   describing the partial-failure state and both directory names involved
   — do not leave the operator with an ambiguous or missing `data_dir`.
5. **Invalidate sessions**: open
   `FjallUserDirectory::open(data_dir.join("users"))` (now pointing at the
   RESTORED data) and call `invalidate_all_tickets(now_ns)` (Part 3), then
   drop it (release the lock) before returning.
6. Return a `RestoreReport` (files restored, bytes, users invalidated,
   the `.pre_restore_backup_*` path so the CLI can print it).

`main.rs`'s dispatch: `Some(Subcmd::Restore { from, force }) => { let report = restore::restore(&from, &config.data_dir, force)?; ... }` —
mirror the existing `Backup` arm's print-and-exit style.

## Tests (MANDATORY)

1. `backup.rs`'s existing test module (find it — check whether it's
   inline or already migrated to a `tests/` sibling per this repo's
   convention; if inline, migrate it while you're there per the
   `mod.rs`-manifest-only rule — check first whether this is already the
   established pattern for this file before deciding) — extend for: a
   backup writes a `manifest.json` whose entries' hashes match the
   actual copied files; `verify_manifest` accepts a valid snapshot and
   rejects one with a tampered file (mutate one byte post-backup, expect
   `ChecksumMismatch`), a missing manifest, and an extra unmanifested file.
2. `user_directory.rs`'s test module — `invalidate_all_tickets` bumps
   every user whose current value is older than `now_ns`, leaves newer
   ones untouched, returns the correct count, and is a no-op (`Ok(0)`) on
   an empty directory.
3. **Genuine end-to-end regression** in `crates/shamir-server/tests/`
   (mirror `quickstart_e2e.rs`'s `spawn_ephemeral`/`Client::connect`
   pattern, and `backup_restore_e2e.rs` if that file already exists and
   covers backup — check first, extend it rather than creating a
   parallel file if so) proving the FULL brief-described flow:
   - Boot a server, write some data, obtain a resumption ticket (log in,
     capture the ticket bytes from `AuthOk` — check how existing tests
     already capture/reuse a ticket, e.g. `resume_fast_path.rs`, mirror
     that).
   - `handle.shutdown()`, run `backup::backup(...)`.
   - Boot again, write MORE data (so post-backup state differs from the
     snapshot).
   - `handle.shutdown()` again, run `restore::restore(...)` pointing at
     the earlier snapshot.
   - Boot a THIRD time against the now-restored `data_dir`: assert the
     extra post-backup writes are GONE (data rolled back to the snapshot
     point) — query for them and assert absence/absence-of-change.
   - Attempt to resume the OLD ticket (captured before backup) via
     whatever resume API path existing tests use (check
     `resume_fast_path.rs`) — assert it is REJECTED (tickets were
     invalidated by the restore).
   - Assert a `.pre_restore_backup_*` sibling directory exists (the
     rollback path was taken, not a destructive overwrite).

## Out of scope

- PITR, WAL-archive-based restore, logical dump/restore — stays beta
  roadmap.
- Restoring while the server IS running (online restore) — this task is
  strictly offline (server-stopped) restore, matching the existing
  offline backup model.
- A PID-file/process-liveness mechanism — explicitly rejected above in
  favor of reusing fjall's existing lock.
- Automatic deletion of the `.pre_restore_backup_*` directory — the
  operator removes it manually once satisfied.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh -p shamir-server --full` green, including every new/
  extended test above.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets
  -- -D warnings` clean.
- Report literal command output for all of the above.
