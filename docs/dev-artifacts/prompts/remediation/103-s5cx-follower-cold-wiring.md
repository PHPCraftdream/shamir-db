# Brief — S5c-X PR6c wire the follower in_flight_range into cold-start certification

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

- `crates/shamir-engine/src/tx/recovery.rs`, `crates/shamir-engine/src/repo/repo_instance.rs` (smallest call-site edits only; both over the line limit: net-zero or negative diff), the cold reconciliation module `crates/shamir-engine/src/tx/cold_reconcile/` (new logic in NEW files inside it, keep its folder at most 7 entries)
- `crates/shamir-engine/src/tx/follower_durability/` (PR6 helpers: `in_flight_range`) and the shared bookmark codec in `crates/shamir-engine/src/meta/` (read `load_clean_horizon`)
- tests in NEW `tests/` subfolders; docs: `docs/guide-docs/follower-journal-durability.md` and the PR4 sections of `09-seed-recovery.md` / `07-operations.md` / `KNOWN_LIMITATIONS.md` / `REPLICATION.md` (exact wording only)
- NOT in scope: retry-safe batch receipts and idempotent counters (PR6b), `completed_reseed` (PR8), benchmarks

## Task

MERGED ALREADY (master `a72f3f09`): PR1 (#161 `durability:` settings, serve-durable bound min(confirmed, W, R)), PR3 (#159 WAL entry v3 + ONE `event_from_wal_entry`), PR7 (#160 lineage + history-epoch fence), PR1c (#163 typed options, query language v4), PR2 (#165 runtime clean-horizon checkpoint + WAL retention policy), PR1b (#164 per-repository durability override: `DurabilityOverride`, CREATE/ALTER/DESCRIBE REPO, live swap; `completed_reseed` is currently REJECTED as not implemented by `resolve`), PR5 (#167 runtime orphan repair, `tx/orphan_repair/`), PR6 (#168 follower journal barrier + atomic J+bookmark record, `tx/follower_durability/`, shared bookmark codec), PR4 (#166 cold reconciliation `tx/cold_reconcile/`, `shamir-tx/src/changefeed/cold/`). Read those modules first.
LESSONS (do not repeat; all cost CI rounds before): the scc `Guard` is not `Send`: never hold it across an `.await` (confine it in a synchronous helper); `Arc<Drainer>` vs `&Drainer` borrows; `bool::then` in `filter_map` and `!x.is_some_and(..)` trigger clippy (`filter_map_bool_then`, `nonminimal_bool`); `clippy::absolute_paths` max 5 segments: import `test_seam`-style paths; test modules MUST be `#[cfg(test)]`-gated and must not use dev-only APIs from non-test code; `RecordKey`/`KeyBytes` is Clone, NOT Copy (clone before reuse); `shamir-server` does not depend on `shamir_engine`: import through `shamir_db::engine::...` / `shamir_db::storage::...`; `Display` has no `replace`; reopen-after-drop tests must RETRY the store open (Fjall lock is released a few ms after drop; use the retry pattern of the sibling tests); module wiring in `shamir-engine` uses `#[path]` files (check how neighbours load before adding a file); no `scc::TreeIndex` iterate-and-remove; tests wait for conditions, never sleep; pre-existing tests must keep their meaning: read EVERY existing test touching your area BEFORE you finish (CI reports failures one stage at a time and each stage costs about 15 minutes).
LAYOUT RATCHET (checked by the orchestrator on the diff, changed paths only): at most 7 direct entries per folder, at most 1000 lines per file; a folder or file ALREADY over the limit must not grow. `crates/shamir-engine/src/tx/` (20+) and `crates/shamir-engine/src/repo/` (18) are already over: put new modules inside an EXISTING subfolder that has room (for example under `tx/cold_reconcile/` or `tx/clean_horizon/` if they are thematically right and have room; verify counts with `git ls-tree --name-only origin/master:<dir> | wc -l`) instead of adding another sibling folder. `repo_instance.rs`, `changefeed.rs`, `drainer.rs`, `apply_replicated.rs` are over the line limit: net-zero or negative diff there; new logic in new files.

PR6c of S5c-X: PR6 stored the journal horizon J together with the upstream bookmark on a WAL-less follower and added a pure `in_flight_range` helper, but cold-start certification does not use it yet. Today a follower (no WAL) that crashes mid-batch is certified with the old conservative gap over the whole interval after the clean horizon. Wire it so only the batch that was truly in flight is uncertain.

REQUIRED
1. Read first: PR6's `tx/follower_durability/` (`barrier.rs`, `batch.rs`, `record.rs`, `in_flight_range`) and its docs, the shared bookmark codec (`meta/`, `load_clean_horizon`; the three formats: legacy u64, v1 fence, v2 bookmark+J), PR4's cold reconciliation (`tx/cold_reconcile/`, `repo/cold_reconcile/`) and the follower branch of `certify_cold_start` / `recovered_open.rs`, how a follower repo is recognised at open (no WAL).
2. At follower cold start: if the stored record has a known J, certify events up to J as complete and mark ONLY `in_flight_range(J, recovered floor)` as an uncertain minimal GAP; unknown J (old record) keeps today's conservative behaviour exactly; a repo that is not a follower is untouched. Keep the order: reconcile/flush the journal, persist the clean horizon through the PR2 checkpoint, THEN serve. Any I/O or decode error refuses open.
3. Tests (deterministic): crash points of a follower batch (after history write, after event write, after journal flush, after the J+bookmark record) each reopened TWICE: the GAP after reopen is exactly the in-flight batch (or none if the record landed), a downstream pull sees no missing event outside it; unknown-J record keeps the old conservative GAP; non-follower repos unchanged; NEGATIVE CONTROL (ignoring J reproduces the wide GAP). Existing follower/PR4/PR6/SEED tests keep their meaning.
4. Docs: the follower paragraph states the exact new behaviour and the remaining limitation (PR6b retry convergence).
5. Hot path: none (cold path only); say so.


S5c-X is the owner-approved FULL plan to remove the false-positive replication GAPs after an unclean shutdown by making the WAL the source of truth for journal events. The binding design (invariants I1-I3, settings, PR plan) is appended below: read it first, then read the real code; verify every claim and line number (they drift). Master already contains VIS-1..5, SEED-1..5, S5a-S5c-2. Sibling PRs of this plan run concurrently or later and own other scopes (PR1 = I3 + `durability:` settings + policy plumbing, PR2 = runtime J checkpoint + retention enforcement, PR3 = derivable WAL entry (I1), PR4 = cold reconciliation): stay strictly inside YOUR scope. You may use child agents for edits (give each an exact file scope and the same prohibitions; read their diffs yourself). NO builds or tests of any kind (not even `cargo check`/`clippy`/`nextest`): only CI compiles and tests, one CI cycle costs about an hour, and earlier agents shipped 5-15 compile errors per PR. The ONLY cargo command you may run is `cargo fmt -p <crate>` for each crate you touched, as the very last step (CI fails the whole PR on formatting drift; this happened twice in the previous PRs). Imports at the top, one primary export per file, `mod.rs` re-exports only, tests under `tests/` dirs, builder-only queries (wire-format docs excepted). Your git writes are refused by the runtime: the orchestrator commits, pushes, opens the PR and feeds you the CI failures in later rounds; do not wait for CI.

COMPILE-READINESS (mandatory): before finishing re-read every new/changed file as the compiler would. Every `use` resolves AND is visible from that module (private `mod`, `pub(crate)`, cross-crate `pub`; a `pub(super)` item cannot be re-exported wider); every called function exists with exactly that arity, receiver and types (grep the real definition; never recall); every struct literal has all fields (adding a field to a public struct breaks every literal and exhaustive match: grep them ALL, incl. tests, benches, examples and other crates); `Send` across `.await`; derive requirements. NO `serde_json` in unit tests of `shamir-query-types`/`shamir-db`/`shamir-engine`/`shamir-server` unless the crate's Cargo.toml already lists it (never add a dependency; Cargo.lock is checked with `--locked`). Lints that fail CI with `-D warnings`: `clippy::absolute_paths` (the configured maximum is 5 segments: import anything longer), `ptr_arg` (`&Vec<T>`/`&String` params), `type_complexity`, `too_many_arguments`, `large_enum_variant`, `needless_borrow`, `redundant_clone`, `field_reassign_with_default`, `unused_imports`, `dead_code` ALSO on Windows for `cfg(test)`/`cfg(unix)`-only helpers, `unused_variables`, `unused_must_use`, and the workspace `disallowed_methods` (`scc::*::len()` is banned: keep an atomic mirror). Tests run on Linux, macOS AND Windows under nextest (180 s per-test kill): no wall-clock sleeps, no sampling of a value right after another becomes visible (wait for the CONDITION; a bookmark/flush/fsync may land after the data), no assumptions about file ordering or Unix-only paths, deterministic paused tokio time where time matters, every spawned task joined, every temp dir cleaned. A negative control (a variant of the test with the new protection removed must fail) is required wherever the brief says so; write it as a test that exercises the unprotected path through an explicit test seam, not by editing code in the test.

HOT-PATH RULES (the owner postponed all benchmarks, so be conservative and honest): the commit path may gain at most the cost stated in your task; no lock, no allocation per commit beyond what is stated, no `Instant::now()` pair, no extra fsync, no `std::sync::Mutex`/`RwLock` (use atomics / `arc_swap`); do NOT inline large futures into the commit / `QueryRunner` async chain (debug-build tests run on a 2 MiB stack: box them or move work into small non-async helpers). In your FINAL MESSAGE state explicitly which commit-hot-path code you changed and what it costs, so the PR description can say the step is unbenchmarked.

DOCUMENTATION (mandatory, same change): update the existing docs your task names so they match the new behaviour exactly (no stale claims left; grep for the old claims), keep language and style of each file (some are Russian), builder-only snippets, no private machine paths.

LAYOUT RULE (CLAUDE.md, ratchet): at most 7 direct entries per folder, at most 1000 lines per source file. The orchestrator checks the diff after you finish. Over-limit files/folders (for example `repo_instance.rs` 2265 lines, `commit.rs` 1183, `pre_commit.rs` 2805, `changefeed.rs` 1132, `drainer.rs` 1711, `crates/shamir-engine/src/repo/`, `crates/shamir-engine/src/tx/`, `crates/shamir-wal/src/`, `crates/shamir-server/src/`) must NOT grow beyond the smallest call-site edit (a field, a one-line call, a re-export): ALL new logic goes into NEW files inside NEW subfolders. Files under 1000 lines (`config.rs` 986, `batch_execute.rs` 951, `wal_group_commit.rs` 511, `wal_entry_v2.rs` 265, `core.rs` 853) must stay under 1000: prefer a new file there too.

## S5c-X DESIGN (BINDING; two independent reviews agreed on the core, the owner approved the FULL plan PR1-PR4 on 2026-10-13)

Evidence labels: OBSERVED = read in source by the reviewers (line numbers drift: re-grep every claim), INFERRED = consequence/proposal.

### 1. Problem

After an unclean shutdown the repository certifies [J+1, H] as a replication-journal GAP (`certify_cold_start`, `repo_instance.rs` ~1895-1906; J = persisted clean horizon, H = recovered gate floor = max(marker, max surviving WAL version)). J is written only by `record_clean_horizon` (`changefeed.rs` ~534-548; adapter `changelog_store.rs` ~64-87) and advanced in exactly three places: cold certification, `shutdown_clean`, seed install. It never advances at runtime, so the window is the WHOLE UPTIME. A follower stops on any gap >= its cursor (`follower_loop.rs` ~328-343; `gap_at = Some(max(from, first))` in `changefeed.rs` ~649-653), so nearly every follower must be reseeded after every unclean stop, even when the journal tail was fully persisted. Making the window narrower does not help followers; EXACTNESS does.

What is missing at recovery: for a version in (J, H], recovery cannot tell (1) a committed mutation whose journal event was lost, (2) an eventless/aborted allocation, (3) evidence already erased by WAL truncation / history GC. WAL entries lack the event header (actor, implicit/public tx_id, table names, projection timestamp: `wal_entry_v2.rs` ~142-160) and the WAL keeps only RecordId-keyed data ops (`commit.rs` ~536-560) while projection keeps every Set/Remove (`changefeed.rs` ~807-841).

### 2. Principle: the WAL is the source of truth for journal events (PostgreSQL slot / logical-decoding pattern)

- **I1 Derivable**: every journal event is a pure function of its WAL entry: `event = event_from_wal_entry(entry)`; ONE function used by the live commit path and by recovery (no second projection). Entries of the old format mean "unknown" -> conservative GAP fallback.
- **I2 Retained**: a WAL segment is never unlinked until the journal has durably checkpointed (flushed events + persisted gaps + persisted J) past every entry in it. Cheapest cadence: checkpoint J right before each truncation attempt (so the segment size is the cadence). If the checkpoint fails, do not truncate.
- **I3 Serve-durable**: followers are only served versions whose WAL entry is already fsynced: replication pulls are bounded by `min(W, S)`, S = the fsynced prefix. Without I3 exact repair would remove the accidental protection of the conservative GAP and followers could be ahead of the recovered H. OBSERVED today: the WAL append is `Buffered` (no fsync) outside tests (`commit.rs` ~430-440, ~1046-1068), the background fsync is every 250 ms (`repo_instance.rs` ~1069-1074); replication pulls are bounded by `min(confirmed, W)` (`repo_instance.rs` ~1634-1640): a COMPLETION bound, not a durability bound. INFERRED latent hole: after a power loss versions above the recovered H that followers already applied are re-allocated and followers skip them silently, with no GAP covering them.
- Rejected: obligation ledger + WAL bit + provenance + legacy scan (a second durable log that only detects, never repairs; under I1 the "requires event" bit is just `event_from_wal_entry(e).is_some()`); repair from history (keys are per record, no transaction grouping, GC deletes); writing the journal inside the WAL fsync (different files; an extra fsync on the commit path is forbidden); a runtime checkpoint alone (narrows the window but followers still stop).
- Events carry only NEW bytes for Put and nothing for Delete (`changefeed.rs` ~67-73), so old values are never needed for repair.
- Backward compatibility is NOT a constraint (alpha), but old WAL entries must be handled safely (unknown -> conservative).

### 3. Settings (owner decision: reliability/speed behaviour must be SETTINGS, not hard-coded)

Technically correct level = the REPOSITORY (WAL, journal, gate, drainer, J and the follower bookmark all exist once per repository; one WAL entry / one commit version can span several tables, `wal_entry_v2.rs` ~142-160). A per-table or per-store switch is impossible. There is no config path from the server into the engine today and no per-repo settings record: `ShamirDb::init(SystemStoreConfig)` is all the engine gets (`server/` launcher ~348), repos load inside `init`, `RepoConfig` is `{name, factory, tables}`. So: a NEW server config section `durability:` (server-level DEFAULTS), parsed and validated at boot like `transactions:` (`deny_unknown_fields`; `transactions_config.rs`, `config.rs` ~740), passed INTO `ShamirDb::init` (new policy argument), held as a plain field on each `RepoInstance`; a per-repo override can be added later without touching the hot path. The top-level `Config` does NOT reject unknown sections (`config.rs` ~74): the new section must still be strict inside.

Existing knobs for context: per-request `durability` (`buffered|synced|async_index`, `batch_request.rs` ~67-81; only string-compared at `batch_execute.rs` ~215/~820; an unknown value silently behaves as `buffered`; `synced` runs `synced_flush` + `sync_wal` after the commit); repo engine `in_memory|fjall|hybrid` chosen at CREATE REPO and persisted; `shamir-tunables` constants (WAL segment 8 MiB with env override `SHAMIR_WAL_SEGMENT_MAX_BYTES`, `MAX_UNDRAINED_VERSIONS` 10000, journal channel 4096); `transactions:` (VIS-2 keys). The doc `02-durability.md` says "~500 ms MemBuffer": stale, the real power-loss window is the 250 ms WAL fsync.

Keys:
- `durability.wal_commit: buffered | synced`, DEFAULT `buffered`. I3 is an INVARIANT (always on), not a setting; the setting only chooses how the durable prefix advances. With `buffered`: followers lag by at most one fsync; PR1 adds a coalesced `sync_now` when a pull is held back by S, so the lag is usually far below 250 ms with no commit cost. With `synced`: commits use `WalDurability::Synced` through the existing seam `commit_wal_durability(repo)` (`commit.rs` ~430-440; called from `commit.rs` ~1051 and `pre_commit.rs` ~2781), S == W. A request can only make things STRONGER (a request `buffered` cannot weaken server-level `synced`); a request `synced` must sample W before its `sync_wal` so its write becomes servable immediately; `async_index` unaffected.
- `durability.wal_retention_max_bytes`, DEFAULT 1 GiB (128 segments of 8 MiB); validated at repo open to be at least 2 x the effective segment size (after the env override).
- `durability.on_wal_retention_exceeded: refuse_writes | declare_gap`, DEFAULT `refuse_writes`. `refuse_writes` = new commits fail fast with a retryable typed error, checked by ONE atomic load at admission next to the existing reopen-required check (`commit.rs` ~650-666); it clears by itself when J catches up; it must NOT park committers (the soft brake never wedges writers, `acknowledgement.rs` ~270-374). `declare_gap` = durably record a real GAP and unlink the old WAL (followers reseed); if the journal writer is dead the gap cannot be recorded, so it falls back to refusing writes. Boot logs a WARN when `declare_gap` is configured (precedent `server_launcher` ~435-444). A per-request `durability` value does not bypass this.
- No other knob: checkpoint cadence = before each truncation (segment size); the I1 event header has NO toggle (it is part of correctness); the 250 ms fsync interval stays a constant.
- `in_memory` and `hybrid` repos: nothing survives a restart (repo identity is regenerated, Hybrid keeps `__history__`, `__tx__`, `__changelog__` in RAM only, `repo_types.rs` ~157-163); they get a Mem WAL sink without background fsync: S == W, `wal_commit` is ignored with one log line, the I2 truncation gate and cold reconciliation do not apply (no WAL to recover from); I1 still writes the event header (one code path). Followers of such a repo are protected only by the seed identity fence (`fence.rs` ~176-197), which runs only with a seed receipt: outside S5c, add one test.

### 4. Plan (each PR merges and passes CI on its own; NO benchmarks will be run: PR descriptions MUST state which steps touch the commit hot path)

- **PR1 I3 + `wal_commit` + policy plumbing** (hot path: one load in `commit_wal_durability`; with `synced`, an fsync per group window). `DurabilityConfig` (ALL THREE keys parsed/validated here; PR2 consumes the retention keys), `ShamirDb::init` policy argument, per-repo policy field, `synced_through` S in the WAL group commit (sample W BEFORE each fsync, store AFTER success), `read_changelog_for_replication` bounded by `min(W, S)`, coalesced `sync_now` + wake of held-back pulls, per-request `synced` samples W before `sync_wal`. S covers a version only after that version's own durability boundary completed (WAL fsync for local commits; the history flush for replicated batch applies, `apply_replicated.rs` ~689-696). Tests: a WAL sink that drops unsynced bytes on a simulated power loss, then no follower is ever above the recovered H; NEGATIVE CONTROL: remove the bound and the test must fail; config validation tests (unknown keys, bad enum, retention minimum). Docs: `07-operations.md`, `02-durability.md` (fix the stale ~500 ms), `REPLICATION.md`, all four `deploy/server*.example.ktav`.
- **PR2 I2 runtime J + retention policy** (hot path: one atomic load at admission). `settle_and_truncate` (`drainer.rs` ~1529-1541): `record_clean_horizon(ceiling)` BEFORE `truncate_below`; no truncation if the checkpoint fails; the retention limit and both overflow modes; rewrite the runtime-safety doc on `record_clean_horizon`; new typed retryable error code. The checkpoint samples settled W and calls `changefeed()` AFTER sampling (so the pending-gap recheck at ~1513 runs); persist a recovery floor at least as high as J before advancing J; never substitute the max successful journal put for a prefix. Risk: during checkpoint flushes the writer stops draining its 4096-slot channel; if it fills it marks real gaps: expose counters. Tests: crash before/after the checkpoint and the unlink; a failing checkpoint means no truncation; both overflow modes; a dead writer under `declare_gap` falls back to refusing writes. Docs: `07-operations.md`, `TRANSACTION_OUTCOMES.md` (new error), `KNOWN_LIMITATIONS.md`, ktav comments.
- **PR3 I1 derivable WAL entry** (hot path: every commit; larger WAL encoding, event built from the entry). New WAL entry FORMAT VERSION with the event header (actor, implicit flag / public tx_id, table names, projection timestamp), `wal_ops_from_tx` aligned with projection (the RecordId-only filter), ONE `event_from_wal_entry` used by the commit path and recovery. Old-format entries = unknown -> conservative. Tests: property test that the live event equals the event rebuilt from the WAL entry, incl. implicit, system and non-RecordId writes; torn frames; eventless commits.
- **PR4 cold reconciliation** (cold path only). For each surviving entry with v > J: put `event_from_wal_entry(entry)` if missing; delete journal events in (J, H] that have no WAL entry and no follower provenance (lost commits; I3 guarantees they were never served); flush; J := H; only then serve. Recovery REFUSES to open on any I/O or decode error. Certify a GAP only where evidence really is gone (old-format entries, a follower's in-flight batch). Tests: crash matrix phase4 / pre-publish / pre-put / pre-flush / pre_truncate / post_truncate / mid-checkpoint, each reopened twice; assert NO GAP whenever the WAL survived and no follower ever misses an event; NEGATIVE CONTROL: removing the retention gate must make tests fail. Docs: `09-seed-recovery.md` (reseed becomes rare), `07-operations.md` (the unclean-shutdown section), `KNOWN_LIMITATIONS.md`, `REPLICATION.md`.
- PR5 (optional): orphan replay emits `event_from_wal_entry` instead of marking a gap (`drainer.rs` ~1358); channel-full gaps repaired before truncation. PR6 (optional): follower (WAL-less) path: journal barrier before the batch flush `flush_all_history` (`apply_replicated.rs` ~689-699), J stored with the bookmark, conservative certification only for the in-flight batch range.

### 5. Key code facts (OBSERVED)

Commit order: WAL append (`Buffered`) -> event projected after WAL acceptance (`commit.rs` ~1083-1084; `pre_commit.rs` ~2781-2784) -> `publish_journaled` (`publish_journaled.rs` ~16-29: enqueue-or-gap then `guard.commit()`; enqueue is `try_send`, a full channel marks a gap, `changefeed.rs` ~381-399) -> the journal writer `store.put`s each event WITHOUT flushing (`changefeed.rs` ~1103-1128). Cold start: `wal.recover()`, replay of all surviving entries (`recovery.rs` ~335-389), then `certify_cold_start` (~458); factory construction awaits recovery. The drainer flushes history, then unlinks WAL segments without requiring journal coverage (`drainer.rs` ~1503-1541); sealed segments are selected by max version (`segment_set.rs` ~467-477); history GC removes old rows independently (`mvcc_gc.rs` ~106-117). A failed-publish (orphan) version gets a gap even though its WAL entry still holds the data (`drainer.rs` ~1358, `materialize.rs` ~255-273). Fjall: one `Database` per repo, `flush()` is `persist(SyncAll)` for ALL keyspaces (`storage_fjall.rs` ~219-245, ~516-525), so the history flush before truncation also persists journal events already `set`. Followers' apply has no WAL: durable floor reserved before history writes, history written before event preparation, then publish and flush (`apply_replicated.rs` ~457-600).


# Reviewer design answer: replication_serve policy (binding for PR8)

## Verdict

**Recommend (c): two safe policies—`durable` by default, and `completed_reseed` for minimum replication latency. Never expose unfenced `completed`.** I3 remains mandatory for same-history crash recovery; the fast policy replaces that guarantee with mandatory history invalidation and reseeding.

**The 250 ms is replication lag, not a 250 ms leader-write wait.** Buffered acknowledgements precede fsync; replication-triggered sync must remain off the acknowledgement path. Shared I/O contention can still indirectly affect writes.

**Evidence labels:** OBSERVED means code inspected, not runtime verified. INFERRED means consequences or proposed design.

## Recommended settings and invariants

Under server `durability:`, inherited through the planned per-repository `DurabilityOverride`:

| Setting | Values | Default |
|---|---|---|
| `wal_commit` | `buffered`, `synced` | `buffered` |
| `replication_serve` | `durable`, `completed_reseed` | `durable` |

**INFERRED recommendation:** typed enums; reject unknown values, unsupported backend combinations and clients lacking mandatory identity fencing. Overrides inherit explicitly. Persist recovery-relevant policy before activation; initially require stopped admission for policy changes.

### 1. Soundness and recovery

**OBSERVED:** Pull uses the journal reader (`crates/shamir-engine/src/repo/repo_instance.rs:418`), whose bound is confirmation capped by W (`crates/shamir-engine/src/repo/repo_instance.rs:1634-1640`). Production WAL commits select Buffered (`crates/shamir-engine/src/tx/commit.rs:430-439`).

**INFERRED failure example:** Leader recovers through 100; follower has durably applied old versions 101–105 and bookmarked 105. Those leader WAL/history writes disappear on power loss. If the leader reuses 101–105, the follower requests 106 and misses their replacements. Even without reuse, it retains old puts/deletes absent from recovered leader state. Followers at ≤100 are not necessarily divergent; followers containing the lost tail are. Exact journal repair cannot recover vanished evidence.

**`durable`:** serve `B = min(confirmed, W, R)`, where **R is a contiguous recoverability prefix**, not the maximum version in an fsynced batch. For normal WAL writes, R includes only entries covered by successful WAL sync, plus certified durable-history/checkpoint coverage after truncation. Out-of-order appends, aborted allocations and WAL-less writes require explicit settlement. Preserve sufficient event evidence for exact repair before truncating WAL.

**`completed_reseed`:** serve `min(confirmed, W)`, but:

1. Durably arm a recovery-required lifecycle record **before any speculative serving**.
2. On any restart lacking a valid clean certificate, recover privately, durably increment `history_epoch`, invalidate old replication continuity, then admit service.
3. Keep lineage stable; changing epoch expresses rewritten history. Failed bump/flush or overflow blocks admission.
4. Clean certification requires stopped admission, settled writes, durable data/WAL, journal and allocation evidence—not merely an empty queue.
5. Persist the armed obligation across configuration changes; switching to durable must not erase it.

**OBSERVED:** Epoch bump uses checked addition and flushed metadata (`crates/shamir-engine/src/meta/repo_identity.rs:46-50,99-109`). Existing clean certification is conservative horizon-based (`crates/shamir-engine/src/repo/repo_instance.rs:1890-1904`), **not** the required lifecycle certificate.

**INFERRED:** This is safe convergence, not lossless replication: speculative observations may be retracted, and an unclean restart conservatively costs reseeding even if bytes survived.

### 2. Fences, backends and request durability

**OBSERVED:** Current fences operate only when a seed receipt exists (`crates/shamir-server/src/replication/seed/fence.rs:158-171,180-195`); epoch mismatch is rejected (`crates/shamir-db/src/shamir_db/execute/replication_seed/control.rs:28-35`).

**INFERRED requirements:**

- Bind **every** subscription/bookmark to lineage+epoch, including unseeded followers. Fence Hello and every Pull before decoding/applying; bind identity and payload to one admitted incarnation.
- Epoch mismatch makes the destination `resync_required` and excludes it from ordinary reads and downstream serving until replacement seed installation. Disconnected followers cannot instantly know about rollback: advertise speculative consistency, not linearizability.
- `wal_commit=synced` normally makes R≥W; equality is unnecessary. A per-request synced guarantee may strengthen acknowledgements, never weaken repository policy or change replication mode.
- OBSERVED: request `synced` currently flushes stores then WAL (`crates/shamir-engine/src/query/batch/batch_execute.rs:215-243`). Refactor acknowledgement durability into typed transaction policy; keep visibility separate.
- Memory/Hybrid cannot claim power-loss durability. Hybrid history and transaction/journal stores are memory-backed (`crates/shamir-engine/src/repo/repo_types.rs:133-162`). Require explicit `completed_reseed` for publishing them; mint a new incarnation on every data-losing reopen, including clean restart.
- WAL-less follower apply needs durable-history coverage in R, not a fictitious fsynced WAL prefix. OBSERVED: history flush precedes durability marking and bookmark persistence (`crates/shamir-engine/src/tx/apply_replicated.rs:689-699`). Preserve that ordering regardless of serve policy; downstream journal reconstructability remains a separate obligation.

### 3. Sync scheduling

**Recommend no lag-budget knob initially.** When available events are blocked solely by R, immediately request a repository-level sync.

**OBSERVED:** `sync_now` currently directly calls sink sync (`crates/shamir-wal/src/wal_group_commit.rs:405-411`).

**INFERRED:** Unify pull, timer and synced-commit demands under one coordinator: one in-flight sync, captured coverage, monotonic publication and notifications. Coalesce targets; requests arriving after captured coverage join the next cycle. Cancellation must not cancel shared work. Serve existing durable pages immediately; wait only within pull deadlines. Failures never advance R. Many followers must not multiply fsyncs, although sustained writes can still saturate fsync throughput. One-fsync lag is typical, not an SLA.

## Risks, observability and proof

**INFERRED:** Expose effective mode/backend capability; W, R, confirmed and served bounds; pending versions/oldest age; sync demand/coalescing/error counters and duration; durability-blocked pull duration; epoch bumps and reseed-required subscriptions. Version distance is not milliseconds.

CI must simulate **loss of unsynced bytes**, not merely kill a process:

- Durable: withhold 101–105 until sync; recover every served version and resume without omission.
- Fast: serve/persist that tail, discard it, bump epoch; reject old bookmark and quarantine before apply; reseed converges.
- Negative control: disable bump/fence and demonstrate stale state or skipped replacements.
- Cover lifecycle crash boundaries, mode switches, legacy followers, out-of-order appends, truncation, sync failure/cancellation, many-followers coalescing, Hybrid restart and chained WAL-less apply.

## Changes to the plan

Replace “S = fsynced maximum” with contiguous **recoverability coverage R**; add mandatory universal fencing and crash-safe lifecycle state before offering fast mode. Keep I3 for same-epoch exact repair; never silently enable exact continuity in fast mode after an unclean restart.

**Files changed: none. Builds/tests run: none, as requested; CI verification remains outstanding.**

