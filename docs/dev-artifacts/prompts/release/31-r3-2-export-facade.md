# Brief: R3 step 2 — logical export: read facade, maintenance gate, offline command and admin job (release-1 R3, implementation)

Design: `docs/dev-artifacts/design/logical-dump.md` — read it in full, the "Review" section included. This is step 2 of its
implementation plan (section 6). Sections 1 (existing-state map), 2 (testable requirements), 3 (container and encoding),
4.1 (export protocol), 5 (tests and budgets), 7 (current-code evidence) and 8 (owner defaults) are binding. Step 1 (the
codec and verifier) has landed: use its codec, envelopes, framing, trailer and verifier; do not redefine them.

## Why

Release gate: "a logical export of one version loads into the next". Step 2 produces the export stream from a live or
offline database; step 3 imports it.

## What

1. **Read facade** (design 4.1, section 6 step 2): the export walks the system catalogue (databases, repositories,
   tables with schemas and buffer configuration, index descriptors, functions and validators, security principals and
   ACLs without credentials — design 1.6 and evidence 5) and every table's live records, with logical field names (never
   interner IDs, owner default 2), only live state at the export point (owner default 1). Use existing raw cursors and
   paged reads where possible. Where a bounded, consistent read needs an engine API that does not exist, add only the
   `db_instance` orchestration, table paging and transaction/snapshot facade the design names, and list each with its
   layer and rust-guard consequence.
2. **Maintenance gate** (design 4.1): the export runs under the gate the design specifies (writes refused or fenced for
   the duration, as decided there); the gate is released on every exit path. Name the primitive and prove release on
   error and cancellation in a test.
3. **Surfaces**: the offline CLI command (data directory, server not running — refuse on a running server's lock) and
   the server admin job (typed `BatchOp` + `DbResponse` DTOs through `shamir-query-types` / `shamir-query-builder`,
   permission check reusing the existing admin gate, job status and cancellation). The output is the step-1 container,
   verified by the step-1 verifier before the command reports success.
4. **Tests** (CI only): export of a fixture with every content family of design 3.3 round-trips through the step-1
   verifier; logical names, not interner IDs, appear in the stream; credentials never appear (byte scan for the fixture's
   password and stored keys); deleted records are absent; the gate is released after success, error and cancellation; the
   offline command refuses a locked data directory; the admin job refuses a caller without permission. Budgets of
   design 5 where they apply to export. On Windows never read a file while another handle holds a byte-range lock on it.

## Rules

* Placement (design section 6, counted 2026-11 — count again from a listing): the natural host folders in `shamir-db`,
  `shamir-sync` and `shamir-engine/src/` are at the 7-entry limit; the engine facade goes in the `db_instance` layer (room
  there). Name every touched folder with its counted entries. A regroup needed to make room is its own preparatory
  change, listed separately in the report.
* Note the engine was split since the design was written: `shamir-engine-repo` and `shamir-engine-table` now hold the
  repo and table layers; re-locate every cited path from a fresh search and report the current path.
* Another change (R2a step 1) is adding a consistency-point facade in the same `db_instance` layer right now; keep your
  files separate from `capture_point` / `seal_point` / `replay_to` and do not edit them.
* No new dependency. Query construction is builder-only; no raw JSON in code, tests or docs (serde round-trip tests
  excepted, with a one-line comment).
* At most 7 entries per folder and 1000 lines per file; one primary export per file; `mod.rs` re-exports only; no local
  `use`; no inline test modules; no new `#[allow]`; visibility widening only where listed and declared to rust-guard.
* Keep the change reviewable: if it passes roughly 2500 lines, stop after items 1–2 and the offline command with their
  tests, and report what remains (the admin job then becomes its own step).

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits.

## Acceptance (observed in CI)

fmt, clippy (`-D warnings`, every touched crate, `--all-targets`) and layout clean locally; the touched crates' tests
green in CI on the three OS. The final report lists every file created and changed (preparatory moves apart), every
engine API added with its layer, every widened item with its declaration, and every deviation from the design with its
reason.
