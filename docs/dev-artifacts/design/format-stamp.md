# Format Stamp: Data-Directory Format Epoch and Startup Rules

- Status: design (release-1 direction R1a)
- Brief: `docs/dev-artifacts/prompts/release/07-r1a-format-stamp-design.md`
- Plan: `docs/dev-artifacts/release-1/README.md`, `docs/dev-artifacts/release-1/01-versioning-and-migrations.md` (sections 2, 3.1-3.3, 3.5, 3.6, 7, 8)
- This document changes no code, test, CI or other document. OBSERVED = fact in the current code (path:line); PROPOSED = design decision of this document.
- Terminology: "format migration" is the change of the on-disk format between epochs (R1b). `MigrateRepo` / `StartMigration` name the move of a repository between engines and are unrelated.

## 0. Summary and decisions

Today nothing stamps a data directory: `launch()` creates the directory, takes the lock and opens stores (`launch.rs:109-120`), and the engine runs WAL recovery inside `ShamirDb::init` via repo attachment (`core.rs:461`; `db_instance.rs:62,75`). This design adds a `format.stamp` file to the root of each data directory and a check that runs once, after the data-dir lock and before the first store is opened, with a total decision table (section 3). Sections 1 and 5.2 are mostly OBSERVED; sections 2-4 and 6-8 are PROPOSED.

Decisions (owner-level ones are repeated in section 8 with defaults):

| # | Decision | Where |
|---|---|---|
| D1 | One stamp per data directory; external repo dirs and in-memory repos are not stamped in R1a | 2.1 |
| D2 | Home: new folder `format_stamp/` in `shamir-sync` (path-based API, no new dependency); embedded `ShamirDb::init` users not covered in R1a | 2.2 |
| D3 | `format.stamp`: fixed header (magic, layout version, epoch) + JSON body with named fields and preserved unknown keys + SHA-256 trailer | 2.3 |
| D4 | Atomic write: tmp in same dir, write, `sync_all`, rename-replace, strict dir fsync on Unix, file re-sync on Windows, read-back verification | 2.4 |
| D5 | Torn or corrupt stamp: fail closed when data is present, tmp never trusted | 2.5, 3.4 |
| D6 | `last_opened_by` rewritten only when the binary version changes | 2.6 |
| D7 | `StampError` lives beside the check in `shamir-sync`; server maps it to `BootError::FormatStamp`; exit 1 | 2.7 |
| D8 | "Data present" = any entry outside the ignore set (`.shamir.lock`, stamp, stamp tmp, configured log file) | 3.1 |
| D9 | Writer vs Inspect mode per subcommand; read-only tools never create a stamp | 3.3 |
| D10 | `backup` is always allowed (observation attached); `restore` checks the snapshot stamp before touching `data_dir`; older snapshot allowed | 3.5 |
| D11 | Base epoch assigned in the release commit; adopted stampless data is flagged `adopted` | 3.6 |
| D12 | Change log is a const table in `shamir-sync` for R1a, moved by R1b | 4.3 |

What R1a does versus defers: R1a ships the stamp, the check, the decision table, the wiring in serve/offline writers, the Inspect behaviour of read-only tools, and the guide section. It defers: the format-migration command and framework (R1b), application schema migrations (R1c), epoch in the replication handshake and seed manifest (R1d), epoch in the backup/export manifests (R2/R3, except the observation the stamp module already exposes), per-repo stamps for external paths, and a read-only engine open for older epochs.

## 1. Observed layout and open path

Paths without a `docs/` prefix are relative to `crates/`. Everything in this section is OBSERVED.

### 1.1 What a server data directory contains

`Config.data_dir` is a plain `PathBuf` (`server/shamir-server/src/boot/config.rs:108`). The server creates the following under it:

| Entry | Created by | Evidence |
|---|---|---|
| `.shamir.lock` | `maintenance_lock::acquire_with` (open+create, `fs4` exclusive try-lock, PID written) | `server/shamir-sync/src/sync/durability/maintenance_lock.rs:9-30` |
| `server_meta/` (fjall) | `ServerMetaStore::open_or_init` | `server/shamir-server/src/engine/server/server_launcher/launch.rs:120-123` |
| `users/`, `counters/` (fjall) | `FjallUserDirectory::open`, `FjallConsumedCounters::open` | `launch.rs:125-131` |
| `audit.log`, `audit_checkpoint/` (fjall) | audit appender | `server/shamir-server-security/src/security/observability/logging/audit_appender.rs:39,41` |
| `bootstrap_token.txt` (default) | bootstrap sweep / `ensure_superuser` | `server/shamir-server/src/boot/bootstrap.rs:55` |
| `shamir_db_meta.redb/` (system store, fjall directory despite the `.redb` name) | `ShamirDb::init_with_durability(SystemStoreConfig::Fjall(..))` | `launch.rs:381-390`; `engine/shamir-db/src/shamir_db/system_store/store.rs:62-67,153-214` |
| `shamir_db_default_main.redb/` (repo `default.main`) | `add_repo("default", RepoConfig::new("main", BoxRepoFactory::fjall(..)))` when the catalogue lacks it | `launch.rs:381,427-435` |
| `<repo path>.shamirwal/` WAL directory, segments `NNNNNNNN.wal`, sidecars `NNNNNNNN.meta` (`WMT3`, legacy `WMT2`, temp `.meta.tmp`) | `SegmentSet::open` (`create_dir_all` of the scan dir) | `engine/shamir-engine/src/tx/commit/core/durability_access.rs:56-64`; `storage/shamir-wal/src/segment_set/segment_set.rs:119-120`; `storage/shamir-wal/src/entry/segment_meta.rs:10-39` |
| wire-tables registry file | `TablesRegistry::open` / `write_atomic` (tempfile + `persist`, no `fsync`) | `server/shamir-server/src/engine/tables_registry.rs:115,159-170`; opened `launch.rs:442` |
| `cache/wasm/` (compile cache) | `WasmEngine::with_compile_cache(data_root)` inside `ShamirDb::init_inner` | `engine/shamir-db/src/shamir_db/shamir_db/core.rs:313-324` |
| `.shamir_restore_epoch` (transient marker) | `restore` writes it into the staged copy; `consume_restore_marker` removes it at the next open | `server/shamir-sync/src/sync/durability/restore.rs:322-325`, consumed `launch.rs:398` |
| `<data_dir>.restore_tmp_<stamp>`, `.pre_restore_backup_<ts>` (siblings of `data_dir`, not inside) | `restore` | `restore.rs:184,247-306` |

`ShamirDb::init_inner` derives `data_root` as the parent of the system-store path (`core.rs:297-301`), so the engine itself assumes "system store lives directly under the data directory" and exposes it as `data_root()` (`core.rs:747-748`).

Backup artefacts live outside the data directory (`backup --to <dir>`; `backup.rs:214`); a snapshot dir holds copies of every top-level entry except `cache/` (`backup.rs:581-601`) plus `manifest.json` (`backup.rs:74`; `Manifest { format_version: u32, created_at_unix_ns, files }` at `backup.rs:165-169`, const `1` at `:172`). Seed artefacts are written to `--out` (`replication_seed/export/exporter.rs:24-31`).

### 1.2 What can live outside the data directory

* Repositories in the catalogue: `ShamirDb::init` re-attaches every catalogue repo from the system store; engine names `in_memory`, `fjall`, `fjall_raw`, `hybrid` are re-created by `factory_from_meta` (`core.rs:867-881`), unknown engines are skipped (survey lead `core.rs:~463`, not re-verified). The path is whatever absolute string is stored in the catalogue row. The server itself only creates `default.main` under `data_dir` (`launch.rs:427-435`); the admin executor, when the system store has a data root, creates `<data_root>/<db_name>/` for repos created by an admin request (`engine/shamir-db/src/shamir_db/execute/admin_db/admin_db_repo.rs:346-352`, second site at `:376`; the in-memory fallback applies when `data_root()` is `None`), so those repos and their `.shamirwal` siblings also live under `data_dir`. Repos whose catalogue row carries any other absolute path (hand-written or created by an embedding application) and their `.shamirwal` siblings live wherever that path points.
* TLS cert/key (config), `logging.file` (resolved to absolute in `server/shamir-server/src/main.rs` before `logging::init` at `:325`, so the log file may be created inside `data_dir` before any check), the bootstrap-token path override, `backup --to`, `seed --out/--artifact`.
* In-memory repositories (`in_memory`) have no files; `hybrid` repos use a path like `fjall` ones (`core.rs:878`).

### 1.3 Open path and order

Server: `main()` maps `Err` to `eprintln!` + `ExitCode::FAILURE` (`main.rs:269-277`). `run_async` loads and validates config (`main.rs:303-304`), initialises logging (`:325`), dispatches subcommands (`:329-457`), otherwise `runtime::serve` (`main.rs:509`; `server/shamir-server/src/boot/runtime.rs:31-43`) builds `ServerLauncher` and calls `launch()`.

`launch()` order (`launch.rs`): `config.validate` `:89` → `create_dir_all(data_dir)` `:109` → data-dir lock `acquire_server` `:118` (failure is `BootError::AlreadyRunning`, `boot_error.rs`) → `ServerMetaStore::open_or_init` `:120` → users `:125` → counters `:129` → audit appender → bootstrap token → `ShamirDb::init_with_durability` `:389` (the system store and the re-attached catalogue repos recover WAL inside `add_repo_with_durability`, `core.rs:461`; `engine/shamir-engine/src/db_instance/db_instance.rs:62,75`) → `consume_restore_marker` `:398` → default repo `:427-435` → `TablesRegistry::open` `:442` → listeners.

Consequence: WAL recovery rewrites WAL/fjall state during `ShamirDb::init`. A stamp check placed at or after `:389` is too late; the first byte the server writes beyond the lock file is at `:120`.

### 1.4 Offline subcommands (gap a)

| Subcommand | Opens what | Lock? | Writes? | Evidence |
|---|---|---|---|---|
| `backup --to` | pure file copy of `data_dir` (+ `manifest.json` in the destination) | none (no `acquire` in `backup.rs`) | only to destination | `main.rs:371-372`; `backup.rs:214` |
| `restore --from` | verifies snapshot manifest, stages `<data_dir>.restore_tmp_*`, swaps; probe checks liveness by opening the EXISTING `data_dir/server_meta` via `ServerMetaStore::open_or_init`; a missing `server_meta` is skipped (`restore.rs:16-20`); `--force` skips the probe | no `.shamir.lock`; liveness by fjall's own lock | replaces `data_dir` | `main.rs:381-382`; `server/shamir-server/src/boot/config/diagnostics/restore.rs:11-33` |
| `doctor` (`--apply` repairs) | `ShamirDb::init(SystemStoreConfig::Fjall(<data_dir>/shamir_db_meta.redb))` retrying 20x, then `TablesRegistry::open` | none | yes: engine init runs WAL recovery, may create `cache/`; `--apply` drops/rebuilds indexes | `diagnostics/doctor.rs:212-250` |
| `access-tree` (offline mode) | same `ShamirDb::init` retry loop | none | same init side effects | `diagnostics/access_tree.rs:146-176` |
| `token list/create/revoke…` | `open()`: `create_dir_all`, `maintenance_lock::acquire`, `ServerMetaStore::open_or_init`, audit log verify, `FjallUserDirectory`, `TokenStore` | yes | yes (audit checkpoint, token store) | `server/shamir-server/src/boot/policy/token_cli/commands/create.rs:176-203`, `list.rs:5-9` |
| `replication-seed-export` | `MaintenanceOpen::open`: lock, requires `shamir_db_meta.redb` to be a directory, `ShamirDb::init_seed`, `consume_restore_marker` | yes | engine init recovery side effects | `main.rs:330-334`; `replication_seed/export/maintenance_open.rs:19-31`; `exporter.rs:37` |
| `replication-seed-install` | `install_inner` (offline, writes the target repo) — shares the maintenance open path (OBSERVED: `maintenance_lock::acquire` at `install/installer.rs:115`) | yes (OBSERVED) | yes | `main.rs:336-354` |
| `replication-seed-verify`, `service …` | no data directory | n/a | n/a | `main.rs:355-369,419-432` |

Two findings relevant to the design: (1) `doctor` and `access-tree` do not take `.shamir.lock` and open the engine, so they are de-facto writers even without `--apply`; (2) `restore`'s probe opens `server_meta` through fjall (taking fjall's own lock) inside a directory that is about to be replaced anyway; a missing `server_meta` is skipped.

### 1.5 Empty versus has data (gap b)

* The fjall backend opens by `Database::builder(path).open()` (`storage/shamir-storage/src/stores/storage_fjall/repo.rs:126`). I did not read fjall's source; that fjall creates the directory is INFERRED from `launch.rs:109` creating only `data_dir` and not `shamir_db_meta.redb`, and from `MaintenanceOpen` having to check `meta.is_dir()` to avoid creating one (`maintenance_open.rs:22`).
* At the moment of the check (after the lock) the only things this binary may already have created in `data_dir` are `.shamir.lock` and possibly the configured log file (section 1.2). The stamp file and its temp file are the format layer's own. Everything else appearing there means data from an earlier run or from a restore.
* The system-store directory `shamir_db_meta.redb` is the strongest signal, but it is not sufficient as the sole key: a directory with only `server_meta/`, `users/` or a stray `.shamir_restore_epoch` was written by some binary and is also data. Section 3.1 therefore defines "data present" as "any entry outside an explicit ignore list".

### 1.6 Other facts used later

* `DbError` lives in `storage/shamir-storage/src/core/error.rs:8` (thiserror enum with `Storage`, `Config`, `Io`, ...). A startup-time format error is not an engine error and should not be added there (see 2.7).
* `BootError` (`server/shamir-server/src/engine/server/boot_error.rs`) already has `AlreadyRunning`, `ShamirDbInit(String)`, `Io(#[from])`, `ServerMeta(String)`.
* Binary version: `env!("CARGO_PKG_VERSION")` is available in every crate at compile time (workspace crates use `0.1.0-alpha.1`, e.g. `server/shamir-sync/Cargo.toml`). No runtime lookup is needed (gap g).
* `fsync_dir` exists as `pub fn` in `restore.rs:466` but is log-only on failure and a no-op on non-Unix (`:491`); restore has a private strict variant `sync_restore_directory` (`restore.rs:378-388`), also a warn-only no-op off Unix. `TablesRegistry::write_atomic` does not fsync (`tables_registry.rs:159-170`). No shared strict atomic-write helper exists.
* Dependencies (gap h): `shamir-sync` already depends on `serde`, `serde_json`, `rmp-serde`, `sha2 0.11`, `hex`, `thiserror`, `tempfile` (dev only), `fs4` (`server/shamir-sync/Cargo.toml`). `crc32fast` is only in `shamir-wal` (`storage/shamir-wal/Cargo.toml:14`). `shamir-db` has `rmp-serde` and `serde` but no `serde_json`/`sha2`/`crc32fast`; `shamir-private-dir` has `tempfile` (dev) plus target-specific `rustix`/`windows-sys`.
* Entry counts: `shamir-sync/src` has 6 entries (`config_error.rs`, `lib.rs`, `meta_port/`, `subscriptions/`, `sync/`, `version.rs`); `shamir-sync/src/sync` and `.../sync/durability` are both at 7 and cannot take a new entry.

## 2. The stamp

Everything in this section is PROPOSED.

### 2.1 Scope: one stamp per data directory (decision D1)

* One stamp file in the root of `data_dir`. It covers everything the server creates there (system store, `default.main`, WAL, audit, caches).
* Repositories whose catalogue path lies outside `data_dir` are NOT stamped separately in R1a. Reasons: the paths are free-form rows in the system store, which is itself under `data_dir` and covered by the stamp; the epoch is a property of the engine's on-disk formats, not of a location, and the only thing that decides which binary may open a repo is the system store that references it; a second stamp inside a foreign directory would be written into a place the server does not own and would have to be kept consistent with the first (two-stamp divergence has no safe resolution rule).
* Consequence and limit: an external repo directory moved to another server is not protected by any stamp. R1a defers this to R2/R3 (backup/export record the epoch of the catalogue, and an external repo is only restored together with its catalogue). It is listed as open question Q2 with default "no per-repo stamp".
* In-memory repositories carry no stamp; their epoch is the process epoch (the constant compiled into the binary). They have nothing on disk to protect. `hybrid` repos have a path and are covered like `fjall` ones (by the data-dir stamp, for the path under `data_dir`).
* Embedded users of `ShamirDb::init(SystemStoreConfig::Fjall(p))` without the server: not covered by R1a (D2): the module lives in the server-side crate (2.2), and `shamir-db` must not depend upward. The stamp API takes a plain directory path, so a later change can call it from an embedding entry point once the owner decides where the code lives (Q3).

### 2.2 Home: a module in `shamir-sync` (decision D2)

Candidates, with the dependency direction `shamir-server -> shamir-sync -> shamir-db -> shamir-engine -> shamir-wal/...`:

| Candidate | Verdict |
|---|---|
| New folder `server/shamir-sync/src/format_stamp/` | CHOSEN. `shamir-sync` already owns the data-dir lock (`maintenance_lock`), backup, restore and seed install/export, i.e. all the offline paths of 1.4 except `doctor`/`access-tree`/`token`, which are in `shamir-server`, a direct dependent. `shamir-sync/src` has 6 entries, so one new sub-folder makes 7 (allowed). Existing deps cover the encoding (2.3): no new dependency. |
| Sub-folder of `sync/durability/` | Rejected: that folder holds 7 entries (`backup.rs`, `config.rs`, `maintenance_lock.rs`, `mod.rs`, `restore.rs`, `tests/`, `tx_registry.rs`); adding one breaks the limit. |
| `shamir-db` (`engine/shamir-db/src/shamir_db`) | Rejected for R1a: needs `serde_json` (or another structured codec) and `sha2`, both absent there (`engine/shamir-db/Cargo.toml`), i.e. a new dependency (README principle 4); the folder `shamir_db/` is also full. |
| New crate (e.g. `shamir-format-stamp` in `foundation/`, 6 crates now so room for one) | Rejected as default: a crate is an owner decision (Q3). It is the better long-term home if embedding is wanted, because it could be a leaf with `serde`, `serde_json`, `sha2`, `thiserror`. |
| Anything in `shamir-engine` (`tx`, `repo`, `table`, `query`) | Forbidden by the brief; those layers are being split into crates. |

The module exposes a path-based API (no `ShamirDb`, no `BootError` in its signatures), so moving it to a crate later is a mechanical move.

### 2.3 File name and encoding (decision D3)

File: `<data_dir>/format.stamp`. Temporary: `<data_dir>/format.stamp.tmp`. Chosen so that `ls` shows them together and so that no engine code or backup tool treats them specially: `backup` copies every top-level file except `cache/` (`backup.rs:581-601`), so the stamp is copied by the existing code (see 5.2).

Layout version 1 (all integers little-endian), a fixed binary frame around a JSON body:

```
offset  size  field
0       4     magic            b"SHFS"
4       2     layout_version   u16   (1)
6       8     format_epoch     u64   (the number the startup rules compare)
14      4     body_len         u32   (<= 65536)
18      N     body             UTF-8 JSON object (named fields, below)
18+N    32    checksum         SHA-256 over bytes [0 .. 18+N)
```

Why this shape:
* `magic + layout_version + format_epoch` come first and are fixed-width, so any future binary, whatever the later layout, can read the epoch of a stamp it does not fully understand (needed so a newer-epoch dir is refused with the right message, 3.4). The codec exposes `read_header()` which returns `(layout_version, format_epoch)` after checking only the magic and the length.
* Named fields in a JSON object body give extensibility without an epoch bump: a new optional field is simply a new key. A reader that does not know a key preserves it. Implementation: the body struct has `#[serde(flatten)] extra: serde_json::Map<String, serde_json::Value>` so unknown keys survive a read-modify-write. A future change that needs a different frame (not just new keys) bumps `layout_version`, not the epoch.
* The checksum covers header and body, so a flipped bit in the epoch is detected (layout 1). SHA-256 comes from `sha2` already in `shamir-sync` (`Cargo.toml`); `crc32fast` is not available there and adding it is a dependency decision. A cryptographic hash is not needed for security here, only for convenience of reuse.
* JSON (not MessagePack with an unknown-key map) because `serde_json` is already a direct dependency of `shamir-sync` and `shamir-server`, a JSON object round-trips unknown keys with `serde_json::Map` without another value type, and an operator can read the body with a standard tool (skip 18 bytes, drop the trailing 32). `rmp-serde` is available too but would need an untyped value type (`rmpv`) for lossless unknown keys, which is not a dependency of `shamir-sync`.
* Duplicating `format_epoch` in header and body is deliberate; they must agree, otherwise `StampError::Corrupt("epoch mismatch")`.

Body fields (layout 1):

| Field | Type | Meaning |
|---|---|---|
| `format_epoch` | `u64` | same value as the header |
| `components` | object `string -> u32` | per-component format versions for diagnostics and for R1b step selection; layout 1 writes `wal`, `storage`, `system_catalogue`, `index_meta`, `vector_snapshot`, `interner`, `audit` with the version each component has at the current epoch (initial values 1; the numbers are documented in the change log, section 4.3). Unknown component names are preserved. |
| `created_by` | `{ "version": string, "unix_ms": u64 }` | `CARGO_PKG_VERSION` of the binary that created the dir (or adopted stampless data, with `"adopted": true`) |
| `last_opened_by` | `{ "version": string, "unix_ms": u64 }` | last binary that opened the dir for writing (see 2.6) |
| `last_format_migration` | `null` or `{ "id": string, "from_epoch": u64, "to_epoch": u64, "unix_ms": u64 }` | written only by R1b's command; R1a writes `null` |

The checksum is the frame trailer, not a body field. Types are fixed-width so they are the same on every OS.

### 2.4 Atomic write per OS (decision D4)

Procedure `write_stamp(dir, stamp)` (always under the data-dir lock):
1. Create `format.stamp.tmp` in the same directory with `create_new`-or-truncate semantics (a leftover tmp from a crash is never reused: it is removed first, then created exclusively).
2. Write the complete frame, `sync_all()` the file.
3. `std::fs::rename(tmp, stamp)` (replace existing).
4. Unix: open the directory, `sync_all()` it, and propagate the error (the stamp module uses its own strict function; it does not use the log-only `restore::fsync_dir`, `restore.rs:466-494`).
5. Windows: no directory flush. Step 3 is `MoveFileExW` with replace (the standard library's rename on Windows). After the rename, the module re-opens the final file and calls `sync_all()` on it as a best-effort flush; this is INFERRED to push the metadata journal on NTFS but is not relied upon.
6. Re-read and verify the stamp that was just written (read back, checksum, epoch equal). A failure is a startup error, not a warning.

Sharing violations and read-only volumes: a failed rename-replace (Windows `ACCESS_DENIED`/sharing violation, e.g. antivirus holding `format.stamp`, or a read-only volume) is `StampError::Io` and fails the start; it is NEVER classified `Corrupt` and never deletes the old stamp or the tmp. No retry loop in R1a (fail closed, Q13).

What can be lost, per OS:
* Unix, steps 1-4 completed: nothing.
* Unix, crash before step 4: after power loss the dir entry may show the old stamp or (first write) no stamp; never a torn stamp, because the content was synced before the rename.
* Windows (and any case where step 4 cannot run): after power loss the directory entry for a first-ever stamp can be missing, or a replacement can still point to the previous stamp. A torn file cannot appear under the final name, because the final name only ever receives a fully synced file via rename; a leftover `.tmp` is the only torn artefact.

Why "adopt on absent" stays safe (the property the argument rests on):
* A replacement lost to power loss leaves the previous stamp, whose epoch is older or equal: the startup rule then says "older" (refuse, run the command) or "equal"; an idempotent format-migration step (R1b) can be re-run. Nothing is misread.
* A first-ever stamp lost to power loss leaves "no stamp, data present", which adopts the base epoch (row 5a). This is correct while the dir really is at the base epoch. It is wrong only for a directory created by a binary whose epoch is above base and which lost its very first stamp before any later stamp replaced it. Mitigations: the stamp is the first thing written into an empty dir, before any store file (3.1), so the loss window is the interval before the OS flushes one directory entry; and the residual risk is stated, not hidden. R1b must additionally write its final stamp with read-back verification and must not run a format-migration chain on a dir whose stamp is `adopted` without operator confirmation (R1b boundary note). This residual Windows risk is open question Q6 with default "documented, accepted for the first release".

### 2.5 Torn and corrupt stamps (decision D5)

States of the stamp location, computed by `inspect(dir)`:

| Observation | Classification |
|---|---|
| no `format.stamp`, no `format.stamp.tmp` | `Absent` |
| no `format.stamp`, `format.stamp.tmp` exists | `Absent` plus `leftover_tmp = true`; the tmp is NEVER read or trusted |
| `format.stamp` shorter than 18 bytes, bad magic, `body_len` beyond the file, missing trailer | `Corrupt(ShortFile \| BadMagic \| Truncated)` |
| layout 1, checksum mismatch, body not valid JSON, missing required field, header epoch != body epoch | `Corrupt(BadChecksum \| BadBody \| EpochMismatch)` |
| `layout_version` greater than the binary's | `NewerLayout { layout, epoch }` (epoch read from the header; checksum NOT verified, the frame may differ) |
| valid | `Valid(Stamp)` |

Handling is in the decision table (section 3). In short: `Corrupt` and `NewerLayout` never lead to a write over the stamp while data is present. The operator recovery is documented in the guide (7.3): restore the last backup, or, if the data is known good, remove `format.stamp` and `format.stamp.tmp` by hand to make the directory "stampless" (which then adopts the base epoch, an operator-asserted statement that must be correct, hence the backup-first advice). A leftover `.tmp` next to a valid stamp is deleted at the next writer start (it is a crashed replacement and the old stamp is intact). The stamp is not bound to the directory contents: a stamp file copied from another directory describes the wrong epoch and nothing in the format can detect that. This is accepted (the operator controls the directory); the backup manifest epoch of R2 and the restore check of 3.5 are the cross-checks, and the guide's manual-recovery step (verify against a backup first) is the procedure.

### 2.6 `last_opened_by`: written only when it changes (decision D6)

* Not rewritten on every start. It is rewritten only when `CARGO_PKG_VERSION` differs from the recorded value (a different binary of the same epoch opened the dir) and only by a writer-mode open (3.3).
* Crash-safety: every rewrite is a stamp replacement with the atomic procedure; each replacement carries a small risk (power loss on a platform without directory fsync, bug in a rewrite path). Rewriting at every start multiplies the exposure of the only file the whole safety model depends on by the number of starts for no information gain. Rewriting on version change bounds it by the number of upgrades, which are operator-visible events with a backup.
* The cost of the choice: `last_opened_by` means "last *distinct* binary version that opened it". It does not prove which version crashed the dir last. Acceptable for diagnostics (Q5, default as stated).

### 2.7 API and error type (decision D7)

In `shamir-sync` (`format_stamp/`), path-based, no dependency on `BootError`:

* `FormatStamp` (the decoded stamp, with `pub fn format_epoch(&self) -> u64` getter for R1d/R2/R3, `components()`, `created_by()`, ...).
* `pub const CURRENT_FORMAT_EPOCH: u64` and `pub const BASE_FORMAT_EPOCH: u64` (section 4).
* `check_data_dir(dir: &Path, mode: StampMode, ignore: &IgnoreSet) -> Result<StampOutcome, StampError>` where `StampMode` is `Writer` or `Inspect` (3.3) and the outcome says what was done (`Created`, `Adopted`, `Unchanged`, `VersionUpdated`, `InspectedStampless`, `InspectedOlder`).
* `StampError` (thiserror): `NewerEpoch { dir, found, supported }`, `OlderEpoch { dir, found, current }`, `Corrupt { dir, kind }`, `NewerLayout { dir, layout, epoch }`, `AbsentOlderBase { dir }`, `Io { dir, source }`. Messages are in the decision table.

Where the type lives: not in `DbError` (`storage/shamir-storage/src/core/error.rs:8`), which is the engine's error and sits below `shamir-sync`, and the failure is a property of the directory, detected before any engine exists. `StampError` lives next to the check in `shamir-sync`. `shamir-server` maps it with a new `BootError::FormatStamp(#[from] StampError)` variant (`boot_error.rs`), which gives the existing `eprintln!("{error:#}")` + `ExitCode::FAILURE` behaviour (`main.rs:269-277`) for the server. Offline subcommands return `anyhow::Error` already; they wrap `StampError` with `?`.

## 3. Startup rules (decision table)

Everything in this section is PROPOSED, except where it cites observed code.

### 3.1 Definitions

* **Data present** (decision D8): `data_dir` exists and contains at least one directory entry (file, directory, hidden or not, symlink) whose name is NOT in the ignore set. **Ignore set**: `.shamir.lock` (created by the lock acquisition that must precede the check, `maintenance_lock.rs:15-20`, `launch.rs:118`), `format.stamp`, `format.stamp.tmp`, and the configured `logging.file` when its parent is `data_dir` (the log is opened before any subcommand dispatch, `main.rs:325`; the ignore list is passed in by the caller, so the stamp module knows no config). Log rotation file names were not verified; a rotated log next to a fresh data dir would count as data and block creation with a "data present, no stamp" adoption, which is the safe direction (Q8 asks the implementer to confirm). `data_dir` absent equals empty (`launch.rs:109` creates it first).
* **Empty** = exists-or-not and no non-ignored entry.
* **Epoch relation** compares the stamp's `format_epoch` to `CURRENT_FORMAT_EPOCH` of the binary: equal, newer (stamp > binary), older (stamp < binary). It exists only for `Present` stamps; for `Absent`/`Corrupt` the epoch is unknown and the cells are marked N/A.
* **Writer mode**: the caller will write to the data dir (serve, token, seed install, restore target). **Inspect mode**: the caller must not create a stamp or any file because of the check.

### 3.2 Where the check runs

Once per process, per data dir, after the lock and before any store:

* Server: in `launch()` between `acquire_server` (`launch.rs:118`) and `ServerMetaStore::open_or_init` (`launch.rs:120`). `create_dir_all` (`:109`) and the lock file are the only writes that precede it, and both are in the ignore set or are the directory itself. This is before `ShamirDb::init_with_durability` (`:389`), hence before any WAL recovery (1.3).
* Offline writers: immediately after `maintenance_lock::acquire`: `token_cli::commands::create::open` (`create.rs:177-180`, between acquire and `open_or_init`), `MaintenanceOpen::open` (`maintenance_open.rs:20`, between acquire and the `is_dir` check).
* `doctor` and `access-tree` (no lock today, `doctor.rs:212-250`, `access_tree.rs:146-176`): Inspect-mode check before the first `ShamirDb::init`. (Taking `.shamir.lock` there is a separate defect, not part of R1a; the step-2 plan lists it as a recommendation.)
* `restore`: see 3.5.
* `backup`: see 3.5.

### 3.3 Mode per subcommand (decision D9)

| Caller | Mode | Stamp written? | Reason |
|---|---|---|---|
| `serve` (`launch()`) | Writer | yes (create / adopt / version update) | owns the dir |
| `token *` | Writer | yes, same rules | `open()` already creates `server_meta` in an empty dir (`create.rs:177-180`), so it must stamp first |
| `replication-seed-install` | Writer | yes | writes the target repo |
| `replication-seed-export` | Inspect | never | `MaintenanceOpen` requires an existing system store (`maintenance_open.rs:22`) |
| `doctor` without `--apply`, `access-tree` offline | Inspect | never | they must not leave a stamp in a dir they only inspect; note that the engine init they run still performs WAL recovery (`core.rs:303`), which is outside the stamp's scope but is why newer/older/corrupt are refused for them |
| `doctor --apply` | Writer | yes | modifies data |
| `backup` | file-copy check (3.5) | never | never interprets data |
| `restore` | snapshot check (3.5) | no new stamp; the copy brings its own | see 5.2 |

Answer to "may offline `doctor` stamp": no for the read-only invocation, yes for `--apply` (Q4, default stated there).

### 3.4 The decision table

Notation: S = stamp state, D = directory, E = epoch relation. "Refuse" means: return `StampError` before anything else is opened, write nothing (no stamp, no file, no mtime change, directory listing unchanged except the lock file), the server exits with status 1 (`BootError` -> `eprintln!` + `ExitCode::FAILURE`, `main.rs:269-277`; the lock `File` guard is dropped on process exit). `<dir>`, `<found>`, `<cur>`, `<ver>` are filled in the message. The table gives Writer-mode and, where different, Inspect-mode behaviour.

| # | S | D | E | Writer-mode action | Error / outcome and message | Inspect mode |
|---|---|---|---|---|---|---|
| 1 | Absent | Empty | Equal | IMPOSSIBLE as a cell: with no stamp there is no epoch to compare. The reachable behaviour of (Absent, Empty) is row 3 | - | - |
| 2 | Absent | Empty | Newer | IMPOSSIBLE (as 1) | - | - |
| 3 | Absent | Empty | Older | IMPOSSIBLE (as 1); this row carries the (Absent, Empty) behaviour: write stamp `{epoch: cur, created_by: {ver}}` as the FIRST file written, then continue; remove a leftover `.tmp` first | `Created` (info log: "created format stamp, epoch <cur>") | write nothing; allow (nothing to inspect) |
| 4 | Absent | Data | Equal | IMPOSSIBLE as a comparison; behaviour is split by `BASE` vs `cur` in rows 5a/5b/5c | - | - |
| 5a | Absent | Data | (`BASE == cur`) | Adopt: write `{epoch: BASE, created_by: {ver, adopted: true}}`, continue | `Adopted` (warn log: "data directory had no format stamp; adopted base epoch <BASE>") | write nothing; allow, outcome `InspectedStampless` |
| 5b | Absent | Data | (`BASE < cur`) | Refuse. Do not stamp (adopting and then refusing would still modify the directory) | `StampError::AbsentOlderBase`: "data directory <dir> has no format stamp, so it is treated as epoch <BASE>, but this binary requires epoch <cur>; refusing to start. No data was modified. Take a backup, then run the format-migration command (R1b)." | refuse (engine-opening tools); `backup` allowed |
| 5c | Absent | Data | (`BASE > cur`) | IMPOSSIBLE: `BASE <= CURRENT` is a compile-time-checked invariant of the constants | - | - |
| 6 | Absent + leftover `format.stamp.tmp` | Data | n/a | Refuse (fail closed). The `.tmp` is an interrupted stamp write; it is not trusted and not deleted | `Corrupt(TornTmp)`: "a format stamp write in <dir> was interrupted (found format.stamp.tmp, no format.stamp); refusing to start. No data was modified. Restore a backup, or, if you are sure the data is intact, remove format.stamp.tmp (the directory is then treated as unstamped; see the operations guide)." | refuse |
| 7 | Present | Empty | Equal | continue; rewrite the stamp only if `last_opened_by.version != ver` (D6) | `Unchanged` / `VersionUpdated` | allow, write nothing |
| 8 | Present | Empty | Newer | Refuse | `NewerEpoch`: "data directory <dir> has format epoch <found>, but this binary (shamir-server <ver>) supports epoch <cur>; refusing to start. No data was modified. Use a binary that supports epoch <found>, or restore a backup made by a binary that writes epoch <cur>." | refuse |
| 9 | Present | Empty | Older | Refuse | `OlderEpoch`: "data directory <dir> has format epoch <found>, but this binary requires epoch <cur>; refusing to start. No data was modified. Take a backup, then run the format-migration command (R1b)." | refuse for engine-opening tools; `backup` allowed |
| 10 | Present | Data | Equal | as 7 | as 7 | as 7 |
| 11 | Present | Data | Newer | Refuse | `NewerEpoch` (as 8) | refuse |
| 12 | Present | Data | Older | Refuse; data and stamp untouched until the explicit command | `OlderEpoch` (as 9) | refuse (engine-opening); `backup` allowed |
| 13 | Corrupt | Empty | Equal | IMPOSSIBLE as a comparison (a corrupt stamp has no trusted epoch); behaviour in row 15 | - | - |
| 14 | Corrupt | Empty | Newer | IMPOSSIBLE (as 13) | - | - |
| 15 | Corrupt | Empty | Older | IMPOSSIBLE (as 13); carries the (Corrupt, Empty) behaviour: rename the corrupt file to `format.stamp.corrupt-<unix_ms>` (evidence kept), then act as row 3 | warn log: "corrupt format stamp (<kind>) in a directory with no data; replaced" | refuse with `Corrupt` (a damaged stamp is not an empty-dir case for a tool that inspects) |
| 16 | Corrupt | Data | Equal | IMPOSSIBLE as a comparison; behaviour in row 18 | - | - |
| 17 | Corrupt | Data | Newer | IMPOSSIBLE (as 16) | - | - |
| 18 | Corrupt | Data | Older | IMPOSSIBLE (as 16); carries the (Corrupt, Data) behaviour: Refuse | `Corrupt(kind)`: "format stamp <dir>/format.stamp is corrupt (<ShortFile / BadMagic / Truncated / BadChecksum / BadBody / EpochMismatch>); the format epoch of the data cannot be determined; refusing to start. No data was modified. Restore a backup, or see the operations guide section on the format stamp." | refuse; `backup` allowed |
| 19 | NewerLayout | Empty or Data | epoch newer | Refuse | `NewerEpoch` message built from the header epoch (the reason for the fixed header, 2.3) | refuse |
| 20 | NewerLayout | Empty or Data | epoch equal or older | Refuse (this binary cannot rewrite a stamp whose frame and unknown keys it cannot preserve) | `NewerLayout`: "format stamp <dir>/format.stamp uses layout <layout>, which this binary (<ver>) cannot read completely; refusing to start. No data was modified. Use the binary version that wrote it or a newer one." | refuse |
| 21 | Present + leftover `format.stamp.tmp` | Empty or Data | per the valid stamp (rows 7-12) | Delete the stale `.tmp` first (interrupted replacement; the old stamp is intact and is the one trusted), then apply the epoch rule of rows 7-12 | `Unchanged` / `VersionUpdated` / the refusal of 7-12; a refused run leaves the tmp in place | Inspect: never deletes; applies the epoch rule of 7-12 |

Rows 1-18 are the full 3 x 2 x 3 cross product (stamp absent / present / corrupt x empty / has data x equal / newer / older); the cells where the stamp has no trusted epoch are marked impossible as comparisons and point to the row that carries the real behaviour. Rows 5a-5c, 6, 19, 20 and 21 are the extra dimensions the cross product alone does not contain (base-epoch adoption, tmp-only state, newer stamp layout, tmp next to a valid stamp — classified in 2.5 as `Absent + leftover_tmp` only when the stamp itself is absent).

Further impossible or degenerate combinations, with reasons:
* `Present` stamp + `Empty` dir (rows 7-9): possible, not impossible: first stamp written, crash before the first store. Treated by the same epoch rules as data (uniform and simple); R1b's command makes an empty older dir trivial to move forward.
* A `Valid` stamp that carries `epoch < BASE`: cannot be produced by any shipped binary; classified `Corrupt(BadBody)` ("epoch below base").
* Absent + Data + epoch compare: undefined; rows 4-5 use `BASE` against `cur` instead.

### 3.5 Read-only and file-copy callers (decision D10)

* `backup` copies bytes and never interprets formats (`backup.rs:581-601` copies everything except `cache/`). It runs `inspect()` read-only, refuses nothing, and reports the observed stamp state in its output so the operator can see "epoch <n>, older than this binary" or "no stamp" or "corrupt stamp (copied as is)". The epoch recorded for R2's manifest is the observed `format_epoch` or the explicit values `unstamped` / `unreadable`. This is what makes "back up before upgrading" work for an older-epoch directory: `backup` is allowed in rows 9, 12 and 18 (and 5b, 8, 11, 20) with the observation attached. (Under R2's online backup the same observation is taken before the copy.)
* `restore` verifies the snapshot's stamp BEFORE touching `data_dir` (it already verifies the manifest first, `restore.rs` step order, `main.rs:381`): snapshot stamp newer than the binary -> refuse (`NewerEpoch`); `Corrupt`/`NewerLayout` -> refuse; absent -> allowed (a pre-stamp backup is base-epoch data, adopted at next start by row 5a (or refused by 5b)); older -> allowed, because "rollback = restore the pre-upgrade backup" (`README` principle 6.4) must work with the new binary; the next `serve` then applies row 12 (refuse) until the explicit command, which is the intended result and is stated in the guide. `restore` never writes a stamp: the stamp inside the copy is the source's (5.2).
* Export of an older-epoch directory by a newer binary: R1a offers no read-only engine open, and `MaintenanceOpen` would run WAL recovery (`core.rs:303`, i.e. write). So `replication-seed-export` refuses older epochs (row 12 Inspect). The pre-upgrade path for R3 is: export with the binary that wrote the data, or `backup` then format-migrate. A read-only engine open mode is an R3 prerequisite that R1a only names (Q9, default: backup-only for older epochs).

### 3.6 Pre-release builds and the base epoch (decision D11)

* Only the format at the first release is the base epoch (README 6.1). Data from earlier builds has no stamp; it is indistinguishable from base-epoch data, so row 5a adopts it silently.
* R1a limits the damage in three ways: (1) while `CURRENT_FORMAT_EPOCH == BASE_FORMAT_EPOCH` (every pre-release build), adopting is the only possible outcome, and it records `created_by.adopted = true` plus the binary version, so later diagnostics can tell "adopted from unknown pre-release data" from "created by a stamping binary"; (2) the warn log of row 5a; (3) the operations guide states that pre-release data must be moved by logical export (R3) and not by upgrading in place.
* What the owner must do at release time: choose the numeric base epoch (default `1`) and make that the value of both constants in the release commit; from that commit on, any change that satisfies the rule of 4.2 bumps `CURRENT_FORMAT_EPOCH` and adds a change-log entry. Before that commit the epoch is not bumped for any change (pre-release data is not protected).

## 4. Base epoch and format change log

### 4.1 Base epoch (PROPOSED)

* `pub const BASE_FORMAT_EPOCH: u64` and `pub const CURRENT_FORMAT_EPOCH: u64` in the stamp module. A unit test asserts `BASE <= CURRENT` and `CURRENT == BASE + number of bump entries in the change log` (so a bump without a log entry, or an entry without a bump, fails in CI).
* Proposed value: `1`, assigned in the release commit (3.6). Until then both constants are `1` and no change bumps the epoch.
* Initial `components` versions (all `1`): `wal` (segment frame and `WMT3` sidecar with legacy `WMT2` read, `storage/shamir-wal/src/entry/segment_meta.rs:54-118`), `storage` (fjall layout and table naming), `system_catalogue` (system-store rows, 13 tables, `store.rs:171-184`), `index_meta` (`shamir-index-core/src/model/meta/persistence.rs`), `vector_snapshot` (`shamir-vector/src/snapshot/`), `interner`, `audit` (`audit.log` tab-separated lines without a version header, `audit_appender.rs:39-41`). These are labels for diagnostics; the compatibility decision uses only `format_epoch`.

### 4.2 The rule (README 3.3, restated)

A change raises the epoch iff, after it, the PREVIOUS binary could read the data wrongly or lose or corrupt data when it writes. Changes that do not raise it are logged with the reason the previous binary is safe. The test is about the previous binary's writes, not only about file-format readability.

### 4.3 Where the change log lives (decision D12) and entry fields

* R1a: one file in the stamp module, `server/shamir-sync/src/format_stamp/format_changes.rs`, holding `pub const FORMAT_CHANGES: &[FormatChange]` (ordered; one primary export). R1b moves it, together with the stamp constants if wanted, next to the format-migration modules it creates (a folder of its own, by R1b's design); until R1b exists there is nothing to sit next to, and a second location now would be moved immediately.
* `FormatChange` fields (static strings so the table is a `const`): `id` (`"2026-xx-short-name"`), `bumps_epoch: bool`, `epoch_after: u64`, `components: &[(&str, u32)]` (component versions after the change), `summary`, `previous_binary_effect` (what the previous binary does with the new data: reads / writes / loses), `reason` (for `bumps_epoch = false`: why the previous binary is safe), `commit` (first commit that carries it), `migration_module` (`None` in R1a; R1b fills it).
* A test (6.1) checks ids are unique, epochs are consecutive and every `bumps_epoch = true` entry names the next epoch.

### 4.4 Worked example: `call_signature`

OBSERVED (current code):
* Written: when a function is created or replaced, `management.rs:170-171` obtains `call_signature` from `materialize_function_source`; the fresh record gets it at `management.rs:266-267` (`fields.insert("call_signature", QueryValue::Bin(..))`). On replace of an existing row the new fresh record (with its own signature, or without one for a non-`shamir` source) is saved together with the existing ACL metadata only (`ResourceMeta::from_record(&existing)`, `management.rs:272-300`); no old field is carried over.
* Altered: `facade/ddl/alter.rs` clones the existing row, sets `optimize` (`alter.rs:93-94`), and REMOVES `call_signature` if the row is not `lang = "shamir"` or the value is not a `Bin` (`alter.rs:96-99`); otherwise the field survives.
* Read: `invocation.rs:105` rejects a checked call when the target's `call_signature` is absent or different ("checked target signature is missing or does not match"); `facade/source_gate.rs:83` skips rows with no (or empty) `call_signature` when building the signature catalogue.

INFERRED (the older binary is not in this tree): a binary from before the field existed builds the replacement row from the fields it knows, i.e. the same shape as `management.rs:235-267` minus the signature insert. Therefore:
* `create_function` / replace by the older binary: the stored row has no `call_signature`. For a function that was a `shamir`-language function (language support arrived with the signature) the older binary cannot even compile the source, so a replace would turn it into a binary-WASM function without signature, which is a real loss of the checked-call contract; every later `call_checked` to it fails at `invocation.rs:105` (a refusal at runtime, not silent corruption).
* `alter` (optimize) by the older binary: if its alter also clones the whole row (as today's does, apart from the cleanup), the field survives; if it rebuilds the row, it is lost. NOT VERIFIABLE here.
* Read paths of the older binary: ignore the field (a missing field is "compatible"); the README 3.3 text says exactly this: read is compatible, write loses.

Epoch decision under the rule: by the rule's letter the change needed an epoch bump (previous binary's replace path writes a row without the signature; callers then refuse at runtime). In practice the loss is detected by refusal, not silent, so a defensible alternative is a log-only entry (`bumps_epoch = false`, reason: "previous binary cannot create `shamir` functions; rows it replaces lose the signature and callers fail closed at `invocation.rs:105`"). Because it predates the first release, it is absorbed into the base epoch and needs no bump; the change log gets a first entry `pre-release-absorbed` documenting it, which also serves as the template. Going forward, the conservative reading applies: when in doubt, bump (a bump with an empty format-migration module is cheap; a missed bump is silent data loss). That is the proposal; Q10 asks the owner to confirm the conservative default.

## 5. Replication and seeds boundary

### 5.1 What R1a records for R1d (PROPOSED)

* The stamp handle exposes the epoch through `FormatStamp::format_epoch(&self) -> u64` and the module exposes `CURRENT_FORMAT_EPOCH`. The server keeps the `StampOutcome` (with the epoch) available to its handle so R1d can read it where the replication handshake is built. No handshake, no wire change in R1a.
* Seed export records the source epoch: R1d adds a `format_epoch` field to the seed manifest (`replication_seed/artifact/manifest.rs`: `deny_unknown_fields` at `:9`, `format_version` `:11`, `codec_version` `:12`, MessagePack — OBSERVED). Because the manifest denies unknown fields, a new field is a `format_version` or `codec_version` decision of R1d, not an additive change; R1a only notes it. `MaintenanceOpen::open` is where the exporter would read the stamp (it already holds the lock and runs the Inspect check, 3.3).
* In-memory repos and external repo directories have no stamp (2.1): the epoch of a seed from such a repo is the process epoch / the data-dir stamp of the catalogue that references it.

### 5.2 Backup and restore (gap f) (PROPOSED)

* `backup` copies every top-level file except `cache/` (`backup.rs:581-601`), so `format.stamp` is copied byte for byte and covered by the manifest's per-file SHA-256 (`hash_file_streaming`, `backup.rs:57-60`; the `sha256` field of `ManifestFileEntry`, `:158`). The stamp in the copy is the SOURCE's stamp; nothing re-stamps it.
* R2's manifest records the format epoch (a new `Manifest` field, `Manifest { format_version, created_at_unix_ns, files }` at `backup.rs:165-169`, is not `deny_unknown_fields`): the value is the observation from 3.5 taken at backup time, equal to the copied stamp's epoch unless the stamp changed between observation and copy (offline backup: cannot). `verify_manifest` (`backup.rs:402-424`) can later compare the manifest epoch with the copied stamp's epoch.
* `restore` swaps a staged directory (`restore.rs:184,247-306`); the staged copy carries its stamp into `data_dir`. It writes no stamp of its own (3.5). A pre-stamp backup restores to an unstamped dir, adopted on the next start (rows 5a/5b).
* The compile cache (`cache/`) is not copied; it is derived data with its own namespace (`wasm/compile_cache/namespace.rs`) and not part of the epoch.

## 6. Tests and acceptance

PROPOSED. CI only (no local test runs, per the repository policy). Test code follows the existing convention: `#[cfg(test)] mod tests;` declared in the owning `mod.rs` with the files under a `tests/` folder per the repository test-organisation rule (e.g. `shamir-server/src/tests/boot/`; note that `shamir-sync/src/sync/durability/tests/mod.rs` exists today only as an empty placeholder).

### 6.1 How to fake an older epoch (no new `#[allow]`, no visibility widening)

`check_data_dir` takes the epoch constants as an ordinary parameter, `EpochPolicy { current: u64, base: u64 }`, a plain public struct in a new file with `EpochPolicy::compiled()` returning the two constants. Production callers pass `EpochPolicy::compiled()`. Tests pass any values: "older" is a stamp with epoch 1 checked with `current: 2`; "newer" is a stamp with epoch 5 checked with `current: 1`. Stamps are created in the fixture through the public encoder (`FormatStamp::new(..)` + `write_stamp`). No `cfg(test)` hook, no existing item becomes more visible. At the real-server level, "newer" needs no faking: a stamp with epoch `u64::MAX` is always newer; "older" cannot be reached through `launch()` while `CURRENT == BASE` (R1a) and is covered at module level. R1b gets the older case through its own migration fixtures.

Shared helper (test side, `format_stamp/tests/support/`): `DirSnapshot::take(dir)` records the recursive listing (relative path, kind, size), a content hash of every file and every modification time, excluding only `.shamir.lock` (created by the lock acquisition that precedes the check in every caller). `assert_unchanged(before, after)` is the "refused before any write" assertion: listing, bytes and mtimes identical.

### 6.2 Tests

Crate `shamir-sync`, module `format_stamp/tests/`. "ALL" = must run on Linux, macOS and Windows (rename, fsync and sharing semantics differ); the rest are pure and run in the same job anyway (cheap).

Codec (fixture: in-memory byte vectors; ALL, pure):
* `codec_round_trip_preserves_all_fields`
* `codec_unknown_body_keys_preserved_on_rewrite`: decode a body with an extra key, re-encode, extra key still present and checksum valid
* `codec_bad_checksum_is_corrupt` (flip one bit in header, in body, in trailer: three cases)
* `codec_short_file_is_corrupt`: lengths 0, 3, 17, 18, `18+N-1`, `18+N+31`
* `codec_bad_magic_is_corrupt`, `codec_body_len_beyond_file_is_corrupt`, `codec_header_body_epoch_mismatch_is_corrupt`
* `codec_newer_layout_still_yields_epoch`: layout 2 file with unknown frame after the header; `read_header` returns `(2, epoch)`; full decode returns `NewerLayout { epoch }`
* `codec_epoch_below_base_is_corrupt`

Presence and ignore set (fixture: `tempfile` dir; ALL):
* `data_presence_ignores_lock_stamp_tmp_and_configured_log`
* `data_presence_counts_any_other_entry` (a lone `server_meta/`, a lone `.shamir_restore_epoch`, a lone hidden file)

Decision table (fixture: `tempfile` dir + `DirSnapshot`; ALL). One test per reachable row, plus a totality test:
* `table_row3_absent_empty_creates_stamp`, `table_row5a_absent_data_adopts_base`, `table_row5b_absent_data_older_base_refuses_unchanged`, `table_row6_leftover_tmp_data_refuses_unchanged`, `table_row7_present_empty_equal_ok`, `table_row8_present_empty_newer_refuses_unchanged`, `table_row9_present_empty_older_refuses_unchanged`, `table_row10_present_data_equal_ok`, `table_row11_present_data_newer_refuses_unchanged`, `table_row12_present_data_older_refuses_unchanged`, `table_row15_corrupt_empty_replaced_and_evidence_kept`, `table_row18_corrupt_data_refuses_unchanged` (each corruption kind), `table_row19_newer_layout_newer_epoch_refuses`, `table_row20_newer_layout_equal_epoch_refuses`.
* Every `refuses_unchanged` test asserts the exact error variant and the message text of 3.4, and `DirSnapshot` equality.
* `table_cross_product_is_total`: enumerates (stamp state x directory state x epoch relation) = 18 combinations plus the extra states of rows 5a-5c, 6, 19, 20 and 21 through the classifier and asserts each maps to either a documented row or an explicit `Impossible` marker; a new state added to the enums without a table decision fails this test.
* `older_epoch_not_modified_without_a_command`: stamp epoch 1, policy current 2, directory with data; run `check_data_dir` in Writer mode three times; refused each time; `DirSnapshot` equal; no function in the stamp module can rewrite an older stamp except the explicit write API R1b will call (asserted by the public API surface: the only writers are `check_data_dir` paths 3/5a/7/10/15 and `write_stamp`).
* `inspect_mode_never_writes` (stampless data dir, stampless empty dir, older, newer): listing and bytes unchanged.
* `last_opened_by_not_rewritten_when_version_unchanged` (stamp file mtime and bytes unchanged after a second check), `last_opened_by_rewritten_when_version_differs`.

Atomic write and torn stamps (fixture: `tempfile` dir; ALL, these are the ones whose behaviour differs per OS):
* `atomic_write_creates_and_read_back_verifies`
* `atomic_write_replaces_existing_stamp` (rename-replace over an existing file; on Windows this is the sharing-sensitive operation)
* `atomic_write_replace_after_reader_closed_handle`: a reader that opened and closed the stamp does not block the replacement (guards the Windows sharing-violation case; the stamp reader must not keep handles)
* `interrupted_replacement_keeps_old_stamp`: write a complete `.tmp` and stop before the rename; `inspect` returns the OLD stamp, ignores the tmp; the next Writer check deletes the stale tmp
* `torn_first_stamp_tmp_only_with_data_refuses` (row 6) and `tmp_only_empty_dir_creates_stamp` (row 3 with leftover tmp)
* `dir_fsync_error_propagates` (Unix only: `#[cfg(unix)]` on the fn; directory removed under the writer) and `dir_fsync_noop_on_windows_does_not_fail_write` (Windows only)
* `stamp_is_first_file_written_in_empty_dir`: a hook-free check: run `check_data_dir` on an empty dir and assert only `format.stamp` (and the pre-existing lock file) exists afterwards

Backup and restore (fixture: the existing `shamir-server/src/tests/durability/{backup_tests,restore_tests}.rs` helpers; ALL):
* `backup_copies_stamp_byte_for_byte_and_reports_observation`
* `backup_older_epoch_dir_is_allowed` (policy older; backup succeeds and reports "older")
* `backup_corrupt_stamp_is_copied_as_is_and_reported`
* `restore_newer_snapshot_refused_before_touching_data_dir` (`DirSnapshot` of `data_dir` unchanged, no `restore_tmp_*` sibling)
* `restore_older_snapshot_allowed_stamp_preserved`, `restore_pre_stamp_snapshot_leaves_dir_stampless`

Server wiring, crate `shamir-server`, folder `src/tests/boot/` (fixture: the launcher tests in `server_tests.rs`, `launcher_starts_http_loopback_ephemeral_and_shuts_down_cleanly` at `:13`, and the lock test `data_dir_lock_blocks_second_instance` at `:191`; ALL):
* `launch_creates_stamp_in_empty_dir`
* `launch_second_start_leaves_stamp_bytes_and_mtime_unchanged`
* `launch_refuses_newer_epoch_and_writes_nothing` (pre-seed `format.stamp` with epoch `u64::MAX` plus a dummy `server_meta/`; `launch()` returns `BootError::FormatStamp(NewerEpoch)`; `DirSnapshot` equal except `.shamir.lock`; no `users/`, `counters/`, `audit.log`, `shamir_db_meta.redb/` created, i.e. refused before `open_or_init`)
* `launch_refuses_corrupt_stamp_with_data_unchanged`
* `launch_adopts_stampless_existing_dir_and_marks_adopted`
* `binary_exits_1_on_refusal` (runs the built binary; uses the e2e harness style of the existing boot tests; exit status 1 and the message on stderr; ALL)
* `token_cli_stamps_empty_dir_before_server_meta`
* `doctor_and_access_tree_do_not_create_a_stamp_in_stampless_dir`, `doctor_refuses_newer_epoch_unchanged`
* `seed_export_inspect_does_not_create_stamp` (crate `shamir-sync`, fixture: `replication_seed` export tests)

Crate `shamir-db` (no stamp code; unchanged fixtures that must still pass, proving R1a touched nothing there): `shamir_db/tests/system/boot_oracle_tests.rs:75,147`, `shamir_db/shamir_db/tests/catalogue/rename_restart_tests.rs:39`, `tests/repo/durable_repo_tests.rs:50` (line numbers from the survey, not re-opened).

### 6.3 Acceptance

* Newer epoch refused and data untouched (`launch_refuses_newer_epoch_and_writes_nothing`, `table_row11/8`).
* Older epoch not modified without a command (`older_epoch_not_modified_without_a_command`, rows 9/12).
* Stampless directory stamped (`launch_adopts_stampless_existing_dir_and_marks_adopted`, `launch_creates_stamp_in_empty_dir`).
* Partially written stamp: the codec short/corrupt tests plus `interrupted_replacement_keeps_old_stamp` and row 6/18.
* All three OS green for the tests marked ALL; `python -I scripts/ci/check_layout.py` OK.

## 7. Implementation plan

PROPOSED. Two PR-sized steps. Each file has one primary export, `mod.rs` files only re-export, no inline `#[cfg(test)]` modules, <= 7 entries per folder, < 1000 lines per file (all new files are far below). The stamp code is in the `server` layer group, crate `shamir-sync`; nothing in `shamir-engine`, `shamir-db` or `shamir-wal`. The module layering gate (`.rust-guard/layers.toml`) is configured for `shamir-engine` only (CLAUDE.md, "layers"), so it does not apply; the `structure` gate applies to the new files: no logic in `mod.rs`, no local `use`, no `impl` outside the type's own module (the `impl` blocks live in the file that owns the type, so no `owned_impl.toml` entry is needed), no new `#[allow]`, no visibility widening of existing items. The `modules` gate needs every new `.rs` reachable from `lib.rs`, which the `mod` chain provides.

### Step 1: the stamp module in `shamir-sync` (no behaviour change anywhere)

New folder `crates/server/shamir-sync/src/format_stamp/` (7 entries: `mod.rs`, `model/`, `codec/`, `io/`, `check/`, `change_log/`, `tests/`):
* `model/`: `mod.rs`, `format_stamp.rs` (`FormatStamp`, getter `format_epoch()`), `epoch_policy.rs` (`EpochPolicy`), `epochs.rs` (`BASE_FORMAT_EPOCH`, `CURRENT_FORMAT_EPOCH`), `stamp_error.rs` (`StampError`), `components.rs`
* `codec/`: `mod.rs`, `header.rs` (`read_header`), `encode.rs`, `decode.rs`
* `io/`: `mod.rs`, `atomic_write.rs` (`write_stamp`, strict dir fsync per OS), `inspect.rs` (`inspect`), `data_presence.rs` (`IgnoreSet`, `data_present`)
* `check/`: `mod.rs`, `check_data_dir.rs`, `stamp_mode.rs`, `stamp_outcome.rs`
* `change_log/`: `mod.rs`, `format_change.rs`, `format_changes.rs` (the const table, first entry `pre-release-absorbed`)
* `tests/`: `mod.rs`, `support/` (`DirSnapshot`), `codec/`, `table/`, `atomic/`, `presence/` (each a folder or file; keep <= 7 each)
* Edit `crates/server/shamir-sync/src/lib.rs`: `pub mod format_stamp;` (7 entries in `src/`).
* No `Cargo.toml` change: `serde`, `serde_json`, `sha2`, `thiserror`, `tracing` are present.

### Step 2: wiring and the guide

* `crates/server/shamir-server/src/engine/server/boot_error.rs` (34 lines): add `#[error("format stamp: {0}")] FormatStamp(#[from] shamir_sync::format_stamp::StampError)` (BootError variant).
* `crates/server/shamir-server/src/engine/server/server_launcher/launch.rs` (869 lines, +~8): call `check_data_dir(&config.data_dir, StampMode::Writer, &ignore, &EpochPolicy::compiled())` between `acquire_server` (`:118`) and `ServerMetaStore::open_or_init` (`:120`); keep the outcome in the `ServerHandle`.
* `crates/server/shamir-server/src/boot/policy/token_cli/commands/create.rs` (`open`, `:176-180`): Writer check after `maintenance_lock::acquire`.
* `crates/server/shamir-server/src/boot/config/diagnostics/doctor.rs` and `access_tree.rs`: Inspect check before the first `ShamirDb::init`; `doctor --apply` Writer. Recommendation only (separate task): take `.shamir.lock` in these two.
* `crates/server/shamir-sync/src/sync/replication_seed/export/maintenance_open.rs` (33 lines): Inspect check after `acquire`; the install path Writer check.
* `crates/server/shamir-sync/src/sync/durability/backup.rs` (645 lines) and `restore.rs` (510): `backup()` gets one call that observes the stamp (read-only) and reports it in `BackupReport`; `restore_with_probe` gets the snapshot-stamp check before staging. The `durability` folder is full (7 entries), so all new logic lives in `format_stamp/` and these two files only call it.
* Server-level tests of 6.2: `crates/server/shamir-server/src/tests/boot/` already has 7 entries (`bootstrap/`, `config_tests.rs`, `mod.rs`, `server_tests.rs`, `shutdown_truth_tests.rs`, `tokens_config_tests.rs`, `version_tests.rs`), and `src/tests/` has 7 too. The PR therefore moves `tokens_config_tests.rs` and `version_tests.rs` into a new `boot/config/` folder (7 - 2 + 1 = 6 entries) and adds `boot/format_stamp/` (7 entries). This is the one layout wrinkle of the plan; the author confirms with `check_layout.py`.
* Guide (do not edit in this task): `docs/guide-docs/guide/07-operations.md`, new subsection `### Штамп формата и обновление версии (format epoch)` placed at the end of `## 8. Backup`, right before `## 9. Capacity planning` (`:939`), after the durability-guarantees subsection (`:909`). Bullets:
  * what the stamp is (`format.stamp` in `data_dir`, epoch, who created and last opened it) and that it is created automatically on first start;
  * upgrade procedure: stop the server, `backup --to`, read the `backup` output (stamp observation), install the new binary, start; if the epoch is older the server refuses and names the format-migration command (R1b); run it; start;
  * meaning of each refusal message (newer epoch: use a newer binary or restore a backup; older: run the command after a backup; corrupt or interrupted write: restore a backup or the manual recovery below; newer layout);
  * rollback = restore the backup taken before the upgrade (`restore --from`); in-place format rollback is not supported (README 6.4); restoring an older backup then requires the format-migration command again;
  * manual recovery of a corrupt stamp (remove `format.stamp` and `format.stamp.tmp` only after verifying against a backup; the directory is then treated as the base epoch);
  * pre-release data: only logical export/import (R3), not in-place upgrade;
  * Windows durability note (no directory flush; what can be lost, 2.4);
  * the stamp is copied by `backup` and restored by `restore`, and `cache/` is not part of it.

## 8. Open questions

Each has a default that applies until the owner decides.

* **Q1. Stamp per data dir vs also per external repo dir.** Default: one stamp per data dir only (2.1). External repo directories are unstamped; R2/R3 treat them as part of the catalogue that references them.
* **Q2. External repo paths and portability.** Related to Q1: should the server refuse catalogue repos whose path lies outside `data_dir` at upgrade time? Default: no, warn only (not designed in R1a).
* **Q3. New crate vs module.** Default: module in `shamir-sync` (2.2), no new dependency. A leaf crate in `foundation/` becomes attractive if embedded `ShamirDb::init` users must be protected; that needs a crate decision and the same dependencies (`serde_json`, `sha2`, `thiserror`).
* **Q4. May offline `doctor` stamp?** Default: `doctor` without `--apply` and `access-tree` never stamp; `doctor --apply` does (Writer).
* **Q5. `last_opened_by` rewriting.** Default: only when the binary version changes (D6).
* **Q6. Windows durability.** Default: no directory fsync is possible; the residual risk of 2.4 (first stamp of an above-base directory lost on power loss and then adopted as base) is documented and accepted for the first release; R1b must not run a chain on `adopted` stamps without confirmation.
* **Q7. Base epoch number and when it is assigned.** Default: `1`, set in the release commit; before it no change bumps the epoch (3.6).
* **Q8. Which pre-release builds.** Default: none are covered; stampless data from any earlier build is adopted as base (row 5a, flagged `adopted`) and the guide says to move it by logical export. Also: confirm the log-file rotation naming for the ignore set (3.1).
* **Q9. Read-only open of an older epoch for backup/export.** Default: `backup` (file copy) is always allowed with an observation; export of an older directory needs a read-only engine open that does not exist yet (R3 prerequisite) and is refused in R1a.
* **Q10. Conservative bump rule.** Default: when in doubt, bump the epoch (4.4); the `call_signature` change is recorded as absorbed into the base epoch.
* **Q11. Corrupt stamp in an empty directory.** Default: replace it and keep the evidence file (row 15); an alternative is to refuse always.
* **Q12. Stamp-only older directory.** Default: refuse like any older directory (row 9) rather than silently re-stamp.
* **Q13. Windows sharing violations on the stamp rename.** An antivirus or backup agent holding `format.stamp` open can make the rename-replace fail. Default: no retry — fail closed with `StampError::Io` (2.4); a bounded retry is a later refinement if field reports justify it.

## Review

Independent review, 2026-11-15. Every OBSERVED citation in sections 0, 1, 4.1, 4.4, 5.1, 5.2, 6.2 and 7 was re-checked against the code by two read-only passes. The design is sound: the check point (after the data-dir lock, before `ServerMetaStore::open_or_init`) is correctly re-derived — the only writes before it are `create_dir_all(data_dir)` (`launch.rs:109`) and the lock file, both in the ignore set; the first non-ignored write is `:120`. The fail-closed bias of the decision table never destroys data and never silently accepts newer data. Findings:

1. ERROR (fixed). WAL recovery was cited as `core.rs:303`; that line is `SystemStore::init_with_durability`. Recovery actually runs inside `add_repo_with_durability` (`core.rs:461`; `engine/shamir-engine/src/db_instance/db_instance.rs:62,75`). The conclusion (check before `:389`, first non-ignored write at `:120`) is unchanged. Fixed in 0, 1.3.
2. ERROR (fixed). Section 1.4 claimed the `restore` probe "creates the store if missing"; the probe skips a missing `server_meta` (`boot/config/diagnostics/restore.rs:16-20`). Fixed in 1.4 (row and finding 2).
3. ERROR (fixed). Section 5.2 cited the per-file SHA-256 as `backup.rs:152-161`; the hashing is `hash_file_streaming` (`:57-60`) and the `sha256` field of `ManifestFileEntry` (`:158`). Fixed.
4. GAP (fixed). The state "valid stamp + leftover `format.stamp.tmp`" was handled only in prose (2.5) and not by any decision-table row, although it is reachable (crash between tmp write and rename). Added row 21; the totality test of 6.2 now includes it.
5. GAP (fixed). No policy for Windows sharing violations (antivirus holding the stamp) or read-only volumes: both now fail closed as `StampError::Io`, never `Corrupt`, no retry (2.4, Q13).
6. GAP (fixed). The stamp is not cryptographically bound to the directory contents; a stamp transplanted from another directory misdescribes the epoch and is undetectable. Stated in 2.5 with the accepted mitigations (R2 manifest epoch, restore check, guide procedure).
7. CONFIRMED (citation upgraded). `replication-seed-install` takes `.shamir.lock` — was INFERRED, now OBSERVED at `install/installer.rs:115` (1.4). The seed manifest's `deny_unknown_fields` was a survey lead, now OBSERVED (`manifest.rs:9`) (5.1).
8. MINOR (fixed). Stale line numbers corrected: `durability_access.rs:56-64`, `core.rs:313-324`, `restore.rs:491`/`:378-388`, `segment_meta.rs:54-118`, `management.rs:170-171`/`:235-267`, `alter.rs:93-99`, `sha2 = "0.11"` re-confirmed. `shamir-private-dir` does have target-specific `rustix`/`windows-sys` deps, not "only tempfile" (1.6). The cited tests-convention example `sync/durability/tests/mod.rs` is today an empty placeholder; replaced (6).
9. CHECKED, no defect. Decision-table consistency: rows 1-18 are the full cross product with the no-epoch cells correctly marked impossible; section 6.2 has one test per reachable row and matches section 3 messages. Epoch policy of section 4 covers every format surface of `01-versioning-and-migrations.md` section 2 with an owner or an explicit deferral: WAL `WMT3`/`WMT2` (`segment_meta.rs:56-57`), backup manifest `format_version` (mismatch rejected, `backup.rs:412-414`), seed manifest (`manifest.rs:11-12`, R1d), vector snapshot legacy codec (`snapshot/legacy.rs:17-51`), index meta version (`persistence.rs:59`), function-row `metadata_version`/`version` and table `schema_version` (absorbed into the base epoch, 4.4); protocol version is a separate axis per the plan. Attack cases considered and covered by the table or the prose: torn write (2.4/2.5, fail closed), crash at every tmp/sync/rename step (2.4), rename-over-existing on Windows (2.4, D4), two processes racing on an empty directory (writers serialize on `.shamir.lock`; the unlocked `doctor`/`access-tree` race is already flagged as a separate defect, 1.4 finding 1), restore over a stamped directory (3.5/5.2), read-only volume (5, above), a stamp newer than the binary combined with an interrupted older-format write (older stamp refuses; the half-written migration state is R1b's journal domain, 3.4 rows 9/12). Implementation plan respects the layout limits (the new `format_stamp/` folder has exactly 7 entries; the `tests/boot/` reshuffle arithmetic was re-verified against the actual folder listings), keeps the dependency direction (stamp in `shamir-sync`, `BootError` mapping in `shamir-server`), and adds nothing to the `shamir-engine` layers. Open questions are consistent with `README.md` section 6 defaults; none is unsafe (Q6's accepted Windows residual risk is bounded by 2.4's mitigations and R1b's `adopted`-stamp confirmation).

What remains open for the owner: Q13 (new, sharing-violation retry), and the existing open questions are unchanged.
