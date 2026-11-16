# Brief: wiring the format stamp (release-1 R1a, step 2)

Design: `docs/dev-artifacts/design/format-stamp.md`, sections 1 (observed code), 3.2 (where the check runs), 3.3 (mode per
subcommand), 3.4 (the decision table), 3.5 (backup and restore), 5.2, 6 (tests) and 7 "Step 2". Read them in full. Step 1
landed (the module `crates/server/shamir-sync/src/format_stamp/`, public API `check_data_dir`, `StampMode`, `StampOutcome`,
`StampError`, `EpochPolicy::compiled()`, `IgnoreSet`, `inspect`, `data_present`; read its code, not only the design). The
open questions of section 8 have defaults; the defaults apply.

## Why

The module exists but nothing calls it. This step runs the check at every place that opens a data directory, so that a data
directory with a newer, older (after the base epoch), torn or corrupt stamp is refused before any store is opened, a new or
stampless directory is stamped, and `backup`/`restore` observe and check the stamp as 3.5 says.

## What

1. **`shamir-server` boot error**: `crates/server/shamir-server/src/engine/server/boot_error.rs` gets a `FormatStamp` variant
   (`#[from] shamir_sync::format_stamp::StampError`, message `format stamp: {0}`); the exit status of a refusal is 1 like the
   other boot errors.
2. **`launch()`** (`engine/server/server_launcher/launch.rs`): call `check_data_dir(&config.data_dir, StampMode::Writer,
   &ignore, &EpochPolicy::compiled())` between `acquire_server` and `ServerMetaStore::open_or_init`; the ignore set carries the
   configured log file when its parent is the data directory; keep the `StampOutcome` in the `ServerHandle` (read how the handle
   is built; a small field and getter). The file must stay under 1000 lines (it is at about 870; add the minimum).
3. **Offline writers**: the token CLI `create::open` after `maintenance_lock::acquire` (Writer); `MaintenanceOpen::open` in
   `shamir-sync` `sync/replication_seed/export/maintenance_open.rs` after `acquire` (Inspect for `replication-seed-export`; the
   seed-install path is Writer: find how the install path reaches the data directory and place the Writer check there);
   `doctor` and `access-tree` (Inspect before the first `ShamirDb::init`; `doctor --apply` Writer). Do not add `.shamir.lock`
   to `doctor`/`access-tree` (a separate task; mention it in the report).
4. **`backup`** (`sync/durability/backup.rs`): one call that observes the stamp read-only (`inspect`) and puts the observation
   into `BackupReport` and into the printed output (epoch, `unstamped`, or `unreadable`), never refusing. **`restore`**
   (`restore.rs`, `restore_with_probe`): the snapshot-stamp check before staging and before touching `data_dir`: newer, corrupt or
   newer-layout refuses; absent or older is allowed; it never writes a stamp. The `durability` folder is full (7 entries), so
   all new logic lives in `format_stamp/` (add a small file there for the backup observation and the restore check, with its
   tests) and the two files only call it.
5. **Tests** as design 6.2 lists for the server level: boot refusal and creation, token CLI, doctor/access-tree inspect never
   writes, backup observation, restore of newer/older/absent/corrupt snapshots, an unchanged directory listing on every
   refusal. `crates/server/shamir-server/src/tests/boot/` has 7 entries and `src/tests/` has 7 too: move
   `tokens_config_tests.rs` and `version_tests.rs` into a new `boot/config/` folder and add `boot/format_stamp/` as the design
   says; keep every moved test's name; run `python -I scripts/ci/check_layout.py`. Add log-capture tests for the three log lines
   of the module only if a shared capture helper makes it cheap (a layer like the one in
   `crates/server/shamir-sync/src/sync/replication/identity_fence/tests/warn_tests.rs` may be reused by moving it into the
   test support of the crate; otherwise leave them untested and say so).
6. **Guide**: `docs/guide-docs/guide/07-operations.md`, a new subsection `### Штамп формата и обновление версии (format epoch)`
   at the end of `## 8. Backup`, before `## 9. Capacity planning`, in the language and style of the guide, with the bullets of
   design 7 step 2 (what the stamp is; the upgrade procedure; the meaning of each refusal message; rollback by restore; manual
   recovery of a corrupt stamp; pre-release data moves by logical export only; the Windows durability note; the stamp is copied
   by `backup` and restored by `restore`, `cache/` is not part of it).
7. **No** `.rust-guard` campaign (no `shamir-engine` source change), no `Cargo.toml` or version change, no engine change.

## Constraints that bit the previous step (binding)

* Code under `cfg(unix)` / `cfg(windows)` is compiled only by one OS locally; CI compiles the other: re-read every gated line
  for paths, imports and privacy.
* A fixture stamp written with another binary version is rewritten by a Writer check (rule D6): a test that expects `Unchanged`
  must write the stamp with this crate's own version (`write_same_version_stamp_with_epoch` exists in the test support).
* No function-local `use`, no inline test modules, `mod.rs` re-exports only, at most 7 entries per folder, at most 1000 lines per
  file, no `#[allow]`.

## Acceptance (observed in CI)

fmt, clippy (`-p shamir-sync -p shamir-server --all-targets`) and layout clean locally; the new and moved tests green on Linux,
macOS and Windows in CI; the guide section present. State in the final report what you could not verify locally.
