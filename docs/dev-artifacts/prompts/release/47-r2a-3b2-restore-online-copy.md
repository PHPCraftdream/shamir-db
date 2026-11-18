# Brief: R2a step 3b-2 — restore of an online copy (replay, consistency check, stamp cross-check), `verify` v2 completion, refusal of an offline `backup --to` on a live server

Design (settled, read first): `docs/dev-artifacts/design/online-backup.md` — §1.2 (current restore), §4.5 (manifest v2), §4.7 (CLI stays two subcommands), §5 (all bullets: replay or none D10, consistency check, stamp refusal and manifest-epoch cross-check, clean-machine restore, offline verify), §7 step 3, questions Q5 and Q9, the "Step 1 findings" → "Fail-closed recovery and point validation" subsection. Landed and available: the online copy engine and manifest v2 in `crates/server/shamir-sync/src/sync/durability/backup/online/` (`manifest.rs` `decode`/`ManifestV2`/`BackupKind`/`ConsistencySection`/`WalSection`), `backup/offline.rs` (`backup`, `verify_manifest`, already dispatching to `verify_manifest_v2` for v2 manifests: WAL seq contiguity from zero, per-segment hash and size, file set), `backup/restore.rs` (`restore_with_probe`, stamp gate, staged copy, ticket invalidation, restore marker, atomic swap), the server admin operations of step 3a, and the engine primitive of step 3b-1: `shamir_engine::db_instance::replay_to` (read `db_instance/replay_to/` and its module docs first: it takes the paths of ONE staged repository copy, the recorded point `V*` and a caller-supplied `ReplayScope`; it validates the staged WAL before opening the store, runs strict cold recovery, retains the WAL and converges on a repeated call; refusals decided before the store open leave the directories byte-identical, errors after the open do not).

## Why

An online copy is a cross-instant file set plus the WAL taken with it. Restore must make it consistent (replay to the sealed point), prove the result, and refuse anything it cannot prove, with the live `data_dir` untouched on every refusal. Today `restore` trusts the copy as is and ignores manifest v2.

## What (this step only)

1. **Kind-aware restore.** `kind = "offline"` (and every v1 manifest): the current path, byte-for-byte unchanged behaviour. `kind = "online"`: after the copy into the staged sibling directory and before the ticket invalidation / marker / swap, replay every repository of `consistency.per_repo` with `replay_to` against the staged store directory and WAL directory named by the manifest, with `V*` = that repository's `wal_position`. The `ReplayScope` (table configurations incl. physical store ids) is derived from the STAGED copy's own system catalogue, opened offline from the staged directory, exactly the way normal boot derives them; if this needs a new public item in `shamir-db`/`shamir-engine`, name it, keep it minimal and list it before adding (no widening of existing items beyond that). A replay refusal or failure aborts the restore, cleans the staged directory with the same best-effort cleanup as the other steps, returns a typed `RestoreError` naming the repository and the reason, and leaves `data_dir` untouched.
2. **Consistency check (design §5, Q9).** After replay, open the staged copy once through the same init the next boot performs and run a read-only pass: catalogue rows ↔ repository directories ↔ the CAPTURED repo set of the manifest (never the live one). A repository created after capture is excluded and reported; one dropped after capture still copied is an inert orphan and tolerated; a missing captured repository directory refuses. The pass must tolerate store contents above `V*`. Close every handle before the swap. A mismatch refuses with `data_dir` untouched.
3. **Stamp cross-check.** The manifest's `format_epoch` must equal the epoch of the copied `format.stamp` (when a stamp exists; absent stamp keeps today's rule); a mismatch is refused as corrupt before the stage is created. The existing `restore_snapshot_gate` is unchanged.
4. **`verify` v2 completion.** Check what `verify_manifest_v2` already proves (read it) and close the gaps of design §5 "Offline verify": the report carries `format_epoch`, `binary_version`, `kind` and the consistency positions; the manifest `format_epoch` is cross-checked against the copied stamp when one exists; every refusal path has a typed `BackupError`. No engine open, pure file inspection.
5. **Offline `backup --to` on a live server (Q5).** The offline `backup` entry detects a running server on `from` (try the exclusive lock of the live store the way the restore liveness probe does; success of the live-server detection means refuse) and refuses with a pointed error that names the online backup operation. A stopped server keeps working exactly as today. Keep the existing `--force`-style escape if the restore side has one for symmetry; otherwise add none.
6. **Tests** (CI-only; builder-only queries; deterministic; see the checklist): restore of an online copy produced by the real engine facade under concurrent writers (capture/seal through the landed port, files copied with the landed copy engine or the ordinary file API as the 3b-1 live-copy test does), booted afterwards: logical content equals the live content as of `V*` for a probe set; replay failure (an entry above `V*`, an unresolved table token) leaves `data_dir` byte-identical and the staged directory removed; consistency refusals (missing captured repository directory) and the created-after-capture exclusion; manifest/stamp epoch mismatch refused (restore and verify); a v1 offline restore is unchanged (existing tests stay green); offline `backup --to` refused while a live engine holds the store and allowed after it is dropped; a clean-machine restore (absent `data_dir`) of an online copy works.

## Do not

Change `replay_to` semantics, the online copy protocol, the admin operations, the wire protocol, the clients or the CLI surface (two subcommands only); implement encryption, pruning or scheduling; add a dependency; widen an existing item's visibility (list every unavoidable widening BEFORE doing it, with the exact item and why the narrower route fails); touch `repo`/`table`/`tx`/`query` internals of `shamir-engine` (if a needed primitive is missing, stop and list it).

## Test fixture checklist (every item failed in CI on earlier steps)

* Persistent stores only (Fjall in a tempdir); keep every `TempDir` alive for the whole test; close and drop EVERY handle on a directory before it is copied, replayed, restored or reopened; background tasks of a dropped engine keep directory locks until the runtime that ran them drives them again — run on the runtime that built the fixture and repeat an open that meets `Locked` for a bounded time (see the `export_waiting_for_locks` helper in `crates/server/shamir-server/tests/admin/logical_dump/fixture.rs`).
* Windows: a directory cannot be renamed or replaced while handles are open; copy with the ordinary file API.
* Open every table of a fixture before it is read; one runtime per test; handshakes not sleeps; every await bounded; nested values via `QueryValue`, never `doc!`; a batch is at most 50 queries; distinct aliases; tests that drive deep engine async frames run on a roomy thread (`on_big_stack` pattern in `shamir-db`'s schema-migration tests and the server lifecycle fixtures).
* The apply/restore chain nests deep async frames: `Box::pin` large awaits and keep each runner function small (a debug test thread has a small stack).

## Rules that bind the change

* Layout: at most 7 entries per folder, at most 1000 lines per file. `python -I scripts/ci/check_layout.py` counts only TRACKED files: count folder entries by hand (`ls`), measure `wc -l` of every file you touch, name the home of every new file. `restore.rs` is 519 lines: new restore logic goes into new files (for example a `restore/` subfolder after a rename-only move); plan it before coding.
* rust-guard: `structure`, `modules`, `layers` bind changed files: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`, no impl outside the type's module. Register no campaign.
* Imports at the top; one primary item per file.
* Machine memory: at most ONE cargo command at a time, always as `RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1`; never pipe cargo output through `tail`/`head` — redirect to a file outside the repository and read it. Leave no scratch files in the tree.
* Wrap-up: by 150 minutes bring the change to a compiling, formatted, clippy-clean state and stop; say in the final report what is missing.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits. (For file moves use a plain filesystem move and say so in the report;
the orchestrator stages them so git detects the renames.)

## Observable acceptance

`cargo fmt`, `cargo clippy --all-targets -- -D warnings` for every touched crate (expected: `shamir-sync`, possibly `shamir-db`/`shamir-engine` for one named item) are clean; layout limits hold (counts reported); an online copy restores to the state as of `V*` with `data_dir` untouched on every refusal; offline and v1 restores behave exactly as before; `verify` of a v2 manifest reports epoch, binary version, kind and positions and refuses a manifest/stamp mismatch; an offline `backup --to` on a live server is refused; the tests above exist and name what they prove. The final report lists touched files with folders, every widening, decisions taken and what is not covered.
