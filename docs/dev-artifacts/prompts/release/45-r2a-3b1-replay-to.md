# Brief: R2a step 3b-1 — `replay_to`: strict cold WAL replay over an offline copy, up to the recorded point V*

Design (settled, read first): `docs/dev-artifacts/design/online-backup.md` — §5 first two bullets (replay or none D10, the cold
recovery semantics, the consistency check), §7 step 1 (`replay_to` named in the facade), the subsections "Step 1 findings" →
"3. Facade API mapping" (the `replay_to(repo_path, wal_dir)` row), "4. CI tests" (the `online_backup_seal::replay_to_v_star_double_boot`
specification), "Fail-closed recovery and point validation (kept prerequisites)" and "Step 1d decisions", questions Q3 and Q9.
Landed and available: the engine facade of steps 1 and 1d in `crates/engine/shamir-engine/src/repo/instance/repo_instance/lifecycle/online_backup.rs`
(`capture_point`, `seal_point`, `SealHold`, `acquire_truncation_pin`) and the commit admission gate; the boot recovery path
(`crates/engine/shamir-engine/src/tx/recovery/v2.rs`: `recover_inflight_v2`, `replay_v2_entry_admitted`; the admitted recovery entry
`recover_v2_inflight_admitted` in `repo/instance/repo_instance/lifecycle.rs`; the WAL manager in `shamir-tx`
`repo_wal_manager.rs`); the `db_instance` layer (`crates/engine/shamir-engine/src/db_instance/`, with the `logical_export/` facade as the
shape precedent for a new facade folder). This step does NOT touch the restore command, the manifest, `verify`, the server or the
protocol: it provides the engine primitive that the restore step (3b-2) will call.

## Why

A file copy taken while the server runs contains store files from slightly different instants. Restore makes it consistent by replaying
the WAL that was copied with it. Today the only replay is the boot-recovery path: it is permissive (warns and continues on unresolved
table tokens and index scopes), has no upper bound (it replays every retained entry, including entries above the recorded point V*) and
runs inside the production constructor. Restore must instead replay deterministically to V*, refuse anything it cannot prove, and be
repeatable: opening the replayed copy twice yields the same state.

## What (this step only)

1. **Facade** `replay_to` in a new item of the `db_instance` layer of `shamir-engine` (name the concrete home after counting entries; a new
   folder like `logical_export/` is the precedent): input = the paths of ONE staged repository copy (store directory and WAL directory) and
   the recorded point `V*` (the manifest `wal_position` / `max_committed` of that repository), nothing running on those paths. Output = a
   report (entries replayed, highest version applied, per-kind counters) or a typed error. It owns an exclusive offline open of the copy
   (hold the directory locks for the call; refuse when they cannot be taken).
2. **Bounded staging of the WAL**: before anything is applied, validate the WAL against `V*`: only complete entries are accepted; the
   highest complete entry must be exactly `V*` (the copy was sealed at `V*`); an entry above `V*`, a torn/incomplete tail, a version gap or a
   duplicated version refuses with a typed reason and applies nothing. Decide and document how entries at or below the version already
   reflected in the copied stores behave (the cold path replays unconditionally — keep that, it is the convergence property of Q3).
3. **Strict cold recovery mode**: run the existing recovery replay with a restore-mode switch that turns every permissive branch into a
   refusal: unresolved table tokens (nonbroadcast and broadcast table-open failures), index operations whose scope cannot be resolved,
   interner delta scope that cannot be proven, and an unexpected warm-path counter delta. Cold semantics stay exactly as boot recovery has
   them (`CounterDelta` is NOT applied on the cold path, the version floor is seeded from the watermark). The default (boot) behaviour must
   not change by a single branch: the switch is a parameter whose default is today's behaviour. List every tx-layer item you touch.
4. **Result state**: after a successful call the copy opens through the normal constructor with nothing left to recover (no inflight
   entries) and its committed version is `V*`; a second `replay_to` over the same copy is a no-op that reports zero applied entries.
   Nothing is written outside the two given directories.
5. **Tests** (CI-only; builder-only queries; deterministic): `replay_to_v_star_double_boot` as specified in the design (a bounded staged WAL
   with only complete entries ≤ V*: a cold open twice reproduces exactly the sealed state, no post-V* effect resurrects on either boot);
   refusals with before/after directory snapshots (an entry above V*, a torn tail, a version gap, an unresolved table token, a missing
   index scope) each leaving the directories byte-identical; a copy taken while writers run (a real engine writing through the facade,
   `capture_point` / `seal_point`, files copied with the ordinary file API, then `replay_to`) replays to V* and the logical content equals
   the live content as of V* for a probe set of keys; the default boot recovery is unchanged (the existing recovery tests stay green and
   one new test pins that the switch default is permissive as before).

## Do not

Change restore, the manifest, `verify`, the online protocol, the server or the clients; change what boot recovery does by default;
add a dependency; widen an existing item beyond what the facade needs (list every widening BEFORE doing it, with the exact item and why
the narrower route does not work); touch `repo` / `table` / `query` internals beyond the strict-mode parameter in the tx recovery path (if
a needed primitive is missing, stop and list it with the rationale).

## Test fixture checklist (every one of these failed in CI on earlier steps — check each before you finish)

* Persistent stores only (Fjall in a tempdir); close and drop EVERY handle on a directory before it is copied, replayed or reopened;
  background tasks of a dropped engine keep directory locks until the runtime that ran them drives them again — run the replay on the same
  runtime that built the fixture, or shut the engine down explicitly, and prove the locks are free before the call.
* On Windows a directory cannot be renamed or replaced while handles are open; copy with the ordinary file API and clean up in the test.
* Open every table of a fixture before it is read through a facade; one runtime per test; deterministic handshakes, no sleeps; every await
  in a test is bounded; nested values via `QueryValue`, never `doc!`; a batch is at most 50 queries; distinct aliases in a batch builder.

## Rules that bind the change

* Layout: at most 7 entries per folder, at most 1000 lines per file. `python -I scripts/ci/check_layout.py` counts only TRACKED files:
  count the entries of every folder you add to by hand (`ls`), measure `wc -l` of every file you touch, name the home of every new file.
* rust-guard: `structure` and `conservation` run only for registered campaigns; register NONE unless a widening is unavoidable — then say
  so BEFORE doing it. No new `#[allow]`, no inline `#[cfg(test)]` modules, no local `use`, no logic in `mod.rs`, no impl outside the
  type's module. `modules` and `layers` apply always (engine layer order `repo < table < validator < tx < query < instance < db_instance`).
* Future size: `Box::pin` large awaits at the call site; recovery paths are deep.
* Machine memory: at most ONE cargo command at a time, always as
  `RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1`; do not pipe cargo output through `tail`/`head` — redirect to a file
  outside the repository and read it.
* Leave no scratch files in the tree. Stop every worker agent before your final report.
* Wrap-up: by 150 minutes bring the change to a compiling, formatted, clippy-clean state and stop; say in the final report what is missing.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits.

## Observable acceptance

`cargo fmt`, `cargo clippy --all-targets -- -D warnings` for `shamir-engine` (and any other crate you touched) are clean and layout counts
are within the limits; `replay_to` validates and bounds the WAL against V*, replays with strict cold semantics, is repeatable and leaves
the directories untouched on every refusal; boot recovery behaves exactly as before; the tests above exist and name what they prove. The
final report lists the touched files with their folders, every widening, the decisions taken (how entries below the stores' own version
behave, the strict-mode parameter) and what is not covered.
