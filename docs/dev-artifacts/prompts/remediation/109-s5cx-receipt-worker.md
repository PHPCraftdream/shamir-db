# Brief — S5c-X PR6b-2b structured repository worker lifecycle foundation (inactive)

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Layout limits (CLAUDE.md, ratchet): at most 7 direct entries per folder and at most 1000 lines per source file. Existing violations are grandfathered but must not grow: put new logic into NEW files / subfolders instead of extending an over-limit file or folder, and never push a new file past 1000 lines or a folder past 7 entries.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.


## Scope discipline

If a file outside the FILE SCOPE is strictly required to make the change compile or to keep an existing test meaningful (a caller of a changed signature, a fixture, a manifest line), edit it minimally and LIST it in your final message; do not stop to ask. Ask (and stop) only for genuine design decisions or when the scope would grow into another task area.

## FILE SCOPE (only these paths may be edited)

- a NEW module for the repository-owned worker lifecycle inside an existing subfolder with room (for example `crates/shamir-engine/src/tx/follower_durability/` has 7 entries: put it in a NEW subfolder of an existing subfolder with room, or under `crates/shamir-engine/src/repo/<existing subfolder>/`; verify counts with `git ls-tree --name-only origin/master:<dir> | wc -l`; `tx/` and `repo/` are over the folder limit and must not gain entries)
- `crates/shamir-engine/src/repo/repo_instance.rs` (over the line limit: net zero or negative) and the repository foreground/background liveness split (`repo_instance.rs:89-120`) only to own the lifecycle owner and the resource lease; shutdown/close wiring (one absolute deadline)
- `crates/shamir-tx/src/version_guard.rs` only if the fail-closed unsettled drop needs a public entry (prefer the existing one: `version_guard.rs:139-144,168-173`)
- tests in NEW `tests/` subfolders; docs: a short lifecycle section in `docs/guide-docs/follower-journal-durability.md`
- NOT in scope: receipt apply activation (the worker runs only synthetic jobs in tests), the admission domain (PR6b-2a, parallel), leader commit path changes, benchmarks

## Task

MERGED ALREADY (master `d615d123`): PR1 (#161 `durability:` settings, serve-durable bound min(confirmed, W, R)), PR3 (#159 WAL entry v3 + ONE `event_from_wal_entry`), PR7 (#160 lineage + history-epoch fence), PR1c (#163 typed options, query language v4), PR2 (#165 runtime clean-horizon checkpoint + WAL retention policy), PR1b (#164 per-repository durability override: `DurabilityOverride`, CREATE/ALTER/DESCRIBE REPO, live swap; `completed_reseed` is currently REJECTED as not implemented by `resolve`), PR5 (#167 runtime orphan repair, `tx/orphan_repair/`), PR6 (#168 follower journal barrier + atomic J+bookmark record, `tx/follower_durability/`, shared bookmark codec), PR4 (#166 cold reconciliation `tx/cold_reconcile/`, `shamir-tx/src/changefeed/cold/`). PR6c (#169), PR8 (#170), PR6b-1 (#171 atomic batch, publication-safe MVCC, receipt codecs, inactive), PR5b (#172 channel-full repair). Read those modules first.
LESSONS (do not repeat; all cost CI rounds before): the scc `Guard` is not `Send`: never hold it across an `.await` (confine it in a synchronous helper); `Arc<Drainer>` vs `&Drainer` borrows; `bool::then` in `filter_map` and `!x.is_some_and(..)` trigger clippy (`filter_map_bool_then`, `nonminimal_bool`); `clippy::absolute_paths` max 5 segments: import `test_seam`-style paths; test modules MUST be `#[cfg(test)]`-gated and must not use dev-only APIs from non-test code; `RecordKey`/`KeyBytes` is Clone, NOT Copy (clone before reuse); `shamir-server` does not depend on `shamir_engine`: import through `shamir_db::engine::...` / `shamir_db::storage::...`; `Display` has no `replace`; reopen-after-drop tests must RETRY the store open (Fjall lock is released a few ms after drop; use the retry pattern of the sibling tests); module wiring in `shamir-engine` uses `#[path]` files (check how neighbours load before adding a file); no `scc::TreeIndex` iterate-and-remove; tests wait for conditions, never sleep; pre-existing tests must keep their meaning: read EVERY existing test touching your area BEFORE you finish (CI reports failures one stage at a time and each stage costs about 15 minutes).
LAYOUT RATCHET (checked by the orchestrator on the diff, changed paths only): at most 7 direct entries per folder, at most 1000 lines per file; a folder or file ALREADY over the limit must not grow. `crates/shamir-engine/src/tx/` (20+) and `crates/shamir-engine/src/repo/` (18) are already over: put new modules inside an EXISTING subfolder that has room (for example under `tx/cold_reconcile/` or `tx/clean_horizon/` if they are thematically right and have room; verify counts with `git ls-tree --name-only origin/master:<dir> | wc -l`) instead of adding another sibling folder. `repo_instance.rs`, `changefeed.rs`, `drainer.rs`, `apply_replicated.rs` are over the line limit: net-zero or negative diff there; new logic in new files.

CONTEXT: the receipt-based follower apply (S5c-X PR6b-2) lives on the open draft PR #180 (branch `feat/s5cx-follower-receipt-2`). Five review rounds failed on two structural points, so the binding design below splits them out as foundations that merge FIRST, from master, independently of #180. You may read #180 read-only for reference (`git show origin/feat/s5cx-follower-receipt-2:<path>`), but do NOT copy its `ddl_admission` mutex or its worker: they are exactly what the design replaces. Do NOT activate receipt apply in this PR.

PR6b-2b of S5c-X, split item (2) of the binding design appended below (section 2 "Receipt worker lifecycle" and the merge-order table in section 3).

REQUIRED (implement section 2 literally; generic over the job type so PR6b-2c plugs the receipt pipeline in)
1. Three objects: the repository LIFECYCLE OWNER (sole sender, sole join handle, close state, cached terminal result), the worker EXECUTION STATE (receiver, repository resource lease, active/parked state; no sender, no lifecycle-owner reference), the JOB (payload + reply sender only; no foreground `RepoInstance`, no owner `Arc`, no sender). The resource lease excludes the lifecycle owner and the foreground liveness token. Initialization installs the task handle and lifecycle state without an intervening cancellation point; close-before-initialize permanently prevents a later spawn.
2. States `Open -> Closing -> Joined(result)` plus a sticky `quarantined` flag. Admission close is synchronous and atomic with submission (one control critical section; successful enqueue is the acceptance point). All accepted jobs drain FIFO; caller cancellation drops only the reply receiver; each accepted job reaches its endpoint (publication / durable pause / fail-closed terminal). Join exactly once: serialized `Option<JoinHandle>`, on Ready take the handle and store the terminal result in the same non-awaiting step; repeated/concurrent shutdown returns the cached result; a timed-out wait leaves the handle stored; cancelling a shutdown waiter does not cancel close. One absolute deadline for worker shutdown + commit quiescence + flush + checkpoint.
3. Timeout or panic: quarantine UNCONDITIONALLY (never decided by a parked-guard count), close all admission, set sticky reopen-required, forbid later publication from the old worker generation through a small publication/termination interlock (either publication completes first or quarantine wins), keep ownership until the worker finishes; reopen only after join and lease release. Guard cleanup preserves unresolved versions (use the fail-closed unsettled drop, never an ordinary abort).
4. Drop: dropping the last public owner synchronously closes admission and signals close; a supervisor keeps the join handle and lease until the worker reaches its safe endpoint and is joined; physical resource lifetime ends only after join; explicit async shutdown guarantees exit on success; no abort in Drop. Document exactly this contract (do not promise more).
5. Tests (CI, deterministic: barriers/notifies and paused time, no sleeps, no yield budgets), with SYNTHETIC jobs: cancellation at hook points; repeated and concurrent shutdown (success and failed join observed once, later calls cached, no repoll); timeout with locally held guards and zero parked guards still quarantines and blocks publication/admission and retains resources until join; queue close race (4 queued + 1 active, accepted jobs get defined outcomes, later submissions rejected, none disappear); drop while idle/queued/active (supervisor observes exit and resource release, no sender/owner cycle: check with a `Weak`); close racing first initialization; worker panic quarantines before unsafe guard cleanup.
6. Docs: the lifecycle contract.


S5c-X is the owner-approved FULL plan to remove the false-positive replication GAPs after an unclean shutdown by making the WAL the source of truth for journal events. The binding design (invariants I1-I3, settings, PR plan) is appended below: read it first, then read the real code; verify every claim and line number (they drift). Master already contains VIS-1..5, SEED-1..5, S5a-S5c-2. Sibling PRs of this plan run concurrently or later and own other scopes (PR1 = I3 + `durability:` settings + policy plumbing, PR2 = runtime J checkpoint + retention enforcement, PR3 = derivable WAL entry (I1), PR4 = cold reconciliation): stay strictly inside YOUR scope. You may use child agents for edits (give each an exact file scope and the same prohibitions; read their diffs yourself). NO builds or tests of any kind (not even `cargo check`/`clippy`/`nextest`): only CI compiles and tests, one CI cycle costs about an hour, and earlier agents shipped 5-15 compile errors per PR. The ONLY cargo command you may run is `cargo fmt -p <crate>` for each crate you touched, as the very last step (CI fails the whole PR on formatting drift; this happened twice in the previous PRs). Imports at the top, one primary export per file, `mod.rs` re-exports only, tests under `tests/` dirs, builder-only queries (wire-format docs excepted). Your git writes are refused by the runtime: the orchestrator commits, pushes, opens the PR and feeds you the CI failures in later rounds; do not wait for CI.

COMPILE-READINESS (mandatory): before finishing re-read every new/changed file as the compiler would. Every `use` resolves AND is visible from that module (private `mod`, `pub(crate)`, cross-crate `pub`; a `pub(super)` item cannot be re-exported wider); every called function exists with exactly that arity, receiver and types (grep the real definition; never recall); every struct literal has all fields (adding a field to a public struct breaks every literal and exhaustive match: grep them ALL, incl. tests, benches, examples and other crates); `Send` across `.await`; derive requirements. NO `serde_json` in unit tests of `shamir-query-types`/`shamir-db`/`shamir-engine`/`shamir-server` unless the crate's Cargo.toml already lists it (never add a dependency; Cargo.lock is checked with `--locked`). Lints that fail CI with `-D warnings`: `clippy::absolute_paths` (the configured maximum is 5 segments: import anything longer), `ptr_arg` (`&Vec<T>`/`&String` params), `type_complexity`, `too_many_arguments`, `large_enum_variant`, `needless_borrow`, `redundant_clone`, `field_reassign_with_default`, `unused_imports`, `dead_code` ALSO on Windows for `cfg(test)`/`cfg(unix)`-only helpers, `unused_variables`, `unused_must_use`, and the workspace `disallowed_methods` (`scc::*::len()` is banned: keep an atomic mirror). Tests run on Linux, macOS AND Windows under nextest (180 s per-test kill): no wall-clock sleeps, no sampling of a value right after another becomes visible (wait for the CONDITION; a bookmark/flush/fsync may land after the data), no assumptions about file ordering or Unix-only paths, deterministic paused tokio time where time matters, every spawned task joined, every temp dir cleaned. A negative control (a variant of the test with the new protection removed must fail) is required wherever the brief says so; write it as a test that exercises the unprotected path through an explicit test seam, not by editing code in the test.

HOT-PATH RULES (the owner postponed all benchmarks, so be conservative and honest): the commit path may gain at most the cost stated in your task; no lock, no allocation per commit beyond what is stated, no `Instant::now()` pair, no extra fsync, no `std::sync::Mutex`/`RwLock` (use atomics / `arc_swap`); do NOT inline large futures into the commit / `QueryRunner` async chain (debug-build tests run on a 2 MiB stack: box them or move work into small non-async helpers). In your FINAL MESSAGE state explicitly which commit-hot-path code you changed and what it costs, so the PR description can say the step is unbenchmarked.

DOCUMENTATION (mandatory, same change): update the existing docs your task names so they match the new behaviour exactly (no stale claims left; grep for the old claims), keep language and style of each file (some are Russian), builder-only snippets, no private machine paths.

LAYOUT RULE (CLAUDE.md, ratchet): at most 7 direct entries per folder, at most 1000 lines per source file. The orchestrator checks the diff after you finish. Over-limit files/folders (for example `repo_instance.rs` 2265 lines, `commit.rs` 1183, `pre_commit.rs` 2805, `changefeed.rs` 1132, `drainer.rs` 1711, `crates/shamir-engine/src/repo/`, `crates/shamir-engine/src/tx/`, `crates/shamir-wal/src/`, `crates/shamir-server/src/`) must NOT grow beyond the smallest call-site edit (a field, a one-line call, a re-export): ALL new logic goes into NEW files inside NEW subfolders. Files under 1000 lines (`config.rs` 986, `batch_execute.rs` 951, `wal_group_commit.rs` 511, `wal_entry_v2.rs` 265, `core.rs` 853) must stay under 1000: prefer a new file there too.

# Design reviewer answer (BINDING): repository write-admission domain and receipt worker lifecycle

## Verdict — request changes; replace the admission and lifecycle structures before merging #180

**Binding recommendation:** use **one repository-owned, capability-enforced mutation domain**, inherited by its tables—not facade-level locking—and a **structured receipt worker whose lifetime is owned outside the worker’s execution state**. Do not continue adding individual facade guards.

Split admission and lifecycle foundations out first. Keep receipt materialization, counters, journal coordination, publication and cold recovery together for activation.

**Evidence status:** OBSERVED below means source inspected, not execution. INFERRED identifies consequences of that source. All branch references below refer to `origin/feat/s5cx-follower-receipt-2`; existing table-barrier references are to this checkout. No runtime failures were reproduced.

## 1. Repository write-admission domain

### Why the present approach cannot converge

**OBSERVED:**

- Receipt apply takes the marker mutex and then the separate repository DDL mutex: `crates/shamir-engine/src/tx/follower_durability/receipt/apply.rs:77–78`.
- Public validator mutation instead takes only `bindings_lock`: `crates/shamir-engine/src/table/table_manager_validators.rs:36–70`.
- Public index drop goes through the table barrier: `crates/shamir-engine/src/table/table_manager_index_mgmt.rs:894–907`. That barrier acquires a mutex allocated separately for each table: `crates/shamir-engine/src/table/table_manager.rs:558,1293–1320`.
- Migration creates the destination with `db.add_repo` before acquiring source admission, then resolves and mutates the destination: `crates/shamir-db/src/shamir_db/execute/admin_migration.rs:137–164`.
- Counter persistence has its own lock and writes the cached value: `crates/shamir-engine/src/table/record_counter.rs:174–192`. The PR’s `adopt_absolute` takes that lock only during cache adoption: `crates/shamir-engine/src/table/record_counter.rs:154–167`.

**INFERRED:** neither public table mutation nor counter persistence is structurally contained by the new repository mutex. Protecting adoption alone does not protect the interval between the atomic durable counter write and adoption.

### Chosen design: repository mutation capability backed by one Tokio mutex

**REQUIRED DESIGN:**

Introduce `RepoWriteDomain`, shared through `Arc`, owned by the repository’s resource state and injected into every table, counter and mutable repository registry **at construction**, before recovery or publication.

Its exclusion primitive is:

**`Arc<tokio::sync::Mutex<()>>`, returning an opaque, repository-identity-bearing `RepoWritePermit`.**

Use exclusive admission for receipt apply, follower-local mutation, DDL and counter persistence. **Do not introduce a repository RwLock into the leader commit path.** Concurrent local writes inside a receipt-enabled follower are not a performance requirement that justifies another locking protocol.

The essential enforcement is the **permit**, not the mutex:

1. Public mutation entry points acquire admission.
2. Their mutation implementations require `&RepoWritePermit`.
3. Nested operations pass the same permit; they never reacquire the mutex.
4. Lower-level mutation surfaces—registry mutation, validator persistence, retention changes, table create/drop/rename, counter writes and repair/recovery mutations—require the permit or are inaccessible outside the admitted implementation.
5. Validate that the permit belongs to the exact repository instance, not merely a matching name.

**Adding a parent mutex inside `begin_write_barrier` is necessary but insufficient.** Validator operations do not currently enter that barrier. Raw registry/store access also defeats any “every DDL already passes here” premise. Capability-requiring mutation APIs must close those exits.

### Exact ownership and ordering

Use this hierarchy:

1. **Repository write permit.**
2. **Marker persistence mutex**, if needed.
3. **Per-table DDL admission**, in stable table-token order when acquiring multiple tables.
4. Existing barrier sequence: **raise intent → drain writers → acquire unique-write locks**.
5. **`bindings_lock` or counter `persist_lock`**, as the terminal subsystem lock; no call from these locks may acquire an earlier lock.

Preserve the existing drain-before-unique-lock requirement at `crates/shamir-engine/src/table/table_manager.rs:1310–1319`. The existing transactional lock-order explanation also requires unique-write locks before `commit_lock`: `crates/shamir-engine/src/tx/pre_commit.rs:268–275`.

Consequently:

- Reverse the receipt’s current **marker → repository** acquisition.
- Public `begin_write_barrier` acquires repository admission and returns a compound guard retaining it. Its admitted implementation accepts an existing permit.
- Validator add/remove acquire repository admission **before** `bindings_lock`.
- Receipt apply acquires one permit before bookmark lookup/translation/allocation and retains it through publication or transfer into a paused-receipt owner.
- Counter persistence acquires the repository permit **before sampling the cache**. Its admitted variant is used by code already holding admission.
- Do not wrap leader commits in this new mutex or read lock. Receipt-enabled follower mode must be fixed before publication; follower-local write entry points use admitted implementations. Switching a writable leader into receipt mode requires close/quiesce/reopen, not a concurrent mode flip.

**Important constraint:** if arbitrary leader writes must coexist with receipt apply on the same repository, “leader hot path gains nothing” is incompatible with introducing a new exclusion domain those writes do not participate in. This design explicitly forbids that coexistence.

### Table membership, migration, recovery and paused receipts

- **Create/drop/rename:** mutate repository table membership under the same permit. A receipt must not snapshot the table list and then try to lock that changing set.
- **Migration:** construct the destination privately; acquire both repository permits in stable **repository-instance identity order**, not source→destination order; initialize it before registry publication. A→B and B→A must acquire the same ordering. Equal repository identities are acquired once or rejected.
- **Boot replay:** construct the domain before table recovery, pass an initialization/admission permit through recovery, and publish the repository only after recovery completes. Lazy table initialization must inherit an existing permit when invoked from admitted code, avoiding recursive acquisition.
- **Paused receipt:** releasing the mutex must **not** permit conflicting DDL or stale counter persistence. Keep an outstanding-receipt reservation in the domain. While it exists, ordinary mutations are rejected or wait; only matching resume, recovery and close are admitted. An idle worker is not evidence that the repository is free to mutate.

That last rule matters because the PR deliberately retains unpublished versions across errors: `crates/shamir-engine/src/tx/follower_durability/receipt/apply.rs:225–258`.

### Completeness proof required in CI

**Enumeration tests alone cannot prove future completeness.** Require both:

- **Compile-time boundary:** mutation implementations require an unforgeable permit; mutable internals are private behind that boundary. Add compile-fail coverage for permit-free mutation access. A debug assertion must check the supplied permit’s identity—not `try_lock` or “some task holds the mutex.”
- **Deterministic behavioural matrix:** hold admission and invoke every public mutation family directly, including `TableManager`, not just the facade. Assert no durable or RAM mutation occurs before admission. Cover validators, every index family, schema, retention, counters, table membership, migration destination publication and recovery.

Also require:
- DDL rejected/blocked while a receipt is paused.
- Counter persistence parked between durable materialization and cache adoption cannot overwrite the absolute count.
- Opposite-direction migrations and cross-table DDL/writers complete under barrier-controlled scheduling.

## 2. Receipt worker lifecycle

### What the current branch actually contains

**OBSERVED:**

- `shutdown_core` polls a stored `JoinHandle` but never removes it after completion: `crates/shamir-engine/src/tx/follower_durability/receipt/resume/worker.rs:223–249`.
- Timeout quarantine is conditional on `parked_guards` being nonempty: `crates/shamir-engine/src/tx/follower_durability/receipt/resume/worker.rs:353–371`.
- The loop no longer directly owns the sender-bearing worker. However, each job owns both a `RepoInstance` and `Arc<ReceiptWorker>`: `crates/shamir-engine/src/tx/follower_durability/receipt/resume/worker.rs:44–53,143–150`.

**INFERRED:**

- A subsequent shutdown can repoll an already-completed join handle.
- Timeout while guards remain local to the active pipeline can skip quarantine entirely.
- The revised loop ownership improves idle-drop behaviour, but queued/active jobs still retain sender-bearing ownership. “The loop has no worker Arc” is not a sufficient lifetime proof.

### Chosen structure

Separate three objects:

| Object | Ownership |
|---|---|
| **Repository lifecycle owner** | Sole sender, sole join handle, close state and cached terminal result |
| **Worker execution state** | Receiver, repository resource lease, active/parked receipt state; **no sender or lifecycle-owner reference** |
| **Job** | Request payload and reply sender only; **no foreground `RepoInstance`, worker-owner Arc or sender** |

The repository resource lease must exclude the lifecycle owner and foreground liveness token. The existing foreground/background distinction provides a useful precedent: `crates/shamir-engine/src/repo/repo_instance.rs:89–120`.

Worker initialization must install the task handle and lifecycle state without an intervening cancellation point. Close-before-initialize must permanently prevent a later spawn.

### Close and shutdown contract

Use lifecycle states:

**`Open → Closing → Joined(result)`**, with a sticky **quarantined** flag independent of join completion.

1. **Close admission synchronously and atomically with submission.** Sender access and the Open/Closing decision share one control critical section. A successful enqueue is the acceptance linearization point.
2. **Drain all accepted jobs in FIFO order.** Caller cancellation drops only the reply receiver. Each accepted job reaches publication, a durable recoverable pause, or a fail-closed terminal state.
3. Once the first job pauses with an outstanding receipt, later jobs cannot bypass that reservation. They may resume the matching receipt or receive a typed paused/closing response.
4. **Join exactly once.** Serialize access to `Option<JoinHandle>`. On `Ready`, take the handle and store the terminal result in the same non-awaiting step. Repeated/concurrent shutdown returns the cached result.
5. Timeout while awaiting `&mut JoinHandle` leaves the uncompleted handle stored. Cancellation of a shutdown waiter does not cancel close or lose ownership.
6. One absolute deadline covers worker shutdown, commit quiescence, flush and checkpoint—not a fresh budget for each phase.

### Timeout and quarantine

**On any shutdown timeout or worker panic, quarantine unconditionally.** Do not inspect the parked-guard count to decide whether the repository is unsafe.

Before returning timeout:

- Close all mutation admission.
- Set sticky reopen-required/quarantine in shared repository state.
- Prevent subsequent publication from the old worker generation.
- Keep ownership of the worker and its resources until it finishes.

The final synchronous publication section and quarantine must share a small publication/termination interlock. Either publication completes first, or quarantine wins and publication is prohibited. An atomic check followed by unprotected publication is not sufficient.

The active operation may finish already-issued storage I/O and leave a durable receipt for cold recovery. It must not clear quarantine or resume serving. Guard cleanup must preserve unresolved receipt versions; do not falsely settle them as aborted merely to empty a vector. Existing `VersionGuard` already distinguishes fail-closed, unsettled follower drops from ordinary aborts: `crates/shamir-tx/src/version_guard.rs:139–144,168–173`.

**Reopen is permitted only after the old worker has joined and released its storage leases.** A timeout is not permission to open a competing repository instance.

### Drop: the necessary precise contract

**A synchronous Rust `Drop` cannot guarantee both graceful completion of arbitrary async I/O and “the task has already exited when Drop returns.” Do not promise that.**

The binding contract should be:

- Dropping the last public repository owner synchronously closes admission and signals close.
- A repository task supervisor retains the join handle and resource lease until the worker reaches its safe endpoint and is joined.
- The repository’s **physical resource lifetime ends only after join**. No receipt task is detached or outlives that resource lifetime.
- Explicit async shutdown provides the guarantee that, on success, the task has exited.
- Do not abort the active pipeline in `Drop`: abort cannot undo issued storage I/O or already-started blocking work.

If the product insists on **zero running tasks immediately after dropping the public handle**, require successful async shutdown before destruction. That stronger requirement cannot honestly be implemented as ordinary nonblocking `Drop`.

### Minimal CI lifecycle tests

1. **Cancellation:** cancel callers at Prepared, Materialized and pre-publication hooks; accepted work completes or pauses with identical allocation.
2. **Repeated/concurrent shutdown:** successful and failed joins are observed once; later calls return the cached result without repolling.
3. **Timeout with locally held guards:** parked count is zero, active pipeline is blocked; timeout still quarantines, blocks publication/admission, and retains resources until join.
4. **Queue close race:** four queued jobs plus active work; accepted jobs receive defined outcomes, later submissions are rejected, none disappear.
5. **Drop ownership:** last public owner dropped while idle, queued and active; supervisor observes exit and resource release; no sender/owner cycle remains.
6. **Initialization race and panic:** close racing first initialization cannot spawn afterward; worker panic quarantines before unsafe guard cleanup.

Use barriers/notifies and paused time; validate durable bytes, publication bounds, allocation and task/resource destruction—not only returned errors.

## 3. Safe merge order

**Yes—split the structural foundations out. Do not split the activated receipt transaction across several merges.**

| Order | PR | Safe merge condition |
|---|---|---|
| 1 | **Repository admission and mutation-capability boundary** | Existing follower behaviour remains; all mutation families enter the domain, with ordering/completeness tests. No new receipt writes or leader commit locking. |
| 2 | **Structured worker/lifecycle foundation** | Worker remains inactive for production receipt apply; synthetic CI jobs establish cancellation, close, timeout, panic and drop contracts. |
| 3 | **Complete receipt apply activation** | Materialization, absolute counters, journal writer coordination, publication, paused reservations and cold recovery activate **together**, with kill-subprocess crash proofs. |
| 4 | **Existing-bookmark migration and rollback tooling—PR6b-3** | Explicit conversion, receipt completion and downgrade validation; legacy repositories retain their prior path until conversion. |

The activation PR must not write a receipt format that the same merged version cannot recover. Its crash harness must reopen twice and replay twice, covering exact allocation, history/timestamps, counters, downstream bytes and snapshots.

This preserves the binding design’s coherent activation unit in `docs/dev-artifacts/prompts/remediation/106-s5cx-follower-receipt-2.md`, while extracting the two foundations responsible for repeated review failures.

**Files changed:** none. **Builds/tests run:** none, as requested; all proposed verification belongs in GitHub CI. **Recommendation:** block #180 until these admission and lifecycle contracts replace the current wrapper-based structures.

