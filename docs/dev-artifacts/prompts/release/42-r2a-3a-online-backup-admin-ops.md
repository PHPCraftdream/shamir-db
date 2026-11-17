# Brief: R2a step 3a — online backup admin operations end to end (StartBackup / BackupStatus / CancelBackup)

Design (settled, read first): `docs/dev-artifacts/design/online-backup.md` — §4.2 (protocol A–G), §4.4 (the in-process permit),
§4.7 (command and server-side entry point, decision D6), §4.8 (metrics names), §6.3 (failure injection per step), §7 step 3 (this
step is its first half), questions Q1, Q9, and the subsection "Step 1 findings" with the new "Step 1d decisions" (the seal hold
and the observed backpressure while the pause is held). Landed and available: the copy engine, manifest v2, progress and the
protocol in `crates/server/shamir-sync/src/sync/durability/backup/online/` (`run_online_backup`, `BackupPermit`,
`OnlineBackupPort`, `CapturedPoint`, `PortError`, cancellation, progress); the engine facade of steps 1 and 1d in
`crates/engine/shamir-engine/src/repo/instance/repo_instance/lifecycle/online_backup.rs`
(`RepoInstance::capture_point`, `seal_point` returning a `SealHold`, `acquire_truncation_pin`, `OnlineBackupError`) and the
commit admission gate; the typed admin operations pattern of R1c (mirror how `ApplicationSchema` was added end to end: query-types
`BatchOp` variant and `OpKind`, the builder constructor, the HMAC canonical form, `required_access`, the HTTP scope, the server
handler gate, the token-matrix gaps, tests). This step is NOT restore, `replay_to`, offline verify or the offline-backup refusal on
a live server (step 3b).

## Why

The copy engine and the engine barrier exist but nothing connects them: no operation starts an online backup, reports its
progress or cancels it, and the port has no implementation. This step makes the online backup usable against a running server.

## What (this step only)

1. **Wire.** New typed admin `BatchOp`s `StartBackup { to }`, `BackupStatus`, `CancelBackup` (names exactly these) in
   `shamir-query-types` with their `OpKind` discriminators, the `shamir-query-builder` constructors, the HMAC canonical encoding,
   and the stable wire shape (the discriminator key next to the operation's own fields — read how `application_schema` was fixed
   in `batch_op.rs`; do not repeat that mistake). Superuser-only, gated like the other superuser admin operations in the server
   handler; they are node-local, not database-scoped. A follower/read replica refuses `StartBackup` and `CancelBackup`;
   `BackupStatus` is allowed anywhere.
2. **Port extension (shamir-sync).** The seal re-copy must run while the seal hold keeps the stores at `V*` (design "Step 1d
   decisions"). Add the port method that ends the seal for a repo (for example `end_seal`) and call it from the protocol right
   after the seal re-copy of that repo's stores and on every failure/cancellation path; update the protocol tests and test
   doubles. Keep the existing protocol semantics otherwise; the hold's lifetime bound is the per-repo re-copy cap of Q12: add the
   cap as a documented option of the protocol (bytes and wall clock) that fails the backup closed instead of blocking unboundedly.
3. **Adapter (shamir-server).** Implement `OnlineBackupPort` over the engine facade in a new module of `shamir-server`:
   the catalogue repo set fixed at step A (Q9: only that set; repos created later are excluded and reported), pins acquired per
   repo (held A→G, released by drop, idempotent), `capture_point` / `seal_point` per repo through the `RepoInstance` facade, the
   system store flush and version capture, the seal holds kept per repo until `end_seal`. The port is synchronous: run the
   protocol on a blocking thread and bridge to the async facade through the runtime handle; no async work is started from inside
   a lock, and nothing blocks the runtime workers.
4. **Session registry and permit.** One online backup per server (the in-process `BackupPermit`, a distinct typed error for a
   second start), a registry holding the running/last backup's status, progress snapshot and cancel flag for `BackupStatus` /
   `CancelBackup`; status survives until the next start. `StartBackup` returns immediately with the backup id and the destination;
   the work runs in the background. Destination handling follows §4.2 A (atomic create, refuse an existing directory); the
   operator-supplied `to` is the root and is validated like the offline backup does (no traversal outside it).
5. **Metrics.** The `shamir_backup_*` series of §4.8 that this step can measure (started/completed/failed with `kind` and `stage`,
   active gauge, bytes copied, duration, writer-block histogram for capture and seal, WAL segments copied, pin duration), emitted
   through the existing observability render path of `shamir-server-security` (mirror how the query and index metrics are exposed:
   a small snapshot struct plus a renderer; describe each series in the operations guide file that already lists the others).
6. **Tests** (CI-only; homes named per the layout rule; builder-only queries): the full protocol against a live in-process server
   with writer clients running: the backup completes, the manifest is written last, the snapshot verifies with the existing
   verifier; status shows stages and progress; cancel at each stage boundary releases the pins, the gate and the pause and leaves
   no destination directory (R4: before/after directory snapshots); a second start is refused with the typed error; a non-superuser
   is refused; a follower refuses start and cancel; failure injection per protocol step through the adapter (pin failure, capture
   error, copy error, seal error, manifest write error) leaves the engine fully open (gate open, pause released, pins released);
   the re-copy cap fails the backup closed; the wire round trip through the normal batch entry built with the builder;
   the metrics render and their values follow the state.

## Do not

Implement `replay_to`, restore changes, offline `verify` extensions, the offline `backup --to` refusal on a live server, schedules or
retention (R2d), encryption (R2c), clients or the shell; change the copy engine's copy/hash logic or the manifest format; add a
dependency; touch `shamir-engine`'s `repo` / `table` / `tx` / `query` internals beyond what the facade of 1d already exposes (if a
needed primitive is missing, stop and list it with the rationale). Widen no existing item: re-export what the adapter needs
through new items and list every widening you cannot avoid.

## Test fixture checklist (every one of these failed in CI on earlier steps — check each before you finish)

* A persistent system store is required wherever the journal or catalogue is touched: `ShamirDb::init(SystemStoreConfig::Fjall(tempdir/"system"))`.
* A database identity on the wire is a canonical base58 `RecordId`; a UUID-looking literal does not parse. `Manage` is owner/admin-only.
* A batch is capped at 50 queries; never build nested test values with `doc!` — use `QueryValue` directly; the batch builder keeps
  the first op per alias, so give every op a distinct alias.
* The export/engine facades refuse tables that are not opened: open every table of a fixture before it is read through a facade.
* Close and drop every handle on a Fjall directory before the same directory is opened again; keep one runtime alive across a test
  that spawns engine tasks (a temporary runtime that dies kills the changefeed writers).
* Tests must be deterministic: handshakes, not sleeps; every await in a test is bounded.

## Rules that bind the change

* Layout: at most 7 entries per folder, at most 1000 lines per file. `python -I scripts/ci/check_layout.py` counts only TRACKED
  files, so it does not see the files you create or the ones you move: count the entries of every folder you add to by hand
  (`ls`), measure `wc -l` of every file you touch, and name the home of every new file in the report. Many natural homes are full
  (`shamir-sync` `src/`, `sync/`, `sync/durability/`, `shamir-server` `src/`, `boot/`; `shamir-query-types` `ops/batch/ops/`):
  a split or move is its own, separately described change.
* rust-guard: `structure` and `conservation` run only for registered campaigns; register NONE unless a widening of an existing item is
  unavoidable — then say so BEFORE doing it, with the exact items. No new `#[allow]`, no inline `#[cfg(test)]` modules, no local
  `use`, no logic in `mod.rs`, no impl outside the type's module. `modules` and `layers` apply always.
* Future size: `Box::pin` large awaits at the call site; the dispatch path is deep.
* Machine memory: at most ONE cargo command at a time, always as
  `RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1`; do not pipe cargo output through `tail`/`head` (a pipe hides the
  exit code) — redirect to a file outside the repository.
* Leave no scratch files in the tree.
* Wrap-up: by 150 minutes bring the change to a compiling, formatted, clippy-clean state and stop; say in the final report what is missing.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits.

## Observable acceptance

`cargo fmt`, `cargo clippy --all-targets -- -D warnings` for `shamir-query-types`, `shamir-query-builder`, `shamir-sync`,
`shamir-server`, `shamir-server-security` (and any other crate you touched) are clean and the entry counts and line counts of every
touched folder and file are within the limits; an online backup can be started, observed and cancelled through the typed
operations against a live server; the seal hold lives exactly through the seal re-copy; every failure and cancellation path leaves
the engine fully open; the tests above exist and name what they prove. The final report lists the touched files with their folders,
every widening, the decisions taken (cap values, registry shape, destination validation) and what is not covered.
