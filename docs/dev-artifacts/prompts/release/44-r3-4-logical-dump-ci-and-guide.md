# Brief: R3 step 4 — CI fixtures, budgets, three-OS coverage and the logical-dump section of the operations guide

Design (settled, read first): `docs/dev-artifacts/design/logical-dump.md` — §5 (CI-only tests and budgets), §6 PR 4, §8 questions 4–6 and
11, and the scope note `docs/dev-artifacts/dev/logical-export-offline-scope.md`. Landed and available: the codec and verifier in
`crates/server/shamir-sync/src/sync/replication_seed/artifact/logical_dump/` (re-exported as `shamir_sync::replication_seed`), the
offline export `crates/server/shamir-server/src/boot/config/diagnostics/logical_dump/` (`shamir-server logical-export`) with its unit
tests, the offline import `.../diagnostics/logical_import/` (`shamir-server logical-import`: preflight, stage, load, validate,
receipt, publish) and the integration tests in `crates/server/shamir-server/tests/admin/logical_import/` (`common.rs`, `roundtrip.rs`,
`corrupt.rs`, `refusals.rs`, `scope.rs`, `identities.rs`). The export supports repositories and tables with the stored value kinds
listed in the scope note, `enable_indexes`, TTL buffer settings, identities and ACLs; it REFUSES indexes, schemas, validators,
functions, groups and guest modules (steps R3-2b and later). This step adds no product behaviour: it measures, pins and documents what
exists.

## Why

Release gate R3 needs a verified, budgeted round trip on all three operating systems and an operator guide. The existing tests prove
correctness on small inputs on the CI matrix; nothing yet proves bounded memory and time on large inputs, canonical determinism, or the
documented workflow.

## What (this step only)

1. **Fixture builder.** One shared test helper that builds, through the engine facade and the query builder only, (a) a small database
   covering every supported stored kind (null, bool, ints, floats, strings, lists, nested maps, binary), several repositories and
   tables, `enable_indexes`, non-default `ttl_ms`, identities and ACL owners; (b) a streaming dataset of about 64 MiB of payload
   (tables with many small records plus a few large values) generated deterministically from a fixed seed in bounded batches, never
   held in memory. Put it next to the existing `logical_import/common.rs` helpers (read them first; reuse, do not duplicate).
2. **Budget tests** (new test files in a NEW sibling folder of the existing ones, see the layout rule): on the 64 MiB dataset export,
   import and standalone verify each finish within 60 s (excluding builds), with a separate 180 s allowance for index work if the
   fixture enables indexes; incremental RSS over the open-database baseline stays at most 256 MiB — measure the process RSS through
   the existing peak-memory helper of `shamir-bench-utils` ONLY if it is already a dev-dependency of `shamir-server`; otherwise assert
   the bounded-streaming property structurally (frame and batch counters, the maximal buffered bytes exposed by the driver progress,
   no whole-table collection) and say so in the report. Assert the framing overhead of at most 36 bytes per item frame plus bounded
   fixed section/trailer framing against the produced artifact. Every await is bounded by a timeout so a regression fails instead of
   hanging.
3. **1 GiB dataset.** Same checks with a 600 s per-operation budget. It must NOT run in every PR: gate it by the environment variable
   `SHAMIR_LOGICAL_DUMP_LARGE=1` (the test returns early with a printed note otherwise; do not add `#[ignore]`), and set the variable
   in an existing scheduled workflow only (`.github/workflows/stress-nightly.yml` — read it; add one step or env entry for
   `shamir-server` on the three OSes if its structure allows, otherwise stop and describe the smallest workflow change). Do not touch
   the PR workflow files.
4. **Pinned properties.** (a) Canonical export determinism: two exports of the same frozen state are byte-identical after the header
   (the diagnostic header fields — snapshot instant, per-repository versions, epoch observation — are the only allowed difference;
   compare through the verifier's header/frames split, not ad-hoc offsets). (b) Verify-only operation on a produced artifact and on a
   handcrafted artifact whose header carries an older epoch observation; the import of the latter succeeds. (c) Exact `ttl_ms`
   round trip. (d) Complete-target exact retry accepted and a changed-option retry refused (already covered on small inputs — extend
   only where the large dataset adds value, do not re-test what `logical_import/roundtrip.rs` already proves). (e) No failed import
   publishes a destination or leaves a stage (before/after directory snapshots), including cancellation mid-load on the 64 MiB
   dataset. List in the report which items of design §5 are NOT applicable yet because the export refuses them (guest modules,
   functions, validators, indexes) and name the step that will cover each.
5. **Three operating systems.** The new tests live in an integration-test target that the CI matrix already runs for `shamir-server`
   on Linux, macOS and Windows (check `scripts/ci/matrix/` selection and `edges.toml`; if the new files would not be selected on all
   three, say exactly why and propose the smallest matrix change — do not edit matrix scripts without saying so first).
6. **Operations guide.** Add a logical-dump section to `docs/guide-docs/guide/07-operations.md`, in the language and style of the
   neighbouring sections, covering: what the dump is and is not (stored shapes; `Dec`/`Big`/`Set` come back as `str`/`list`), the
   `logical-export` and `logical-import` commands with every flag as implemented (read `args.rs` of both), scope options, `--as-db`,
   security (identities imported disabled and passwordless, no credentials, nothing granted implicitly, ACL owner mapping), limits
   and budgets, TTL behaviour, the lock and stage files beside the target and their recovery after a crash, idempotent retry and
   receipts, cancellation, what is refused today and why, and the verify-only workflow. Examples use the real command line; any code
   sample builds queries with the builder, never raw JSON. No private paths.

## Do not

Change product code (export, import, codec, verifier, engine), lift any refusal, add a dependency, add `#[ignore]`, edit the PR
workflow files or matrix scripts without saying so first, or write measured numbers into the docs as facts (budgets are budgets).

## Test fixture checklist (every one of these failed in CI on earlier steps — check each before you finish)

* A persistent system store is required wherever the catalogue is touched: `ShamirDb::init(SystemStoreConfig::Fjall(tempdir/"system"))`;
  open every table (and the system tables via `check_seed_catalogue`) before a facade reads it.
* A database identity on the wire is a canonical base58 `RecordId`; `Manage` is owner/admin-only; create repository `main` and grant
  the owner on `store(db, "main")` in the fixture.
* A batch is capped at 50 queries; never build nested values with `doc!` — use `QueryValue`; unique aliases in a batch builder.
* Close and drop every handle on a Fjall directory before the same directory is opened again; keep one runtime alive per test.
* On Windows a directory rename fails while handles are open; follow the helpers of `logical_import/common.rs`.
* Deterministic tests: handshakes, not sleeps; every await bounded; large fixtures generated in bounded batches.

## Rules that bind the change

* Layout: at most 7 entries per folder, at most 1000 lines per file. `python -I scripts/ci/check_layout.py` counts only TRACKED files:
  count the entries of every folder you add to by hand (`ls`) — `tests/admin/logical_import/` is at the limit, so new test files go
  into a new sibling folder with its own `mod.rs` manifest wired from the parent manifest (count the parent too) — measure `wc -l` of
  every file you touch, name the home of every new file in the report.
* rust-guard `structure`: no logic in `mod.rs`, no inline `#[cfg(test)]` modules, no local `use`, no new `#[allow]`; imports at the top;
  one primary item per file. No campaign is registered.
* Machine memory: at most ONE cargo command at a time, always as
  `RUSTC_WRAPPER= wnice -p idle -c 50 -t 8 -m 50 -- cargo ... -j 1`; do not pipe cargo output through `tail`/`head` — redirect to a
  file outside the repository and read it.
* Leave no scratch files in the tree. Wrap-up: by 150 minutes bring the change to a compiling, formatted, clippy-clean state and stop;
  say in the final report what is missing.

## Policy (verbatim, binding)

Compilation, builds and tests run on GitHub CI, not on this machine. Banned locally: `cargo test`, `cargo nextest`,
`./scripts/test.sh`, `cargo build`, `cargo run`, `cargo bench`, `cargo doc`, npm/tsc builds, any build or test run.
Allowed locally only: `cargo check -p <crate> --all-targets`, `cargo clippy -p <crate> --all-targets -- -D warnings`,
`cargo fmt -p <crate>` (and `-- --check`), `python -I scripts/ci/check_layout.py`.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working
tree or index. Only edit files; the orchestrator commits.

## Observable acceptance

`cargo fmt`, `cargo clippy --all-targets -- -D warnings` for `shamir-server` (and any other crate you touched) are clean and layout
counts are within the limits; the fixture builder, budget tests (64 MiB always, 1 GiB behind the variable), pinned properties and the
guide section exist and name what they prove; no product code changed. The final report lists the touched files with their folders,
the decisions taken (RSS measurement method, workflow change, matrix selection result) and what is not covered.
