# 121 — S5c-X PR6b-3: retire the legacy follower apply path and v1/v2 bookmarks

## Context

PR6b-2c (#186) made receipt-based follower apply/resume active for
receipt-native repositories and kept repositories with a v1/v2 bookmark or
legacy floor evidence on the legacy apply path until a migration. The owner
decided (2026-10-16): the product is unreleased, backward compatibility may
be broken freely, and NO migration is wanted. The receipt protocol becomes
the ONLY follower apply path; old follower state is refused with a typed
error (operator reseeds).

The binding design is the reviewer verdict in the appendix. Implement it in
full as ONE PR, in the staged order of its section 6, including both
mandatory additions (seed installation writes v3; durable follower
provenance is the presence of a valid v3 record, separate from
`LastCommittedVersion` floor evidence, which leaders also use).

A parallel PR (PR8b-2, scoped quarantine) is changing the subscription
catalogue rows and quarantine cache in `shamir-db` / `shamir-server`. Keep
your edits in those two crates to what this design requires (identity
adoption, server supervisor/identity-fence tests) so the two merge cleanly.

## Rules

- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `rm`, or any git command that mutates the working tree or index. Only
  edit files; the orchestrator commits. File deletions of retired code are
  done by editing (delete the file's content through your edit tool and
  tell the orchestrator which files to remove, or remove them with your
  file tool — never with git).
- NO `cargo test` / `build` / `check` / `clippy` / `nextest` or any other
  compile or test command, by you or any child agent (repeat this rule in
  every child brief). Only `cargo fmt -p <crate>` for touched crates as the
  last step. CI is the gate; tests run only through `./scripts/test.sh` in
  CI. Clippy runs with `-D warnings` (`await_holding_lock`,
  `type_complexity`, `needless_borrow`, `clone_on_copy`, `dead_code`,
  unused imports — gate test-only imports with `#[cfg(test)]`). After the
  deletions, sweep for now-unused functions, imports, seams and re-exports
  in every crate.
- Never silence a lint with `allow`.
- Layout ratchet: at most 7 entries per folder, at most 1000 lines per file;
  files and folders already over a limit must not grow.
- Imports at the top of the file; `mod.rs` files only re-export; tests in
  `tests/` folders, one topic per file, every new test file wired into its
  `tests/mod.rs`, every deleted test file removed from it.
- No machine-specific absolute paths in any file you write.
- Futures on deep inline chains (nested Batch, ALTER SUBSCRIPTION RESUME,
  seed install, admin dispatch) must not grow: heap-pin new heavy awaits.
- Every `impl Trait` function returns one concrete type on every branch.
- Before finishing, one verifier child agent (same no-compile rule) reads in
  full every changed file plus direct callers in all crates (benches,
  examples, tests) and checks name resolution, privacy, imports, `cfg(test)`
  gating, signatures, `Send` across awaits, dead code left by the removals,
  and the clippy lints above.

## Final message

Per merge criterion and per numbered acceptance test of the design: what was
built or deleted, file:line, the test name; the list of deleted files; measured
line counts for every touched file above 900 lines.

## Appendix — binding design (reviewer verdict)

## Verdict — APPROVE the re-scope, as one atomic retirement PR, with two mandatory additions

**Receipt apply must become the only follower apply protocol. Refuse old follower state; do not migrate it.** Make receipt visibility an invariant of a **durably identified follower**, not an optional protocol mode.

Two changes are essential beyond deleting the selector:
1. **Convert seed installation to write v3.** Today the reseed path itself writes the legacy bookmark format.
2. **Separate follower identification from allocation-floor evidence.** `LastCommittedVersion` without v3 is not sufficient to reject an arbitrary repository at open: leaders use that marker too.

**Evidence status:** OBSERVED below means inspected source or test assertions, not executed behavior. Recommendations are design decisions/inferences. No builds, tests, or runtime verification were performed.

## 1. Concrete deletion and retention plan

### Delete the legacy execution path completely

| Remove | Evidence and boundary |
|---|---|
| `apply_replicated_batch_legacy`, `apply_batch_with_info`, their floor reservation and finalization machinery | `crates/shamir-engine/src/tx/apply_replicated.rs:685`, `:695`, `:768`. These implement the old allocate → reserve floor → publish/write history → flush → bookmark ordering. |
| The standalone legacy `apply_replicated` implementation and its `apply_prepared` executor | `crates/shamir-engine/src/tx/apply_replicated.rs:215`, `:442`. Leaving this public entry intact would leave a second WAL-less apply protocol available despite removing batch routing. Remove its repository wrapper or make that wrapper invoke the receipt batch path. |
| `BatchDurabilityGuard` and legacy-only `PreparedEvent.guard`/`version` fields | `crates/shamir-engine/src/tx/apply_replicated.rs:230`, `:647`. Receipt execution must retain its own guard ownership and lifecycle. |
| Legacy batch durability module | `crates/shamir-engine/src/tx/follower_durability/batch.rs:12`, `:26`. Both `persist_batch_record` and `finish_batch` belong to the retired ordering. |
| Activation selector and legacy-routing negative control | `crates/shamir-engine/src/tx/follower_durability/receipt/resume/activation.rs:18`, `:30`. Replace selection with validation/admission; there is no successful “choose legacy” result. |
| Legacy bookmark encoders, permissive decoding and downgrade guards | `crates/shamir-engine/src/meta/tests/follower_bookmark/v2.rs:28`, `:126`, `:145`, `:161`, `:182`, `:207`. Production code is currently loaded from this tests directory by `crates/shamir-engine/src/meta/repl/bookmark.rs:7`; do not mistake it for test-only code. |
| v3 loader’s legacy fallback and redundant raw-format sniff | `crates/shamir-engine/src/meta/repl/bookmark_v3.rs:96`, `:127`. Use one strict loader/classifier instead of `has_v3_record` followed by separate reads. |

**Also remove the free-standing bookmark-advance API rather than mechanically rewriting it to v3.** `RepoInstance::advance_replication_bookmark` lets callers advance acknowledgment separately from applying data (`crates/shamir-engine/src/repo/repo_instance.rs:1215`). Under the new contract, receipt checkpointing owns that advance. Seed completion remains a separate, explicitly authorized initialization operation.

### Retain the shared semantics

- **Preparation, validation, interner translation and downstream projection.** Receipt code calls `prepare_event` and `reproject_for_downstream`; these are not legacy-only (`crates/shamir-engine/src/tx/follower_durability/receipt/apply.rs:26`; `crates/shamir-engine/src/tx/follower_durability/receipt/resume/identity.rs:114`). Retain `ApplyOutcome`, rejection reasons and policy.
- **Fence identity and v3 projections.** Keep `FenceRecord` and the useful `load_replication_bookmark`/`load_fence_record`/`load_clean_horizon` interfaces, implemented as projections of strict v3 data—not through `Version2Record`.
- **Identity adoption, rewritten as v3-only.** Retain the field-preserving update in `save_fence_record_v3_aware`; delete its legacy branches (`crates/shamir-engine/src/meta/tests/follower_bookmark/v2.rs:234–279`). It must preserve bookmark, J and outstanding receipt reference.
- **`LastCommittedVersion`, `NextTxId` and allocation-floor restoration.** Gate initialization uses `max(marker, inflight WAL, checkpoint)` (`crates/shamir-engine/src/repo/repo_instance.rs:864–883`). Receipt staging also reads the marker (`crates/shamir-engine/src/tx/follower_durability/receipt/stage.rs:422`). Deleting legacy reservation helpers does not justify deleting recovery markers.
- **Leader/non-follower cold reconciliation.** Keep WAL classification, eventless handling, unsupported-evidence gaps, journal repair and recovery-floor reservation (`crates/shamir-engine/src/tx/cold_reconcile/algorithm.rs:98–180`). “Legacy WAL evidence” is not “legacy follower bookmark.”
- **Follower J and gap handling.** Keep the distinction between upstream bookmark and follower-local clean horizon, but read J from strict v3 (`crates/shamir-engine/src/tx/cold_reconcile/follower.rs:35–48`).
- **Receipt worker, reservation, uncertain-outcome reconciliation and cold receipt completion.** These are the surviving protocol, not transitional scaffolding.

### Journal barrier: retain the obligation, retire the old implementation

Do **not** remove the WAL-less journal durability boundary.

The old `single_barrier`/`durable_journal_barrier` functions serve the retired executor (`crates/shamir-engine/src/tx/follower_durability/barrier.rs:13–60`). They can disappear with that executor and its tests. The receipt implementation independently performs exact-byte read-back, explicit flush and FIFO confirmation; retain those operations (`crates/shamir-engine/src/tx/follower_durability/receipt/checkpoint.rs:198–239`), including cold repair through the sole writer (`crates/shamir-engine/src/tx/cold_reconcile/receipt_recovery.rs:99–117`).

## 2. Refusal semantics and fresh installation

**Decision: refuse at open whenever durable metadata identifies old follower state; additionally validate at first follower admission for previously unclassified repositories. First-apply-only refusal is insufficient.**

Recommended state table:

| Durable state | Required action |
|---|---|
| Supported, valid v3, including bookmark zero and no outstanding receipt | Open as a follower; recover any receipt before service. |
| Legacy fixed-width/u64 bookmark, v1 or v2, including bookmark zero | Typed unsupported-follower-format refusal at open. No rewrite, recovery apply or cursor advance. |
| No v3, but established follower provenance plus a nonzero allocation floor | Typed reseed-required refusal at open. |
| No bookmark and nonzero floor, without follower provenance | Do not globally reject a leader. Refuse an attempt to adopt/apply as a follower; require reseeding instead. |
| No bookmark, no floor, no conflicting installation state | Eligible for explicit fresh follower initialization; persist v3 before admitting apply. |
| Malformed, truncated or unknown-version bookmark | Fail closed with a typed format/corruption error; never treat as absent or initialize over it. |

The old selector already distinguishes absent bookmark plus nonzero floor from freshness (`crates/shamir-engine/src/tx/follower_durability/receipt/resume/activation.rs:47–58`). **That check is meaningful in follower activation, not as a universal repository-open rejection.**

Use a structured `DbError` variant carrying the unsupported format/evidence reason. Preserve a machine-readable terminal classification through server handling rather than reducing it solely to a retryable string; current identity startup converts storage errors to `ReplError::Bookmark(String)` (`crates/shamir-server/src/replication/identity_fence/checks.rs:28–36`).

**Operator action:** stop the affected subscription and use the existing seed/reseed workflow to install a fresh destination. No in-place conversion, floor clearing, bookmark deletion or force-continue option.

**Fresh-case answer:** no protocol ambiguity remains once fresh adoption is explicit. Absence of both records is eligibility, not proof that a repository is already a follower. Existing seed-install state must also be checked.

## 3. Visibility: an unconditional follower invariant, with durable provenance

**Keep the follower/non-follower distinction; eliminate the legacy/receipt distinction within followers.** An internal cached flag is acceptable, but it must be derived from durable role state—not enabled opportunistically by the first receipt.

OBSERVED:
- Warm processing enables visibility at pipeline entry (`crates/shamir-engine/src/tx/follower_durability/receipt/apply.rs:90–93`).
- Cold processing enables it when a raw v3 record exists (`crates/shamir-engine/src/tx/cold_reconcile/receipt_recovery.rs:42–48`).
- Cold reconciliation separately infers follower provenance from `bookmark > 0 || fence.is_some()` (`crates/shamir-engine/src/tx/cold_reconcile/algorithm.rs:91–93`).

**These definitions must converge.** A valid v3 record with bookmark zero and no fence must still identify a follower. Conversely, `LastCommittedVersion`, local `RepoIdentity`, WAL capability and current subscription activity must not identify the role.

**Smallest durable discriminator: the presence of a valid v3 bookmark record itself.**
- Create it durably during fresh follower adoption, including an empty/zero-event follower.
- Preserve it across receipt retirement, pause, subscription removal and restart.
- Load it and configure the gate before reads, materialization or recovery publication.
- Do not infer “leader again” from an absent active subscription. Promotion is outside this PR.

Keep follower-only gating of:
- cold MVCC history bounds;
- observation of materialized allocation versions;
- ordinary-read refusal after `requires_reopen`.

These gates currently occur at `crates/shamir-tx/src/mvcc_store/mvcc_history.rs:672`, `crates/shamir-tx/src/visibility_metrics/resume_access.rs:39`, and `crates/shamir-engine/src/table/table_manager_crud.rs:14–18`. Do not globally enable them on leaders.

**Important ordering requirement:** validation/provenance loading must precede cold-reconcile early exits. The current non-persistent-WAL branch exits before receipt inspection, and `h <= j` exits before the later provenance calculation (`crates/shamir-engine/src/tx/cold_reconcile/algorithm.rs:57–59`, `:82–93`).

### Mandatory seed-install conversion

OBSERVED: seed completion writes the old bookmark via `save_replication_bookmark`, after persisting floor 1 (`crates/shamir-engine/src/repo/seed_install/writer.rs:292–310`).

**Without converting this producer, the prescribed operator recovery path would manufacture repositories that the new version refuses.**

Seed completion must emit a v3 baseline with:
- upstream bookmark = seed source version;
- local clean horizon = the certified local baseline, currently 1;
- no outstanding apply receipt;
- fence information populated or subsequently adopted without losing that baseline.

Preserve the existing `Installing → Complete` publication safety: partial staging is never served (`crates/shamir-engine/src/repo/seed_install/writer.rs:125–136`, `:312–314`). This is new-format installation, not migration.

## 4. Test disposition and required new coverage

### Delete obsolete assertions and seams

- Legacy-routing expectations in `receipt/tests/activation_routing_tests.rs:45–62`; replace them with typed refusals.
- The forced-legacy negative control in `receipt/tests/receipt_apply_tests.rs:430–459`, and its `disable_receipt_lookup` seam.
- Legacy subprocess replay/convergence expectations in `receipt/tests/crash_subprocess_tests.rs:590–665`; retain the receipt crash matrix at `:563–588`.
- Legacy flush-order/crash-image assertions tied to `finish_batch` and v2 bookmark writes. The old fake-store matrix explicitly exercises those helpers (`crates/shamir-engine/src/tx/follower_durability/tests/crash_image_tests.rs:15–19`).
- Legacy encoder round trips, compatibility-success tests and refuse-v3-downgrade tests once no legacy writer remains.

Retain old encoded bytes only as **test fixtures for rejection**, not as production writers.

### Rewrite coverage rather than deleting the guarantees

- **Server partial-history fixture:** it deliberately seeds v2 and injects a per-table `Store::set` failure that receipt atomic batches bypass (`crates/shamir-server/src/replication/tests/supervisor_tests.rs:509–540`). Replace with:
  1. receipt I/O pause/retry convergence without extra allocations;
  2. terminal receipt corruption/ambiguity stopping the supervisor and refusing resume.
  
  Do not preserve the old expectation that ordinary history I/O failure necessarily causes reopen.
- **Identity adoption tests:** retain failed/cancelled flush and retry-before-contact coverage (`crates/shamir-server/src/replication/identity_fence/tests/adoption_durability_tests.rs:59–68`, `:98–119`). Use v3 fixtures and assert preservation of J, bookmark and receipt pointer.
- **Preflight, translation, counters and journal ordering:** port the single-event test families listed in `crates/shamir-engine/src/tx/tests/mod.rs:3–6`, `:48–49` onto the public receipt entry.
- **Gap/cold tests:** port meaningful J, retained-gap and repeated-reopen invariants; remove expectations of successful legacy replay. Existing legacy callers occur in `crates/shamir-engine/src/tx/follower_durability/tests/gap_tests.rs:49` and `crates/shamir-engine/src/tx/cold_reconcile/tests/follower_cold_tests.rs:118`.

Prune only legacy phase members from the shared seam; retain receipt phases and failure/panic controls (`crates/shamir-engine/src/tx/follower_durability/test_seam.rs:8–41`).

### New acceptance tests

1. u64/v1/v2 records, including zero bookmarks, refuse open with the exact typed error.
2. Legacy floor-without-bookmark fixture refuses follower adoption/apply; repeated attempts do not allocate, write history, change counters or mint v3.
3. A leader with the same recovery marker still opens and preserves its allocation floor.
4. Fresh, zero-event follower initialization writes durable v3; restart retains follower visibility even with no fence and no receipt.
5. Refusal also holds across cold-open early-return branches.
6. Seed install → reopen → identity adoption → receipt tail succeeds, with upstream cursor and local J kept distinct.
7. Corrupt/unknown records are never overwritten as “fresh.”
8. Unsupported atomic-batch backends refuse before allocation and first-touch mutation.
9. Prepared/Materialized receipt recovery, cancellation, uncertain checkpoints, exact journal durability and materialize-before-publish invisibility remain covered.
10. Legacy crash-image fixture now refuses repeatedly instead of replaying. No need to retain a runnable legacy executor to produce it.

## 5. Documentation changes

- **`docs/guide-docs/follower-journal-durability.md`:** replace the contradictory historical activation narrative with the current receipt-only contract. Specifically remove legacy acceptance and migration/rollback promises at `:3–12`, `:23–25`, `:65`, `:77`. Document strict refusal, durable follower provenance, v3 seed baselines, operator reseed and backend requirements. Update test references when old matrices are removed.
- **`docs/guide-docs/KNOWN_LIMITATIONS.md`:** replace the legacy-until-migration statements at `:659–660` and `:676–677`. State that unsupported follower formats are refused, no migration tooling is provided, and reseeding is required. Preserve receipt residual limits and leader/non-follower behavior.

Do not call successful subprocess termination/reopen tests power-loss proof; the current durability guide explicitly distinguishes these models (`docs/guide-docs/follower-journal-durability.md:7–10`).

## 6. PR shape, acceptance criteria and non-goals

**Recommendation: one PR, internally staged in dependency order.** The format consumers, identity adoption and seed producer must switch together. A deletion-only PR followed by a producer fix is not safe.

Suggested implementation sequence:
1. Add typed classification/refusal and strict-v3 reader tests.
2. Convert fresh adoption and seed installation; establish durable follower provenance before service.
3. Route every follower API through receipts; remove independent bookmark advancement.
4. Delete legacy executor/codecs/seams; port remaining meaningful tests.
5. Update documentation and validate through **GitHub CI only**.

**Merge criteria:** no production legacy apply or legacy bookmark writer; no successful legacy-read fallback; no rejected state changed into v3; reseeding produces supported repositories; role survives restart; leaders retain their recovery behavior; receipt durability/visibility guarantees remain covered; required CI gates pass.

**Non-goals:** migration, rollback/downgrade tooling, force-open repair, automatic reseeding, follower promotion, index/vector replication support, or unrelated WAL-format retirement.

**Files changed:** none. **Builds/tests run:** none, as requested. **Notable:** converting the seed writer and unifying durable follower provenance are mandatory parts of this retirement, not optional follow-ups.