# Brief — SEED-5 end-to-end crash recovery certification and runbook

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

- NEW directory `crates/shamir-server/tests/seed_recovery_e2e/` (`main.rs` + modules; harness for a real child leader process, scenarios, assertions); small additive public test seams in NEW files of `crates/shamir-server/src/` ONLY if a scenario cannot be driven through the CLI/wire/public API (justify each in your final message)
- `crates/shamir-server/Cargo.toml` ONLY for a `[[test]]` entry or dev-dependency the new target needs (no new heavy dependency without need)
- docs: NEW `docs/guide-docs/guide/09-seed-recovery.md` (the offline recovery runbook: when to use it, preconditions, step-by-step commands for export / verify / install / Resume, refusal codes and the operator action for each, failure-boundary table, permissions, DDL-not-replicated limitation, what it does NOT recover, sensitivity of the artifact, process-crash vs power-loss honesty), NEW `docs/guide-docs/architecture/SEED_FORMAT.md` (the artifact format specification as built by SEED-2/3: manifest, records, digests, identity block, versioning) and the smallest edits to `docs/guide-docs/guide/README.md` (index), `docs/guide-docs/guide/08-interconnect.md` (one link line), `docs/guide-docs/KNOWN_LIMITATIONS.md` and the replication roadmap/status wording wherever the R0/R1/R2 tiers are described (grep for `R2`): distinguish the delivered offline R2-lite from the not-built online R2 snapshot
- NOT in scope: export/installer/control-plane internals except bug fixes the new tests prove (a fix must be minimal, in the owning file, with its own regression test, and listed in your final message), online snapshots, automatic reseed, batch/visibility work

## Task

SEED-5 (task #1329): end-to-end certification of the offline follower seed and the operator runbook, step 5 of 5. SEED-1 (repository lineage id + history epoch, Hello/Pull identity, real gap interval), FOLLOWER-DURABILITY, SEED-2 (offline export: `crates/shamir-server/src/replication_seed/export/`), SEED-3 (isolated installer: `replication_seed/install/`, engine `repo/seed_install/`, db `shamir_db/seed/`, boot gate, `replication-seed-install` CLI) and SEED-4 (paused receipt-bound subscription, validated Resume with typed `SeedRefusal` codes, follower identity/schema fence in `crates/shamir-server/src/replication/seed/`, status fields, db control plane in `crates/shamir-db/src/shamir_db/execute/replication_seed/`) are MERGED on master, your base. Also merged: S5c-1/S5c-2 (unclean-shutdown certification: a repository reopened after a crash certifies a recovery gap `[J+1, H]` before it is served) and S5c-4 (`replication/tests/recovery_gap_consumer_tests.rs`: follower stops with `resync_required`). Read all of them first, and the existing harnesses: `crates/shamir-server/tests/common/mod.rs` (in-process live server), `repl_convergence_e2e.rs`, `repl_supervisor_boot.rs`, `backup_restore_e2e.rs`, `doctor_e2e.rs`; verify every claim against the code (line numbers drift). Sibling work (VIS-4b batch receipts, VIS-5a clients) runs concurrently in other worktrees and owns `crates/shamir-engine/src/query/batch/**`, `db_handler/handler.rs`, `transactions_config.rs`, `crates/shamir-client*`; do not touch them. Do NOT delegate to child agents and do not wait on schedules: finish the work yourself in this worktree. No git mutation, NO builds or tests of any kind (not even `cargo check`: only CI compiles and tests; earlier agents burned their whole time budget running cargo locally), imports at the top, one primary export per file, `mod.rs` re-exports only, tests under `tests/` dirs, builder-only queries (wire-format docs excepted).

COMPILE-READINESS (mandatory; the main cost driver of this program is one compile error per CI cycle): before finishing re-read every new/changed file as a compiler would: every `use` resolves (the engine `crate::query::admin` re-exports only a subset of `shamir_query_types::admin`: import `ReplDirection`/`ReplMode` from `shamir_query_types::admin` directly), visibility across CRATE boundaries (integration tests under `crates/shamir-server/tests/` see only the PUBLIC API of `shamir-server`, `shamir-db` and `shamir-engine`: if you need a private item, add a documented public seam in a NEW file instead), every struct literal has all fields (grep them all, including tests), real signatures of every call (grep, do not recall), clippy classics (`cloned_ref_to_slice_refs`, `field_reassign_with_default`, `type_complexity`, `needless_borrow`), `Send` across `.await`; `doc!` yields a builder `Doc` (use `.into()` for a `Value`) and cannot carry a list of maps through `FilterValue`; never encode a QueryValue with bincode/MetaEnvelope directly; tests that read a PENDING/unpublished state must read an explicit version; shutdown-budgeted `flush_all()` is slow on Windows runners (use `flush_seed_catalogue()` when only durability is needed); Windows byte-range locks are mandatory (read a locked file only after dropping the lock). Repository lints that fail CI (`-D warnings`): `clippy::absolute_paths` (never write a path like `crate::a::b::c::f()` inline: import it with `use`), `clippy::type_complexity` (give tuple-of-fn test tables a `type` alias), `dead_code` on Windows for `cfg(test)` helpers used only by `cfg(unix)` tests (keep such helpers under the same `cfg(unix)`), and `unused_must_use`. `shamir-query-types`, `shamir-db` and `shamir-server` unit tests have NO `serde_json` dependency: use `rmp_serde` and serde-derived mirror structs for old-wire fixtures (never add a dependency to fix a test; Cargo.lock is checked with `--locked`). Ordinary (non-seed) subscriptions, profiles and legacy tests must keep behaving exactly as before: any new check on a shared admin path must be a no-op for rows without a seed receipt and must tolerate an unresolved profile. The seed install/export path is Unix-only (directory fsync): gate every disk-bound scenario with `#[cfg(unix)]` (the Linux and macOS CI jobs run it; Windows compiles it away) and keep helper code `cfg(unix)`-gated too so no dead-code or unused-import warning appears on Windows.

LAYOUT RULE (CLAUDE.md, ratchet): at most 7 direct entries per folder, at most 1000 lines per source file; `crates/shamir-server/tests/` already holds more than 7 entries (grandfathered): put ALL new test code into ONE NEW integration-test directory `crates/shamir-server/tests/seed_recovery_e2e/` (cargo builds `tests/<dir>/main.rs` as one test target; `main.rs` + at most 6 further entries, use sub-folders for harness / scenarios / assertions) and do not add files to the flat `tests/` folder; new docs are NEW files; existing files get only the smallest edits.

GOAL: prove, with real process crashes in CI, that a follower that lost its leader history to a crash can be re-seeded offline and converge, and document the procedure honestly. Read the appended reviewer design (section '4. Operator surface and refusal policy', section '5. Why this respects gap semantics', and the PR 5 row of the PR table in section 6) as BINDING.

REQUIRED:
1. Harness: a REAL leader process (the `shamir-server` binary via `env!("CARGO_BIN_EXE_shamir-server")`, with a generated config, a replicator account and a publication; use the existing config/launch helpers as precedent) whose process is killed without a clean shutdown (`Child::kill` = SIGKILL on Unix). Drive writes/deletes through the public client or wire path with the typed builders (no raw JSON). Followers may be in-process (`ShamirDb` + `SubscriptionSupervisor` on a temp dir, as in `repl_convergence_e2e.rs`) or child processes, whichever is deterministic; seed export/install MUST go through the real CLI subcommands (`replication-seed-export`, `replication-seed-verify` if present, `replication-seed-install --subscription --upstream --publication --profile`). Choose free ports without races (bind port 0 / reuse the existing helper) and bound every wait with an explicit generous deadline and event-driven polling of observable state (status / row counts / Hello), never fixed sleeps; each scenario must stay well under the 180 s nextest kill margin (split scenarios into separate `#[tokio::test]` functions, multi-thread runtime where the harness needs it).
2. Scenario A (the main path, design PR-5 row): leader child writes rows (inserts, updates, deletes, more than one table) -> killed uncleanly -> restarted (reopen certifies the recovery gap) -> a fresh follower (bookmark 0) pulls and the subscription becomes `resync_required` with NO payload applied (assert follower tables empty and bookmark unchanged) -> leader stopped -> `replication-seed-export` -> artifact verified -> `replication-seed-install` into a NEW follower data dir creating the paused subscription -> follower boots, no loop runs, status shows installation complete and both version domains -> leader restarted -> Resume -> first pull is `V+1` -> subsequent puts/updates/deletes on the leader converge (compare full row sets per table). Assert the SOURCE gap certificate (`gap_first`/`gap_through` from Hello) is identical before and after the whole procedure.
3. Scenario B (second crash): after convergence the leader is killed again and certifies a new horizon above the follower bookmark -> the follower stops with `resync_required` again (no payload applied) -> a second offline seed into another new repository converges. Also assert the opposite branch from the design: a crash whose new horizon is at or below the follower bookmark does not stop the follower (only if it can be produced deterministically; otherwise state in your final message why it is covered by the existing unit tests instead).
4. Scenario C (mismatch refusals, each with its distinct typed code and NO state change - bookmark, rows, subscription row, receipt all unchanged): Resume against a leader whose repository was recreated (lineage mismatch), against a leader whose history epoch changed (restore), against a leader whose schema changed after the export (schema digest mismatch, DDL is not replicated), a wrong publication/scope, an incomplete install (interrupt the installer with the existing failure-injection seam and confirm the boot gate / Resume refuse), and an existing ordinary destination repository.
5. Scenario D (restart at every durable boundary): restart the follower process/home after (i) install, (ii) Resume, (iii) the first applied batch; pause/receipt/bookmark and convergence must survive each.
6. Honesty: the process-kill tests establish process-crash behaviour only, NOT power-loss guarantees; say so in the runbook and in the test module docs. Use the existing deterministic failure-injection seams (SEED-3 install boundaries) for ordering claims; do not claim more than the tests prove.
7. Docs per the scope: the runbook must be executable by an operator who has never seen the code (exact commands with placeholders, expected output, every refusal code with its meaning and next action, a failure-boundary table: what is on disk and what to do after a failure at each install boundary), the format spec must match the code (read the manifest/record structs), and the roadmap/known-limitation wording must state exactly what R2-lite does and does not do (no DDL replication, no online snapshot, whole-repository scope, Unix-only publication, artifact is sensitive data, no writes lost on the old leader are recovered).
8. Report in your final message: every bug the new tests exposed (with the minimal fix), any scenario you could not make deterministic and why, and the exact test names for CI.
Hot path: none (tests and documentation).

REVIEWER ANSWER (offline seed design, BINDING):

# Verdict: approve offline logical seeding as R2-lite

**A stopped-source seed into a new, isolated follower repository is the right first recovery path.** Do not add a leader-side gap bypass, and do not implement recovery by copying a leader directory and changing its bookmark.

Approval is conditional on three essentials:

1. **FOLLOWER-DURABILITY lands before seed activation.**
2. **The bookmark is bound to a durable source lineage, not merely database/repository names or the current wire epoch.**
3. **Incomplete installations cannot enter the serving or replication registries.**

There is no better general minimal alternative in the inspected implementation. An independently verified, matching snapshot plus bookmark would be equivalent to this design—not a shortcut around it.

**Evidence labels:** **OBSERVED** below means verified in source text, not executed. **DESIGN** denotes the recommended new behavior. No runtime, filesystem-state, build, or test verification was performed.

## 1. Verified foundation and necessary corrections

| Finding | Evidence |
|---|---|
| **OBSERVED:** cold open records a conservative gap `(J, H]`, even when journal events exist. | `crates/shamir-engine/src/repo/repo_instance.rs:1838–1852` |
| **OBSERVED:** gap inspection reports `Some(max(from_version, first))` whenever `from_version <= through`. Thus `gap_at` is **not the affected horizon**. | `crates/shamir-tx/src/changefeed.rs:642–648` |
| **OBSERVED:** the follower checks gaps before decoding/applying accompanying events; equality also stops it. | `crates/shamir-server/src/replication/follower_loop.rs:305–325` |
| **OBSERVED:** the supervisor persists `resync_required`; only active subscriptions are started. | `crates/shamir-server/src/replication/supervisor.rs:237–240`, `:307–334` |
| **OBSERVED:** `Resume` changes subscription state to `active`; it does not repair data, advance the bookmark, or remove the source certificate. | `crates/shamir-db/src/shamir_db/execute/admin_replication.rs:379–407` |
| **OBSERVED:** seeds must use `consistent_version()`, which returns the contiguous settled visibility floor—not eager `last_committed()`. | `crates/shamir-engine/src/repo/repo_instance.rs:914–932` |
| **OBSERVED:** upstream bookmarks and follower-local versions are deliberately different domains. | `crates/shamir-engine/src/meta/repl_bookmark.rs:10–18` |
| **OBSERVED:** replicated apply translates field IDs by name and rejects follower tables with secondary indexes. | `crates/shamir-engine/src/tx/apply_replicated.rs:293–328`, `:348–371` |

**Important correction:** do not accept `--minimum-version <gap_at>` as proof that a seed clears the certified interval. A pull from version 1 can return `gap_at = 1` while the affected horizon is much larger. Expose the complete interval, especially `through`.

The existing consumer regression test explicitly distinguishes its runtime teardown from power-loss simulation: `crates/shamir-server/src/replication/tests/recovery_gap_consumer_tests.rs:135–146`. It is useful precedent, not proof of the proposed process-crash recovery workflow.

## 2. Seed artifact

### Format and scope

**DESIGN:** use a **versioned directory artifact**, one complete user `(db, repo)` per artifact:

```text
manifest.mpack
schema.mpack
bindings-000000.mpack
tables/000000/rows-000000.mpack
tables/000000/rows-000001.mpack
...
```

Use generated numeric paths; table names remain metadata, not filesystem paths. Publish the manifest last, then publish the completed directory by same-filesystem rename.

A directory avoids a repository-sized allocation and permits bounded verification and import. Existing backup already streams hashing through a fixed buffer, but its manifest only contains format, timestamp, and file entries: `crates/shamir-server/src/backup.rs:47–70`, `:147–169`.

**Do not reuse the physical backup format as the seed image.** Physical backup includes the entire server data directory, including registries, audit material, and TLS files: `crates/shamir-server/src/backup.rs:3–11`. A seed must not clone credentials, server secrets, source WAL, source journal, source bookmarks, or source-local version markers.

### Contents

Include:

- Complete table inventory, including empty tables.
- Source table configuration and normalized follower configuration.
- Declarative schema, schema version, and a canonical schema digest.
- Every live record at `V`, preserving its **exact raw key/RecordId**.
- Record payload bytes plus complete referenced source-field-ID/name bindings.
- Per-table live-record counts, independently recomputed during export and import.
- Source index inventory, explicitly marked **excluded**, for operator visibility.
- Format/codec versions, feature requirements, counts, file sizes and hashes.

**DESIGN:** keep raw payloads plus bindings rather than converting everything through ordinary query values. Reuse the existing recursive field-ID translation mechanism; preserve value types and opaque raw keys. Reject missing or conflicting bindings.

Schema requires its own semantic conversion: schema paths contain numeric IDs as values, not just record-map keys. Export schema paths as names and recompile using the destination interner. The current parser explicitly resolves interned path IDs: `crates/shamir-db/src/shamir_db/shamir_db/schema_management.rs:45–58`.

**Exclude all secondary-index definitions and state from installed configuration:** regular, unique, sorted, and index2 descriptors/backends. The inspected apply path checks all those registries, not merely `enable_indexes`: `crates/shamir-engine/src/tx/apply_replicated.rs:293–328`. Set follower `enable_indexes = false`; the engine configuration itself currently contains only name and that flag: `crates/shamir-engine/src/table/table_config.rs:1–17`.

Recompute destination record counters and persist them; do not trust copied source counter metadata. A setter already exists at `crates/shamir-engine/src/table/record_counter.rs:103`.

R2-lite should reject system-repository seeds and unsupported schema dependencies rather than silently omit them. It is **not** full-server disaster recovery or a promotable cluster clone.

### Identity block

**OBSERVED:** existing candidates do not provide repository lineage:

- `repo_token()` is a deterministic name hash: `crates/shamir-engine/src/repo/repo_instance.rs:2160–2167`.
- `server_meta` contains security identity and key material, including rotating identity state—not a repository incarnation: `crates/shamir-server/src/server_meta.rs:3–14`, `:136–144`.
- Backup manifest has no source identity or version: `crates/shamir-server/src/backup.rs:160–166`.
- Wire replication advertises names, current version, journal floor, and leader epoch, but no lineage: `crates/shamir-query-types/src/wire/repl.rs:52–78`.
- Handler constructors initialize `leader_epoch` to 1: `crates/shamir-server/src/db_handler/handler.rs:219`, `:238`. That value alone is insufficient.

**DESIGN:** add durable per-repository replication identity:

```text
repo_lineage_id: random 128-bit identifier
history_epoch: durable u64
```

Generate lineage once for a new repository, or once during exclusive migration of a legacy repository. Retain it across ordinary restarts. Change lineage on repository recreation; advance history epoch before serving a restored/rewound history. Refuse overflow.

The artifact records:

- Source database and repository names.
- Lineage and history epoch.
- Settled source version `V`.
- Certified gap interval, if present.
- Canonical schema digest.
- Artifact identifier and manifest digest.

The destination has **its own** lineage. Persist the source identity separately with its upstream bookmark.

Extend Hello/Pull metadata so the follower checks identity **before applying any payload**. Seeded replication must fail closed against an older peer that cannot supply identity. Persist any existing leader-epoch fencing floor as well; it is not a substitute for lineage/history epoch.

### Obtaining consistency at `V`

**DESIGN:** make export genuinely offline. No new online maintenance admin operation is necessary for R2-lite.

1. Stop the source server and disable automatic restart for the export window.
2. Acquire an exclusive maintenance ownership lock covering server startup and offline tools. Hold it throughout export; no `--force`.
3. Open through a maintenance path that starts **no listeners, subscription supervisor, or write-producing jobs**.
4. Load table/schema metadata and complete normal cold certification and WAL recovery.
5. Instantiate all tables; quiesce commit/apply owners and drain recovery work.
6. Explicitly flush history, interner, table metadata/counters, and repository metadata; propagate every failure.
7. Obtain `V = consistent_version()`. Require settlement through the allocation high-water mark and `V >= certified_gap.through`.
8. Export a bounded, key-ordered MVCC scan at `V`; reject corrupt rows.
9. Confirm version and schema inventory remain unchanged before finalizing the artifact.

`shutdown_clean()` already requires callers to stop admission and checks settlement against the allocation high-water mark: `crates/shamir-engine/src/repo/repo_instance.rs:1856–1883`. Use that lifecycle discipline, not merely a momentary sample of `consistent_version()`.

A restore liveness probe is useful precedent, but a probe that opens and releases a lock is not ownership for the whole export: `crates/shamir-server/src/restore.rs:9–18`. Introduce a held maintenance lock rather than copying its `--force` escape.

### Integrity and bounds

**DESIGN:**

- SHA-256 and byte count per file; canonical manifest digest included in the installation receipt.
- Strict manifest inventory: reject unknown files, duplicate entries, unsafe paths, symlinks/reparse-point escapes, unsupported codecs, duplicate record keys, and invalid binding/schema references.
- Streaming hashing and row import; no whole-table `Vec` and no whole-repository synthetic replication event.
- Initial limits: 16 MiB maximum row frame, 64 MiB row chunks, 64 MiB aggregate manifest/schema metadata, and 1 TiB default artifact-byte budget. Explicit operator options may raise size budgets; checked arithmetic and frame caps remain mandatory.
- Paginate bindings and table scans; bound decoder depth/container sizes too.

SHA-256 establishes integrity relative to a trusted manifest, **not provenance**. Supply the manifest digest through a trusted operator channel; do not claim the artifact is authenticated merely because its internal hashes agree.

## 3. Installation, local versions, and activation

### Durable ordering

**DESIGN:** install only into a previously nonexistent repository, using an unpublished staging image:

1. Verify artifact and expected source identity before destination mutation.
2. Create staging state and durably record `Installing(artifact_digest)`.
3. Create normalized table/schema structures; intern/remap names.
4. Stream records into MVCC history at one destination baseline version.
5. Recompute counters; persist interner, schema, table inventory, counters, and history.
6. Persist destination-local recovery floors and source identity/schema receipt.
7. **Only after all image durability succeeds**, write upstream bookmark `V`, then explicitly flush its metadata store.
8. Write and flush `Complete(artifact_digest, source_identity, V, local_baseline)`.
9. Publish the repository by same-filesystem rename and durable catalogue registration.
10. Create the subscription **paused**, bound to this receipt. Never automatically activate it.

Physical write order needs schema/interner preparation before encoding rows. The essential durability order is:

```text
complete data + schema + interner + counters + local floor
    → durable upstream bookmark
    → durable Complete marker
    → registry publication
```

Current bookmark persistence does not itself perform a flush: `crates/shamir-engine/src/repo/repo_instance.rs:1189–1195`, `crates/shamir-engine/src/meta/recovery_marker.rs:62–75`. Consequently, calling `advance_replication_bookmark(V)` is not an installation durability protocol.

**FOLLOWER-DURABILITY dependency:** seed installation and subsequent event apply must use the same hard rule: history/interner durability precedes bookmark durability; a failure prevents bookmark advancement and successful activation. Current apply writes history, then marks the local version durable: `crates/shamir-engine/src/tx/apply_replicated.rs:462`, `:515–519`. Do not infer fsync from those names or comments.

Also do not inherit backup/restore’s best-effort directory-sync contract for a completion receipt. Its helper logs failures and is a non-Unix no-op: `crates/shamir-server/src/restore.rs:351–369`, `:380–405`. Seed publication needs an explicit platform durability contract, with failures surfaced or unsupported guarantees refused.

### Crash safety and idempotence

| Interruption | Required restart behavior |
|---|---|
| Before/during data, schema, bindings, counters, or local-floor persistence | Staging image is never registered; normal startup refuses to serve it. |
| After image flush, before bookmark flush | Still incomplete; no activation. |
| After bookmark flush, before `Complete` | Still incomplete, despite apparently usable bookmark. |
| After `Complete`, before publication | Exact-digest retry verifies the receipt and finishes publication without replaying rows. |
| After rename, before catalogue update | Discoverable completed image remains unpublished; exact retry finishes registration. |
| After publication, before subscription persistence | Reads may be enabled, but no automatic upstream loop. |
| After paused subscription persistence | Explicit validated `Resume` is required. |

Retrying an incomplete import starts a fresh staging attempt; do not implement row-level resume now. Preserve abandoned staging images for explicit offline cleanup.

An already completed installation with the **same artifact digest and identity** is an idempotent no-op. Any different seed, existing ordinary repository, existing bookmark, or conflicting subscription binding is refused.

Boot must check installation state **before normal registry insertion or serving**, analogous to current cold certification’s pre-registry placement: `crates/shamir-engine/src/repo/repo_instance.rs:1838`.

### Destination-local version floor and downstream journal

**DESIGN:** use **local baseline version 1**, regardless of upstream `V`.

- All seed rows are history entries at local version 1.
- Persist local committed/settled/durable floors at 1 and a fresh local transaction-ID floor.
- First subsequently applied upstream event allocates local version 2.
- Do not copy source transaction IDs or version counters.

Reserve version 0 as empty/bootstrap, consistent with `crates/shamir-engine/src/meta/repl_bookmark.rs:35–42`.

**The seed is state, not a replayable event stream.** Record a destination-local bootstrap gap `[1,1]`, then clean horizon 1, before completion. Do not pretend a pull from local version 1 can reconstruct seeded state.

Therefore:

- A fresh downstream follower must itself seed from this follower.
- A downstream seed at destination-local version `L` bookmarks `L`, then pulls `L+1`.
- Future ordinary apply continues emitting destination-local journal events, as it already does: `crates/shamir-engine/src/tx/apply_replicated.rs:398–401`, `:491–513`.
- Never attach old downstream followers from a replaced repository incarnation.

### Subscription and status

**DESIGN:** keep bookmark authority in repository metadata; bind subscription records to the installation receipt. Initially support exactly one upstream owner and one complete repository pull scope.

Add `paused` creation support—the current handler creates subscriptions active: `crates/shamir-db/src/shamir_db/execute/admin_replication.rs:259–269`.

On `Resume`, validate:

- Complete installation.
- Subscription/profile scope matches the receipt.
- Upstream lineage/history epoch matches.
- Source schema matches the supported normalized schema.
- Current source affected horizon does not exceed the bookmark.
- Source settled position has not regressed below the seed position.

Then activate. The loop already pulls bookmark plus one: `crates/shamir-server/src/replication/follower_loop.rs:236–246`. Replace unchecked addition with checked addition.

Report per repository:

```text
installation_state, artifact_digest
source_db, source_repo, source_lineage_id, source_history_epoch
seed_source_version, upstream_bookmark, next_pull_version
local_baseline_version, local_consistent_version
schema_digest, certified_gap_first, certified_gap_through
subscription_state, last_terminal_reason
```

Current `ReplicationStatus` reports only subscription name/state: `crates/shamir-db/src/shamir_db/execute/admin_replication.rs:435–467`.

**DDL limitation:** schema is not continuously replicated in this inspected design; the roadmap identifies that gap at `docs/dev-artifacts/roadmap/REPLICATION.md:146–150`. R2-lite must require source DDL to remain frozen for that replication relationship. Advertise/check a canonical schema digest before apply and stop on divergence; full transactional DDL replication remains deferred.

## 4. Operator surface and refusal policy

**DESIGN:** add these offline CLI subcommands alongside existing backup/restore/doctor:

```text
shamir-server --config leader.ktav replication-seed-export
    --db app --repo main --to seed-directory

shamir-server --config follower.ktav replication-seed-verify
    --from seed-directory --expect-manifest-sha256 DIGEST

shamir-server --config follower.ktav replication-seed-install
    --from seed-directory --expect-manifest-sha256 DIGEST
    --db app --repo main
    --subscription app-main --upstream ENDPOINT
    --publication PUBLICATION --profile PROFILE
```

Same source/destination names in R2-lite, matching current follower configuration: `crates/shamir-server/src/replication/follower_loop.rs:73–76`.

Export/install require stopped servers and held maintenance ownership. Verify does not open a repository. No `--force`, overwrite, or arbitrary bookmark-setting option.

CLI authorization is OS filesystem authority, **not a fabricated replicator login**. Protect artifacts as sensitive data. Online subscription changes/Resume retain root `Manage`; status retains root `List`: `crates/shamir-db/src/shamir_db/execute/admin_replication.rs:35–56`. Replication transport retains replicator/superuser plus repository access checks: `crates/shamir-server/src/db_handler/repl_handler.rs:44–59`, `:107–114`.

Refuse:

- Seed `V` below artifact or current source certified `through`.
- Wrong names, lineage/history epoch, schema, publication/profile scope.
- Existing destination data or any ordinary existing repository.
- Unsupported/non-MVCC destination configuration.
- Installed secondary indexes.
- Incomplete image, untrusted digest, integrity failure, unsupported format.
- Source version regression, overflow, competing upstream ownership.
- A new source gap beyond the seed bookmark.

Write an offline recovery runbook, seed-format specification, failure-boundary table, permissions/DDL-limit documentation, and update `REPLICATION.md` to distinguish R2-lite from online R2. Existing CLI integration points are `crates/shamir-server/src/main.rs:88–126`, `:163–191`; doctor provides an offline-open precedent at `crates/shamir-server/src/doctor.rs:1–8`.

## 5. Why this respects gap semantics

**DESIGN correctness argument:**

1. The artifact contains the recovered source’s complete live state at settled position `V`.
2. Every source allocation through `V` is materialized or aborted.
3. Require `V >= affected_through`.
4. The follower durably installs that state before durably recording upstream bookmark `V`.
5. Its first pull is `V+1`.
6. Therefore `from_version > affected_through`, and the old interval no longer triggers the gap branch at `crates/shamir-tx/src/changefeed.rs:645–647`.

The certificate remains intact for everyone else. No event or missing interval is silently excused.

This does **not** recover writes already lost from the source’s recoverable state, nor guarantee a gap-free future tail.

If the leader crashes again and certifies a new horizon `H₂`:

- Bookmark below `H₂`: stop with `resync_required`; repeat offline seed into another new repository.
- Bookmark at or above `H₂`: continue from bookmark plus one.
- History rewind/identity change: stop for identity mismatch, even if a numeric cursor would otherwise pass.

## 6. PR split and CI acceptance

All execution belongs in GitHub CI, through the repository’s centralized test entry point.

| Order | Scope and dependencies | Required CI acceptance |
|---|---|---|
| **1. Identity and source admission metadata** | Engine meta keys/repository identity; query-types wire replication metadata; server Hello/Pull, in-process and wire sources; restore history-epoch handling. | Stable identity across reopen; recreation changes lineage; restore invalidates old history epoch; seeded peers refuse missing identity; expose real `through`, not just `gap_at`; schema mismatch stops before apply. |
| **2. Offline export and format** | New narrowly scoped seed-format/export modules; `main.rs`; maintenance-open/ownership path; table/schema inventory and bounded MVCC export. Depends on PR1. | Empty tables; exact keys; nested translated bindings; schema paths; corrupt records refused; dropped/aborted versions; allocated-but-unsettled versions cannot become `V`; bounded streaming; active server refused; interrupted export never verifies as complete. |
| **3. Isolated installer and durability receipt** | Engine seed import/local floor/bootstrap gap; database boot/catalogue gates; server install command and durable publication. Depends on PR1–2 **and FOLLOWER-DURABILITY**. | Inject interruption/failure at every boundary above; reopen never serves partial image; history flush failure prevents bookmark/Complete; bookmark flush failure prevents Complete; exact retry idempotence; counters and bindings survive reopen; first post-seed local version is 2; downstream pull from 1 reports bootstrap gap. |
| **4. Paused subscription activation and status** | Admin/query-builder subscription creation state; admin Resume/status; supervisor binding validation; follower identity/schema fencing. Depends on PR3. | No loop before Resume; wrong identity/schema/scope refused; existing destination refused; bookmark overflow refused; restart preserves pause/receipt; status reports both version domains; recreating subscription cannot bypass identity. |
| **5. End-to-end certification and operator docs** | Server process-level CI tests, existing recovery-gap test suites, recovery guide and roadmap. Depends on PR4 and FOLLOWER-DURABILITY. | Real child leader killed without clean shutdown → reopen → fresh follower `resync_required` with no accompanying payload applied → stopped-source export → isolated install → Resume from `V+1` → subsequent puts/deletes converge. Repeat with second crash, mismatch refusals, and follower restart after each durable boundary. Assert source gap certificate is unchanged. |

CI process-kill tests establish process-crash behavior, not physical power-loss guarantees. Use deterministic flush/fault injection to verify ordering; do not overstate those tests.

## 7. Do not build now

- Online snapshots or an online maintenance admin operation.
- Automatic reseeding, snapshot transfer over replication transport, resumable chunk download.
- In-place replacement of an existing follower.
- Per-follower leader gap exemptions or certificate deletion.
- Arbitrary bookmark advancement.
- Physical leader-directory cloning as follower bootstrap.
- Secondary-index replication/rebuilding on followers.
- Full-server/system-store cloning, promotion, failover election, multi-master.
- Full DDL replication or historical MVCC export.
- A fabricated giant changelog event containing the entire seed.

**Files changed:** none. **Builds/tests run:** none. **Recommendation:** proceed with R2-lite, with lineage binding, pre-registry installation gating, and FOLLOWER-DURABILITY as release-blocking requirements.

----

EARLIER REVIEWER ANSWER (context):

## Verdict

**Prioritize B: a bounded, operator-driven recovery path built around a verified snapshot + matching upstream bookmark. Keep the conservative detector. Do not fund S5c-X now; defer automatic reseed until the manual recovery primitive is proven.**

The ordering should be **correctness prerequisites → offline R2-lite → automatic R2 when operational demand justifies it → exact detection only if false-positive recovery cost becomes material**. Keep replication explicitly experimental throughout.

The central product problem is not “we sometimes stop unnecessarily”; it is **“after stopping safely, we have no supported way to recover the existing lineage.”** Exact detection improves availability when the journal survived. It does not repair an actual missing tail, bootstrap a new follower past a persisted interval, or make follower bookmark durability safe.

### Evidence terminology

**OBSERVED** below means directly read in this checkout—not runtime verification. **INFERRED** means a consequence or risk derived from those mechanisms. No builds, tests, or CI checks were performed.

## 1. Why B is the right investment

**OBSERVED:** The gap remains effective whenever the requested version is at or below its horizon: `crates/shamir-tx/src/changefeed.rs:645-648`. The follower checks that gap before decoding or applying events: `crates/shamir-server/src/replication/follower_loop.rs:305-325`. `Resume` merely sets catalogue state to `active`: `crates/shamir-db/src/shamir_db/execute/admin_replication.rs:385-387`.

**INFERRED:** An installed, verified follower image containing the source state through version **V ≥ affected horizon**, with upstream bookmark **V**, can resume at **V+1** without deleting or bypassing the leader’s gap certificate.

**Do not implement a leader-side “ignore this interval for follower X” switch.** The current pull semantics are version-based, not a repair-attestation protocol. Exempting a follower with an older bookmark would allow an actually missing event to be silently omitted. If its matching image already covers the horizon, the exemption is unnecessary.

Choose **offline, stopped-source seeding** for the first supported recovery path. This deliberately exchanges downtime for a much smaller consistency surface:

- Stop source writes and DDL; finish recovery and clean quiescence.
- Export authoritative logical state, schema and field-name information at a verified settled version V.
- Install into a new, isolated follower repository.
- Persist the image and its source identity/epoch/bookmark before making it eligible for replication.
- Resume from V+1; retain the source’s gap certificate for all other consumers.

Existing backup is **stop-and-copy server backup**, not a replication seed: `crates/shamir-server/src/backup.rs:3-11,29-32`. A backup file checksum does not establish that a follower’s data matches an upstream bookmark.

## 2. Smallest next PR-sized steps, in order

These are proposed scopes, not changes made in this review.

### PR 1 — Establish the durable follower progress invariant

**Scopes:**  
`crates/shamir-engine/src/tx/apply_replicated.rs`;  
`crates/shamir-engine/src/repo/repo_instance.rs`;  
`crates/shamir-engine/src/meta/repl_bookmark.rs`;  
`crates/shamir-server/src/replication/{follower_loop,supervisor}.rs`; existing replication tests.

Require one replication owner per destination repository, and ensure **all event data reaches the explicit durability boundary before its bookmark can become durable**. Partial apply errors must not be treated as ordinary indefinitely retryable progress if the engine requires reopening.

CI acceptance: crash between data application, data flush, bookmark set and bookmark flush; duplicate subscriptions; multi-table partial failure. A durable bookmark must never authorize skipping absent data.

This is a prerequisite for both manual and automatic recovery—not exact journal detection.

### PR 2 — Define and export an offline seed artifact

**Scopes:** engine repository snapshot/export boundary; server backup/CLI surface; replication recovery documentation.

Artifact must bind **source lineage, epoch, database/repository, settled V, schema, interner representation, data and integrity checks**. Freeze writes and DDL for this first implementation; export logical records or provide verified interner translation, rather than assuming leader IDs are portable.

Use the settled position, not latest publication diagnostics: `crates/shamir-engine/src/repo/repo_instance.rs:914-918`.

CI acceptance: export refusal without quiescence; schema/interner mismatch; corrupt or incomplete artifact; seed V below the certified horizon.

### PR 3 — Install the seed with crash-safe readiness

**Scopes:** engine seed installation and metadata; server CLI/restore surface; supervisor readiness checks.

Install only into an isolated destination. Publish an installation-complete marker **after** data, metadata and matching upstream bookmark are durable. Incomplete installation remains unservable/unreplicable across restart. Do not expose a standalone “advance bookmark” repair command.

Initially support the existing follower envelope: matching schema, no secondary indexes, controlled local writes. Preserve leader-local and follower-local version domains separately.

CI acceptance: interruption at every installation boundary; restart refuses partial image; completed seed resumes V+1 against the original gapped leader without changing its certificate.

### PR 4 — Expose the operator workflow and prove the round trip

**Scopes:** `admin_replication.rs`, replication query types/builders if an admin interface is chosen, supervisor state handling, existing server recovery tests, `guide/07-operations.md` and `KNOWN_LIMITATIONS.md`.

Connect verified installation to an explicit recovery-complete/resume action. Report source identity, seed version and installation status—not merely catalogue `active`.

CI acceptance: real leader crash → `resync_required` → offline seed → resume → subsequent writes converge; unrelated unseeded followers still stop.

**Do not describe the whole seed feature as a one-PR metadata toggle.** The command is small; the consistency contract is not.

## 3. When the other options become right

| Option | Decision now | What would change that decision |
|---|---|---|
| **A — Exact detector** | Defer. The supplied design’s WAL change, obligation ledger, legacy reconciliation and readiness work buy primarily fewer uncertainty-driven stops—not recovery. The 4–5 PR estimate is prior-review context, not independently established here. | Manual recovery exists; CI establishes the recovered-floor and follower durability invariants; measured intact-journal crash incidents make unnecessary reseeding a substantial operational cost; startup/migration costs are acceptable. |
| **C — Automatic R2** | Build on B later, not before it. Automating an unproven install contract multiplies failure modes. | Replicas are actually operated unattended, recovery downtime matters, and snapshot/schema/interner/epoch consistency plus interrupted-install recovery are proven. The roadmap already places snapshot bootstrap, schema and retention in R2: `docs/dev-artifacts/roadmap/REPLICATION.md:624`. |
| **D — Spend nothing** | Defensible release positioning, but not my recommendation for this remediation program. A supported manual escape hatch is the best bounded investment. | The owner explicitly shelves replication and accepts rebuilding lineages for experiments. There is no production promise forcing R2: `README.md:69-74`; the roadmap says future features are non-binding: `docs/dev-artifacts/roadmap/ROADMAP.md:3`. |

## 4. Current correctness risks the owner should know

### High priority: follower bookmark can outrun durable follower data

**OBSERVED:** Apply and bookmark advancement are separate calls: `crates/shamir-server/src/replication/follower_loop.rs:352-362`. Bookmark persistence ultimately performs `Store::set`, without an explicit flush: `crates/shamir-engine/src/meta/repl_bookmark.rs:57-58` and `crates/shamir-engine/src/meta/recovery_marker.rs:62-75`. Apply marks the local version durable after history writes, but this path does not explicitly flush history before returning: `crates/shamir-engine/src/tx/apply_replicated.rs:515-541`.

**INFERRED:** If bookmark persistence survives while preceding data does not, the next pull starts too far ahead and silently misses source data. A leader gap detector cannot diagnose this follower-local loss. **Not reproduced or established on the on-disk backend in this review.**

### High priority: certification coverage depends on a possibly understated H

**OBSERVED:** Gate construction swallows recovery-marker read/decode errors and uses `max(marker, surviving WAL maximum)`, not retained history: `crates/shamir-engine/src/repo/repo_instance.rs:888-907`. Follower apply has no local WAL and treats recovery-marker persistence failure as nonfatal: `crates/shamir-engine/src/tx/apply_replicated.rs:68-71,521-533`.

**INFERRED:** Surviving follower-written history above that recovered floor can lie outside `[J+1,H]`; downstream replication may therefore have missing obligations not covered by certification. Version reuse is also a concern. This is expressly outside the documented policy’s coverage: `docs/guide-docs/KNOWN_LIMITATIONS.md:572-585`.

**Recommendation:** Treat trustworthy durable version-floor evidence as a separate correctness requirement. Do not equate preserving a conservative detector with proving complete crash-gap coverage.

### Open refusal now depends on changelog metadata and write availability

**OBSERVED:** Normal factory open calls recovery before returning: `crates/shamir-engine/src/repo/repo_instance.rs:311-327`; recovery invokes certification: `crates/shamir-engine/src/tx/recovery.rs:458`. Clean-horizon decode/read errors propagate: `crates/shamir-engine/src/repo/changelog_store.rs:26-37`. GAP/J writes require explicit flushes: `crates/shamir-engine/src/repo/changelog_store.rs:64-85`.

**INFERRED:** Corrupt J or unavailable certificate writes can prevent opening an otherwise recoverable data repository, even when replication is unused. This is deliberate fail-closed behavior, but it needs an offline diagnostic/recovery procedure—not instructions to delete metadata. Certification awaits have no explicit deadline: `crates/shamir-tx/src/changefeed.rs:499-510,535-547`; a stalled backend can also stall open.

### Do not use stale limitation text as the implementation oracle

**OBSERVED:** `KNOWN_LIMITATIONS.md:542-545,560-562` still says field-name translation is absent and unknown tables are skipped. Current code translates and persists interner bindings (`apply_replicated.rs:348-372`), rejects unknown tables (`:220-227`), and makes rejection terminal (`follower_loop.rs:389-393`).

Those two advertised silent-loss mechanisms are **not supported by the current code I read**. Update the documentation in the recovery work.

## Verification boundary

I inspected certification/checkpoint ordering, marker durability calls, bounded gap reads, follower application/bookmark flow, supervisor terminal-state handling, admin resume behavior, existing recovery-test source, backup semantics and release/roadmap positioning. I did **not** verify runtime durability, reproduce crashes, inspect CI results, or independently re-prove the entire exact-detector design.

**Files changed: none. Builds/tests run: none, as requested.**
