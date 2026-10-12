# Brief — SEED-2 offline seed export and artifact format

You are a sub-agent working inside an isolated git worktree (your cwd) of the S.H.A.M.I.R. Rust database repository. Read `CLAUDE.md` in the cwd first: code ideology (lock-free / async / scc maps, no guard across `.await`), TDD, test layout (`tests/` dir per module, `mod.rs` = manifest only), imports at the top of the file, one primary export per file, query builder only (never hand-assembled JSON).

## Hard rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.
- Do NOT run `cargo`, `rustc`, `nextest`, or any build or test command: the owner's machine must stay idle and ALL compilation and testing happens later in GitHub CI. Therefore be extra careful that the code compiles: read the real definitions (Read/Grep) of every type, trait and function you call instead of guessing signatures; keep the change small and local; prefer the narrowest layer that owns the bug.
- Stay strictly inside the FILE SCOPE below. Other agents work concurrently on other files of this repository; do not touch any file outside your scope even if it looks related (if you believe an out-of-scope change is required, say so in your final message instead of doing it).
- TDD: add the regression tests described in the task in the same change. They must FAIL on the current code for the stated reason, be deterministic (no wall clock, no sleeps; use barriers, notifies, hooks or paused tokio time), assert observable behaviour (not just `is_ok()`), and live in the module's `tests/` directory wired through its `mod.rs` manifest.
- Write rustfmt-clean code (4 spaces, trailing commas, short lines); the orchestrator formats touched files.
- Do not add dependencies, do not bump versions, do not rename or reformat unrelated code, do not touch unrelated comments.
- Finish with a final message listing: files changed, what each test proves and why it fails today, any design deviation from the task text, and anything you could not verify.


## Scope discipline

If a file outside the FILE SCOPE is strictly required to make the change compile or to keep an existing test meaningful (a caller of a changed signature, a fixture, a manifest line), edit it minimally and LIST it in your final message; do not stop to ask. Ask (and stop) only for genuine design decisions or when the scope would grow into another task area.
## FILE SCOPE (only these paths may be edited)

- NEW module directory `crates/shamir-server/src/replication_seed/` (one primary type per file, `mod.rs` re-exports only): artifact manifest and file records, hashing/bounded writer, exporter, verifier, error type, schema-as-names conversion
- `crates/shamir-server/src/lib.rs` (one `pub mod replication_seed;` line) and `crates/shamir-server/src/main.rs` (ONLY the two new subcommands `replication-seed-export` and `replication-seed-verify`)
- NEW `crates/shamir-server/src/maintenance_lock.rs`: an exclusive lock on the data directory HELD for the whole export and shared with the server's existing single-instance guard (read how the guard is acquired today, `fs4` on `data_dir`), so a running server blocks an export and a running export blocks server start; no `--force`. Touch the server boot path ONLY as far as strictly needed to share that lock helper.
- NEW maintenance-open path file(s) under `crates/shamir-server/src/`: open the data directory WITHOUT listeners, subscription supervisor, replication supervisor or write-producing jobs, complete normal cold certification and WAL recovery, reuse the existing `ShamirDb` open internals (do not edit `server_launcher.rs` beyond what the shared lock helper needs)
- NEW read-only inventory/bounded-export helper files under `crates/shamir-engine/src/repo/` or `crates/shamir-db/src/shamir_db/` (one-line `mod` declarations only in the existing manifests): table/schema inventory, key-ordered bounded MVCC scan at a version, flush of history/interner/counters; NO changes to commit, apply, gate, changefeed or follower code
- tests: new files under the matching `tests/` dirs (+ one-line `pub mod`)
- NOT in scope: the installer (SEED-3), follower identity enforcement and paused subscriptions (SEED-4), wire/protocol changes, docs and runbook (SEED-5 owns them)

## Task

SEED-2 (task #1326): offline seed export and artifact format, step 2 of the offline follower seed (R2-lite). The remediation series is MERGED on master, your base (SEED-1 repository identity/epoch and the wire gap interval are in). Verify every claim you act on against the code first (line numbers may have drifted). Sibling work runs concurrently: FOLLOWER-DURABILITY (PR in CI) owns `apply_replicated.rs`, `follower_loop.rs`, `supervisor.rs`, `admin_replication.rs`, the recovery-marker/bookmark helpers and the gate-seeding region of `repo_instance.rs`; VIS-2 will own query-types wire/config, `db_handler` and commit paths: stay strictly inside your scope. Tests that park one commit and then await another INLINE hang (commit and non-tx writes wait for the completion watermark W): spawn the second operation, never raise a timeout. Do NOT delegate to child agents and do not wait on schedules: finish the work yourself in this worktree. No git mutation, no builds/tests (CI only), imports at the top, one primary export per file, `mod.rs` re-exports only, tests under `tests/` dirs, builder-only queries in code (wire-format docs excepted).

GOAL: a stopped source repository can be exported into a versioned, hash-verified directory artifact that SEED-3 will install into a follower. Read the appended reviewer design (sections 'Artifact', 'Contents', 'Identity block', 'Obtaining consistency at V', 'Integrity and bounds', the CLI sketch and the PR 2 row of the PR table) as BINDING; the points below are the acceptance criteria.

REQUIRED:
1. Artifact: a directory with generated numeric file paths (table names are metadata, never filesystem paths), `manifest` published LAST, the completed directory published by a same-filesystem rename, plus an explicit Complete marker. Files: schema (paths exported as NAMES, not interned ids), bindings (complete source field-id/name bindings referenced by the rows), per-table row files (exact raw key/RecordId + payload bytes, key-ordered, bounded chunks). The manifest carries: format and codec versions, feature requirements, source db/repo names, source lineage id and history epoch (from SEED-1), settled version V, the certified gap interval if any, canonical schema digest, per-table live-record counts, per-file size and SHA-256, an artifact id, and the source index inventory explicitly marked EXCLUDED. A canonical manifest digest is computable and printed.
2. Contents policy: full table inventory including empty tables; every live record at V with the exact raw key; reject corrupt rows; reject missing or conflicting bindings (reuse the existing recursive field-id translation, do not invent a second one); reject system-repository seeds and unsupported schema dependencies instead of silently omitting them. Never copy credentials, server secrets, source WAL, source journal, source bookmarks or local version markers. Do NOT reuse the physical backup format.
3. Consistency at V (offline only, no new online admin op): hold the maintenance lock for the whole export; open through the maintenance path (no listeners/supervisors/jobs); complete cold certification and WAL recovery; instantiate all tables; quiesce commit/apply owners and drain recovery work; flush history, interner, table metadata/counters and repository metadata and PROPAGATE every failure; take V = consistent_version() and require settlement through the allocation high-water mark AND V >= certified gap `through`; run a bounded key-ordered MVCC scan at V; confirm the version and schema inventory are unchanged before finalizing. Allocated-but-unsettled versions must make the export refuse, never become V. An interrupted export must never verify as complete (no manifest/Complete marker, partial directory is rejected).
4. CLI: `replication-seed-export --db <name> --repo <name> --out <dir>` and `replication-seed-verify <dir>`. Verify NEVER opens a repository: it recomputes sizes and hashes, checks the manifest digest, the Complete marker and per-table counts from the files; it fails closed on any mismatch, missing or extra file. Export refuses to write into an existing path and has no overwrite or force option.
5. Bounded memory: stream rows through fixed-size buffers; never materialise the repository.
6. Tests (deterministic, no sleeps): empty tables; exact raw keys round-trip through verify; nested translated bindings; schema paths exported as names; corrupt record refused; deleted/aborted versions not exported; an allocated-but-unsettled version refuses export; V below the certified gap through refuses export; running server (or a held lock) refuses export and a running export refuses server start; interrupted export (kill the writer before the manifest) never verifies; verify detects a flipped byte, a missing file and an extra file; manifest digest is stable for identical input; bounded chunking with a tiny chunk size. Reuse the in-process fixtures; do not spawn real servers in unit tests.
Hot path: none (offline tool).

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
