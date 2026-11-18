# Brief: R3 step 3 — offline logical import (and the stored string/array value kinds in the export)

Design (settled, read first): `docs/dev-artifacts/design/logical-dump.md` — §0 decisions 6–8, §3.1–3.3 (stream framing, value
envelope, portable content and exclusions), §4.2 (import: target, preflight, stage and load, validation and publication,
retry and cancellation), §5 (CI tests), §6 PR 3, §8 questions 4, 5, 6, 11. The landed export side and its scope note:
`docs/dev-artifacts/dev/logical-export-offline-scope.md`, the driver
`crates/server/shamir-server/src/boot/config/diagnostics/logical_dump/`, the engine read facade
`crates/engine/shamir-engine/src/db_instance/logical_export/`, and the codec/verifier in
`crates/server/shamir-sync/src/sync/replication_seed/artifact/logical_dump/` (re-exported from
`shamir_sync::replication_seed`: `DumpReader`, `verify_dump_file`, `PortableItem`, `LogicalDumpHeader`, `SectionKind`,
`encode_value`/`decode_value`, `DumpReport`). Every claim carries a path; the tree moves, re-verify what you touch.

## Why

An export that nothing can import is not a recovery path. Release gate R3 needs a verified round trip: export a database
offline, import it into a fresh data directory, and read back the same logical content. Today the export also refuses every
string and every array value, which excludes almost every real database; that refusal is lifted here for the shapes the
storage keeps (decision below), so the round trip is meaningful.

## Decisions taken by the orchestrator (follow them; report if the code contradicts them)

* **Stored-shape fidelity for strings and arrays.** The storage codec flattens `Dec`/`Big` to strings and `Set` to lists, so
  the exporter cannot tell them apart. The export emits what is stored: a stored string as `str`, a stored list as `list`.
  The artifact therefore round-trips the stored bytes exactly; `Dec`/`Big`/`Set` come back as `str`/`list` (document this in
  the scope note and in the design; do not change the storage codec). The import accepts every tag of the value envelope
  (`null`, `bool`, `int64`, `f64`, `dec`, `big`, `str`, `bin`, `list`, `set`, `map`) and converts through the normal
  logical-value-to-stored-value path, so a future exporter that tags `dec`/`big`/`set` needs no import change.
* **Target is a whole server data directory** that must be absent or empty (design §4.2); nothing is merged or replaced.
* **Offline CLI only**: `shamir-server logical-import` next to `logical-export`; no server-side arbitrary-path import.

## What (this step only)

1. **Export: lift the string and array refusal.** In the record projection (`logical_dump/projection.rs`) accept stored
   `Str` and `List` values (recursively, inside maps and lists) within the existing node/size/depth budgets; update the
   scope note and the tests that asserted the refusal. Keep every other refusal (indexes, schemas, validators, functions,
   groups, non-default buffers, …) exactly as it is.
2. **Preflight** (no mutation): stream-verify the whole input with the landed verifier (magic/version/features, canonical
   encoding, limits, frame and section checksums, counts, trailer prefix digest, EOF); additionally check cross-section
   coherence the verifier does not (records refer to exported tables, tables to exported repositories, ACL owners to exported
   identities, raw-key length is the 16-byte record id, `required_features` empty, `application_schema_version` null,
   `schema_only` consistent). Refuse unsupported content with a typed reason; never skip or ignore.
3. **Target, lock and stage.** Exclusive ownership through a create-new lock file BESIDE the target
   (`<target>.import.lock`), acquired before the target is inspected; refuse a target that exists and is not empty. Create a
   uniquely named sibling stage on the same volume; remove a stale stage left by a crashed attempt only while holding the
   lock. Publish by one same-volume rename of the stage onto the absent (or just-removed empty) target; a platform or
   volume that cannot do it fails closed. Cancellation (a flag checked between frames) discards the stage; after a
   successful publish it reports success.
4. **Load**, definitions before records, through the narrowest path that preserves the 16-byte raw record keys:
   create the database, repositories (engine kind and durability as exported) and tables (`enable_indexes`, buffer TTL), then
   insert records with their exact keys in bounded batches (never a whole table in memory). The query builder cannot carry a
   record key; the researched narrowest engine path is `ShamirDb::create_db` / `add_repo` / `add_table` and a keyed upsert
   on the table manager (`TableManager::set` / `set_tx`); confirm it is reachable from `shamir-server` WITHOUT widening an
   existing item's visibility. If it is not, stop and describe the smallest new item that would make it reachable, where it
   would live and its rust-guard consequence, instead of widening. Convert logical values with the landed envelope
   conversion (`into_query_value` is not re-exported today — a new re-export of an existing item is a widening: list it).
5. **Identities and ACLs.** Imported users are created disabled and passwordless (no credential material, no
   superuser/replicator state); ACL and ownership references resolve only through the imported identity names
   (`--as-db` renames the database consistently); an unresolved owner fails the import; nothing grants a privilege or
   administrator status implicitly. Find how the server's user directory and resource ownership are persisted and create
   exactly those records. If a record cannot be created without credential material, stop and report.
6. **Validate and publish.** After load, re-read the staged content through the engine read facade and compare it with the
   artifact (counts per table, key set, value equality, settings, ownership); re-read and re-hash the input while loading and
   abort if the digest differs from the preflight one. Flush and close the staged database, write the format stamp the way a
   fresh server boot does, and write a receipt (exact artifact digest, effective options and name mapping) into the staged
   directory; then publish. A complete existing target is accepted as an idempotent retry only when its receipt matches the
   digest and options and an integrity check passes; otherwise refuse.
7. **CLI wiring** in `shamir-server` (`main.rs` subcommand, `lib.rs`, `boot/mod.rs`, `diagnostics/mod.rs`) with progress on
   stderr, in a new sibling folder `diagnostics/logical_import/` (the researched count of `diagnostics/` is 6, so one new
   folder fits; every other folder on the path is full — re-count before you add anything).
8. **Tests** (CI-only; homes named per the layout rule; builder-only queries; multi-line JSON where unavoidable):
   export then import round trip over a database with strings, lists, nested maps, binary, ints, floats, null, several
   repositories and tables, `enable_indexes` and TTL — read back through the engine and compare keys and values; schema-only
   scope; `--as-db`; identities disabled and passwordless, ACL owner mapping, unresolved owner refusal; negative cases —
   corrupt frame, truncated, trailing bytes, over-limit, unsupported feature/version/flags, non-canonical value, key not 16
   bytes, non-empty target, existing lock, input changed between preflight and load, cancellation, a failed import
   publishes nothing and leaves no stage; idempotent retry with a matching receipt and refusal with a different one; the
   export's string/array shapes now round-trip.

## Do not

Implement the online export job or admin operations, lift any other export refusal (schemas, validators, functions, groups,
indexes, guest modules), add import-into-existing, merge or overwrite modes, add encryption, change the storage codec or the
artifact format, add a dependency, or touch `shamir-engine`'s `repo` / `table` / `tx` / `query` layers (if a needed engine
primitive is missing, stop and list it with the rationale).

## Rules that bind the change

* Layout: at most 7 entries per folder, at most 1000 lines per file. `python -I scripts/ci/check_layout.py` counts only
  TRACKED files and does not see the files you create: count the entries of every folder you add to by hand (`ls`) and
  name the home of every new file in the report.
* rust-guard `structure`: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`, no impl
  outside the type's module, no widening of an existing item's visibility (new items stay as narrow as the wiring allows;
  list every re-export or widening you need). CI runs `structure`/`conservation` only for campaigns registered in
  `.rust-guard/stages.toml`; register none unless a widening is unavoidable, and then say so before doing it.
* Future size: `Box::pin` large awaits at the call site; no extra async layers on recursive paths.
* Imports at the top; one primary item per file.
* Machine memory: at most ONE cargo command at a time, always as
  `RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1`; do not pipe cargo output through `tail`/`head` (a pipe
  hides the exit code) — redirect to a file outside the repository and read it.
* Leave no scratch files in the tree.
* Wrap-up: by 150 minutes bring the change to a compiling, formatted, clippy-clean state and stop; say in the final report
  what is missing.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits.

## Observable acceptance

`cargo fmt`, `cargo clippy --all-targets -- -D warnings` for `shamir-server`, `shamir-sync` and every other crate you touched
are clean and the entry counts of every touched folder are within the limit; the export emits stored strings and arrays;
`logical-import` verifies, stages, loads, validates and publishes atomically with the refusals above, leaving nothing behind
on failure; the tests above exist and name what they prove. The final report lists the touched files with their folders,
every widening or re-export, the decisions taken and what is not covered.
