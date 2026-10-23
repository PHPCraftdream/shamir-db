# 131g - fixes after the independent final review of the PR8b-3 design (DEC-3)

Owner standard: we strive for perfection; for that we are ready to do MORE work and to BREAK backward compatibility (product unreleased, no migrations).

## Situation

Rounds 131e/131f produced the amended documents (HEAD of this branch). An independent read-only reviewer then checked the FINAL text against DEC-3 Q3 (`docs/dev-artifacts/design/dec3-owner-rulings-2026-10-21.md`), the baseline commit eeee8e98 (`git show eeee8e98:<path>` is read-only and allowed) and the source. Its verdict is NOT READY: no content was lost, but four P1 design gaps, residual contradictions and wrong citations remain. I will not let implementation PRs interpret these independently: the documents must settle them.

## Task

Same five documents under `docs/dev-artifacts/prompts/remediation/` (`131-...`, `131a-...`, `131b-...`, `131c-...`, `131d-...`), documents only, no other file. Governing rules, unchanged from 131f: amendments are SURGICAL edits (never a rewrite or compression; every schema/table/field list/state row/error code/CI case of the current text stays unless a finding below or DEC-3 removes it); every citation you keep or write is re-read against the current checkout; at most 1000 lines per file.

Fix EVERY item of the review below, deciding each design point in the direction of the most correct, most beautiful end state (not the cheapest), consistent with DEC-3:
- P1-1: define the single serialized audit-sink writer, who owns an ambiguous or timed-out in-flight append and how it is reconciled BEFORE any further append is submitted, the "at most one duplicate per ambiguous append" bound, the concrete record framing (field order, widths, which bytes the checksum covers, maximum record size, truncation of an incomplete or bad-checksum final record at open), and the fault-injection CI cases (put them into the CI case list of 131a; keep the list numbering coherent).
- P1-2: rewrite R1 steps 4-6 and the crash/recovery table so that every multi-record transition is ONE system-store transaction plus ONE flush per DEC-3 item 3: choose explicit R1 transactions (a durable `Pending` intent may remain, but the transaction that deletes the subscription must atomically record the corresponding phase transition) and give the recovery action for every resulting durable prefix.
- P1-3: delete the "remove/rename the squatter then release" remedy everywhere (131 and 131b at the places the review names); a blocked removal completes only through verified destruction or a durable identity-bound protection handoff (R2' target or incident record).
- P1-4: make the `SubscriptionRemoval` record carry what the tombstone needs (the frozen `storage_object_id`s, or immutable references to the completed destruction evidence; define the no-current-object case); keep binding-keyed retry/status semantics after compaction; update the schema in 131b and the prose in 131.
- P2-1..P2-4 and the "minimal edits" list at the end of the review.

Re-verification before you report (mandatory): after the edits, re-read every citation the review lists as wrong plus at least 25 other citations you did not touch, and grep the five files for the stale paths `shamir_db/system_store.rs`, `execute/admin_replication.rs`, `execute/admin_db_repo.rs`, `execute/admin_migration.rs`, `execute/helpers.rs` and for old line numbers of the follower and core files named by the review. `python -I scripts/ci/check_layout.py` once at the end.

## Review findings (verbatim)

exit_reason= end_turn verdict= None dur_ms= 470302
--- FINAL
# NOT READY

The core DEC-3 corrections are present, but the documents are **not yet a consistent, implementable normative baseline**. The remaining blockers are incomplete audit-log requirements, contradictory R1 durability rules, and a surviving protection-release escape. Citation refresh is also materially incomplete.

**Evidence labels:** **OBSERVED** below means text or source read in this checkout. **INFERRED** means the implementation consequence of that text—not a reproduced runtime failure.

Paths abbreviated below:
- **M:** `docs/dev-artifacts/prompts/remediation/131-s5cx-destination-lifecycle.md`
- **E:** `docs/dev-artifacts/prompts/remediation/131a-s5cx-destination-lifecycle-evidence.md`
- **S:** `docs/dev-artifacts/prompts/remediation/131b-s5cx-destination-lifecycle-surface.md`

## 1. Findings, ranked

### P1-1 — A9/Q8 is incomplete: the audit sink still lacks the required serialization and duplicate bound

**OBSERVED:** DEC-3 expressly requires **exactly one serialized writer** and **at most one duplicate per ambiguous append** (`docs/dev-artifacts/design/dec3-owner-rulings-2026-10-21.md:175`). Neither requirement is carried into the normative audit contract. S:321 adds length/checksum framing, final-record truncation and digest-conflict poisoning, but omits those two requirements. E:15 likewise omits their CI assertions.

M:213 instead permits fresh same-event appends after ambiguous results and restart; M:208 introduces bounded-timeout `spawn_blocking` execution without specifying how a timed-out, still-running append is reconciled before another append is submitted.

**INFERRED:** Independent executors, recovery passes or timed-out blocking calls can produce more duplicates than DEC-3 permits unless serialization and retry ownership are explicitly defined.

**Required edit:** Add the single sink-writer contract, ownership of an ambiguous/in-flight append, the duplicate bound, and corresponding fault-injection CI assertions. Define the actual record framing—not merely “length + checksum”—including lengths, checksum coverage and maximum record size.

### P1-2 — A7/Q3 is contradicted by the detailed R1 protocol

**OBSERVED:** M:186 requires multi-record lifecycle transitions to use **one system-store transaction plus one flush**, explicitly naming “R1 record write + row delete.”

The detailed protocol still specifies:
- persist and flush `Pending`, then delete the subscription: M:268–288;
- delete + flush, then set `RowDeleted`: M:289;
- crash states containing durable `Pending` with row existence unknown: M:298–299.

The new recovery table at M:188–196 does not resolve these R1-specific transaction boundaries.

**INFERRED:** An implementer cannot satisfy both the new atomic-transition requirement and the retained commit-before-flush/delete-then-phase-update instructions literally.

**Required edit:** Choose explicit R1 transactions and rewrite steps 4–6 and the crash table accordingly. A separately durable `Pending` intent can remain, but the transaction that deletes the subscription must atomically record its corresponding phase transition. Specify recovery for every resulting durable prefix.

### P1-3 — A2 still has a “rename the squatter, then release” escape

**OBSERVED:** M:304 allows a blocked `RowDeleted` removal to complete after the operator **removes/renames the squatter**. The same paragraph subsequently says found data must become an R2′ target or incident, “never a release.”

DEC-3 requires found data to become an R2′ target or incident (`docs/dev-artifacts/design/dec3-owner-rulings-2026-10-21.md:174`). Renaming the new incarnation away from the frozen name proves neither destruction nor durable protection transfer.

**INFERRED:** Following the first alternative literally allows completion and reservation release while the discovered data survives elsewhere without the required incident/target handoff.

**Required edit:** Remove rename/removal as an independently sufficient remedy. Require verified destruction or a durable identity-bound protection handoff before `Completed`; make the same rule explicit in S:255 and S:334.

### P1-4 — A2 tombstones cannot be constructed from the SubscriptionRemoval schema as written

**OBSERVED:** Archival requires a tombstone containing **frozen storage-object IDs**: M:214, E:49, S:334. But `SubscriptionRemoval.destinations` contains only `{db, repo}`: M:280 and S:255. Its record has no frozen IDs or durable references from which they must be recovered.

**INFERRED:** A completed removal cannot reliably produce the required tombstone after names are reused; resolving names at archival time would not recover frozen identities.

**Required edit:** Persist the IDs—or immutable references to the completed destruction evidence—in the removal record. Define how the no-current-object case is represented. Preserve binding-keyed retry/status semantics after compaction.

### P2-1 — Ownership minting still has a contradictory writer definition

**OBSERVED:** M:70 correctly limits minting to controlled creation and forbids lazy minting at open. M:75 still names the **“repository open/admission path (mint)”** as a writer.

The mirror-authorizes-deletion exception itself is now correctly rejected at M:72; I found no surviving normative mirror exception.

**Required edit:** Change M:75 to distinguish creation-time minting from open-time validation. Opening existing storage must never mint ownership.

### P2-2 — State-machine summaries still claim facts that their recovery rules explicitly allow to be false

**OBSERVED:**
- `Prepared` means the barrier is durably appended in M:138 and M:251, but barrier failure deliberately leaves `Prepared` without that proof: M:48, M:193, M:236.
- Boot says spawn/continue executors for `Blocked`: M:325; the state machine says `Blocked` stays idle until an administrative retry: M:256.
- S:294 still describes a **retired Blocked** operation and its retirement-gating event, despite A2 restricting retirement to successful completion.

**Required edit:** Define `Prepared` as durable authorization with barrier status separately established; make boot’s `Blocked` behavior explicitly replay-only/parked; delete the obsolete Blocked-retirement language.

### P2-3 — The promised complete error surface omits the new storage-format refusal

**OBSERVED:** M:73 introduces typed `storage_format_unsupported`, but S:70–88 does not define its variant, mapping or remedy. M:23 also says “eleven typed error codes **plus** `destruction_target_changed`,” while the eleven-row table already includes that code.

**Required edit:** Document `StorageFormatUnsupported` and its opening/installer/admin exposure, keeping its scope distinct from the eleven lifecycle DDL codes if that is intended. Correct the count in M:23.

### P2-4 — A13’s “verified current-checkout inventory” is false in several material places

**OBSERVED:** E:92–103 claims current, exact citations and defines insufficient path shorthand. Multiple cited files have since moved or split. The errors below are not merely historical notes.

**Required edit:** Refresh the inventory and all normative references to those locations; do not rely on an inventory-precedence sentence to repair nonexistent paths.

## 2. DEC-3 compliance matrix

### Amendments A1–A15

| Amendment | Assessment | Evidence |
|---|---|---|
| A1 authorization | Applied | Atomic challenge deletion + Prepared insertion: M:210, M:236, S:27, S:253. Historical C6 marked superseded in 131c:7 and 131d:7. |
| A2 archival | **Incomplete/inconsistent** | Correct eligibility/tombstones at M:214, E:49, S:334; findings P1-3, P1-4 and P2-2 remain. |
| A3 no migration | Applied, surface incomplete | No backfill and typed refusal at M:73; missing surface mapping in P2-3. |
| A4 unsupported variant | Applied | Deleted enum alternative at M:99–103, M:132, S:251; historical clause superseded at 131d:10. |
| A5 retire fence | Substantially applied | Same-mutex flag check/live WAL permit and token-versus-epoch distinction: M:460–468, S:330. The old three-site claim is withdrawn. |
| A6 targeted drain | Applied | Full three-phase contract and no control-held join: M:237; repeated at E:46. |
| A7 durable mutation | **Not consistently applied** | New API/table at M:186–196 conflicts with R1 steps/crash rows: P1-2. |
| A8 migration | Applied | Global namespace order, identity recheck and mandatory 3c case: M:442, E:37, E:48, S:125. |
| A9 log contract | **Incomplete** | P1-1. |
| A10 barrier execution | Applied as policy; composition incomplete | `spawn_blocking`, timeout, group acknowledgements and metric: M:208, S:321. Outstanding timed-out I/O needs the P1-1 serialization contract. |
| A11 ownership | Applied except residual writer contradiction | M:70–74 versus M:75. |
| A12 Windows | Applied | Required destruction/release, pre-mutation reseed refusal and mandatory follow-up: M:39, E:27, E:44. |
| A13 citations | **Not complete** | Citation findings below. |
| A14 staging gate | Applied | M:50, M:231, E:31, E:36–37, S:98, S:331. |
| A15 normative consolidation | Substantially applied | Headers identify the normative set and historical documents: M:3, E:3, S:3, 131c:3, 131d:3. Obsolete normative statements identified above still need removal. |

### Nine-question ruling

| Row | Assessment |
|---|---|
| 1 Ownership/hybrid | Correct substantive choice at M:70–74; fix writer definition at M:75. |
| 2 Windows | Applied at M:39 and E:44. |
| 3 Durable transactions/recovery | **Not consistent**, P1-2. Missing database flush correctly assigned to 3a: E:35. |
| 4 Targeted drain | Applied at M:237 and E:46. |
| 5 Permanent system-store table | Applied at M:79 and E:47. |
| 6 Migration interlock | Applied at M:442 and S:125. |
| 7 No archival release | Correct main rule, but **residual escape/schema gap**, P1-3/P1-4. |
| 8 Audit log | **Incomplete**, P1-1. |
| 9 Barrier execution | Main rule applied at M:208; timed-out append ownership needs completion. |

Historical archival, resubmission and epoch claims in **131c/131d are not additional live contradictions**: their explicit non-normative/supersession headers settle precedence.

## 3. Loss against `eeee8e98`

**OBSERVED:** Comparing the five documents against `eeee8e98`, I identified **no unauthorized removal of a schema, field list, state-machine row, invariant, error-table row or CI case**. The restored schema appendix remains present at S:238–323; I1–I14 remain at M:47–60; the full state table remains at M:249–257; the fifteen CI cases remain at E:11–27.

The substantive removals/replacements I identified are DEC-3-authorized:
- legacy backfill → unsupported-format refusal: M:73;
- `UnsupportedBackend` row alternative → preflight refusal: M:132;
- challenge-failure same-ID remedy → atomic authorization: S:27, S:86;
- Blocked archival/release → successful-terminal-only compaction: S:334;
- public destruction authority → `pub(crate)` staging: S:331;
- three-site admission argument → full inventory/fence: M:460–468.

**Qualification:** This is a document comparison finding, not proof of semantic completeness. The remaining problems are mostly retained contradictory instructions or incompletely propagated additions, rather than content loss.

## 4. Citation audit

I re-read **more than 25 cited source locations**. Correct examples include the installer’s Unix gate, name check, directory creation, maintenance lock, `find_complete`, `init_seed`, target check, import, publish, registration and subscription persistence; staging’s destination/attempt/marker/candidate-open/find/publish sites; DTO definitions; TS canonicalizers/builders; Rust NUL tests; strict audit append; scheduler checkpoint; supervisor lifecycle sites; and follower guard release.

### Wrong or misleading current citations found

| Document reference | OBSERVED correction |
|---|---|
| M:64, M:79; E:98, E:139 — `shamir_db/system_store.rs` | File does not exist. Configuration/constants/initialization/delete helper are in `crates/shamir-db/src/shamir_db/system_store/store.rs:55–119`, `:192–213`. |
| M:240; E:45, E:98; S:255 — `system_store.rs:262–275` / `:330–352` | Database removal is `crates/shamir-db/src/shamir_db/system_store/catalogue.rs:33–45`; repository removal/filter/flush is `:101–123`. |
| E:103, E:123–127; S:115, S:122 — db-management shorthand | Actual path is `crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs`, not the path defined by E:103. The extra `db/` is missing. |
| M:306; E:114–120; S:127 — `execute/admin_replication.rs` | Actual path is `crates/shamir-db/src/shamir_db/execute/admin_db/admin_replication.rs`; DROP signature/control/delete remain at `:394/:400/:429`. |
| E:121–122, E:138; S:121 — `execute/admin_db_repo.rs` | Actual path is `crates/shamir-db/src/shamir_db/execute/admin_db/admin_db_repo.rs`. `:267` obtains the registry entry; mutex acquisition is `:271`. |
| E:139 — `execute/admin_migration.rs` | Actual file is `crates/shamir-db/src/shamir_db/execute/admin_db/admin_migration.rs`. |
| S:72 — `execute/helpers.rs:85` | Actual file is `crates/shamir-db/src/shamir_db/execute/core/helpers.rs`. |
| M:33, M:291, M:326; E:98 — quarantine rebuild `core.rs:603–605` | Actual call is `crates/shamir-db/src/shamir_db/shamir_db/core.rs:606`; cited range is blank/comment text. |
| M:59, M:72, M:319; E:98; S:124 — seed gate/publication `:381/:394` | Actual calls are `core.rs:382` and `:395`. |
| M:442; E:48, E:98; S:125 — migration field/function `:142–143/:705` | Field is `core.rs:144`; function is `:706`. Referenced lines are documentation. |
| E:110 — lock API `core.rs:656–657` | Signature is `:657`, constructor/lock acquisition `:658`; the cited range does not include acquisition. |
| E:130 — control acquisition `supervisor.rs:428/:459` | `:428` is the start signature; `:459` is lease acquisition. Control acquisition is `crates/shamir-server/src/sync/replication/supervisor.rs:434`. |
| E:135 — deferred `pending_stall :937` | Assignment is `crates/shamir-server/src/sync/replication/follower_loop.rs:936`; `:937` is `sleep_backoff`. The important read/drop citations `:832/:940` are correct. |

Also, S:30 describes a digest-keyed HMAC as if observed today. The actual current arms at `crates/shamir-server/src/engine/db_handler/handlers/admin.rs:656–659` sign **logical names**, not the proposed preview digest. S:66 correctly treats their v2 conversion as future work. Mark S:30’s digest behavior **DECIDED**, not current behavior.

## 5. Minimal edits required for merge

1. Complete A9’s serialized-writer, bounded-duplicate and concrete framing contract, including timed-out blocking append reconciliation and CI cases.
2. Reconcile R1’s detailed transactions/crash table with A7.
3. Remove the rename-squatter completion escape; require destruction or durable protection transfer.
4. Give `SubscriptionRemoval` sufficient frozen identity evidence for required tombstones.
5. Correct ownership writers, `Prepared`/`Blocked` summaries and obsolete Blocked-retirement wording.
6. Define the unsupported-format error’s surface and correct the error count.
7. Refresh the incorrect current-checkout citations and distinguish proposed digest verification from observed code.

**Recommendation:** Keep #196 unmerged until those edits are incorporated, then perform a focused read-only consistency review. The architecture need not be redesigned, but these are not issues to delegate to implementation PRs to interpret independently.

**Files changed:** none. **Builds/tests:** none run. **Verification performed:** read-only document/source inspection and baseline Git comparisons only.
--- REVIEW

--- WARN None

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.

Final message (mandatory): for each of P1-1..P1-4, P2-1..P2-4 the file and section where it is fixed and the decision taken; the citation list with file:line you re-read; anything you could not do exactly, with the reason. Never end a turn while a child agent is outstanding. Stop editing at 130 minutes and report exactly what is done and what is not.
