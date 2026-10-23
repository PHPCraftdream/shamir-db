# 131e - DEC-3 amendments to the PR8b-3 destination-lifecycle design (draft PR #196)

Owner standard: we strive for perfection; for that we are ready to do MORE work and to BREAK backward compatibility (product unreleased, no migrations).

## Task

The adjudicator ruling DEC-3 (full record: `docs/dev-artifacts/design/dec3-owner-rulings-2026-10-21.md`, section Q3) found the PR8b-3 design architecturally right but not implementable as written. In THIS worktree (branch `docs/pr8b3-lifecycle-design`, PR #196, docs only) apply amendments A1..A15 below to the design documents `docs/dev-artifacts/prompts/remediation/131-s5cx-destination-lifecycle.md`, `131a-...-evidence.md`, `131b-...-surface.md`, `131c-...-open-defects.md`, `131d-...-review-round4-findings.md`.

Rules:
- Documents only. Touch no source code, no workflow, no other file except the five 131*.md documents (and, if a new section needs a home, only inside them). Do not edit `docs/dev-artifacts/design/dec3-owner-rulings-2026-10-21.md`.
- Verify every citation (file:line) you write or keep against the CURRENT checkout (this branch contains master as of the last merge); the stale ones named in A13 must be re-derived, not guessed. Separate OBSERVED from DECIDED from INFERRED in the text as the existing documents do.
- Result must read as ONE coherent normative design (131, 131a sections 9-11, 131b) with the "round N" tags consolidated into final DECIDED text (A15); 131c and 131d become historical and non-normative.
- Keep every file at or below 1000 lines (layout rule; split a document into a new sibling file only if unavoidable and say so).
- Final message: a table A1..A15 -> file and section where applied; the list of re-derived citations with their new line numbers; and any amendment you could not apply exactly as written with the reason. Never end a turn while a child agent is outstanding. A final message is mandatory; stop editing at 130 minutes and report exactly what is done and what is not.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.

## Ruling to apply (verbatim from DEC-3, Q3)

## Q3 - PR8b-3 design and draft #196

**RULING:** The architecture is right but the text is not implementable as written. Land the amendments below as commits on #196's own branch, then mark #196 ready and squash-merge it as the single normative baseline before any 8b-3 code.

### The nine questions

| # | Ruling | Exact modification |
|---|---|---|
| 1 | Accept, strengthened | Hybrid writes `StorageOwnership` into its durable fjall tier. Ownership is minted only on the controlled creation path and published durably before the repository is exposed. Missing or corrupt ownership gives `unsafe_storage_ownership` (refuse to open or delete), never minted lazily. Storage ownership is stated as distinct from replication lineage, which stays per-boot for hybrid. |
| 2 | Modify | Accepted for the 8b-3 series only. Windows reseed is refused before any install mutation, certified on Windows CI (case 15). Windows destruction and name release stay required. Record "Windows seed install" as a mandatory end-state follow-up, not a permanent product limitation. |
| 3 | Modify | Every lifecycle transition that writes >=2 system-store records does so in ONE system-store transaction plus ONE flush, through a typed `SystemStore::commit_durable(batch) -> DurableReceipt`, the only lifecycle write path. "Commit ok, flush failed" is a typed `DurabilityUnknown`, resolved by a forced flush and then a re-read. Add an explicit per-boundary recovery table (each durable prefix -> meaning -> boot action). Fix the `remove_database` missing flush in 8b-3a, not 8b-3c. |
| 4 | New targeted drain API; reject `stop_all` | `drain_targets(targets, generations) -> DrainReceipt`: (a) under control, invalidate and drain each affected lease (existing `quiesce_subscription_lease_locked`) and mark those subscriptions `retiring` so reconcile and start skip them, then release control; (b) outside control, stop and join only those loops, with a bounded timeout (timeout -> `Blocked`, fail-closed); (c) re-take control, verify the generations are unchanged and no lease exists, return the receipt. A task must never be joined while control is held. |
| 5 | Accept | System-store table, permanently. No separate journal. |
| 6 | Modify | The API is reachable through config today (verified), so migration must be interlocked now. It acquires both namespace admissions in global `(db, repo)` order before control and write locks, re-validates identity after acquisition, and refuses reserved targets. The CI case "migration during reserved name" becomes mandatory in 8b-3c, not conditional. |
| 7 | Reject release; accept no expiry | `destruction_archive` never releases protection. Eligible only for Repo/Database `Destroyed` + fully acked, and SubscriptionRemoval `Completed`. Archival replaces the row with a permanent compact tombstone `{operation_id, kind, final_state, frozen storage_object_ids, sha256(full row), archived_at, archived_by}`; operation ids are never reusable. A final `Blocked` operation exits only through `destruction_retry` -> `Destroyed`. A `Blocked` SubscriptionRemoval at `RowDeleted` whose re-verification found data converts that data into an R2' target or an incident, never a release. Backpressure: a quota on concurrent non-terminal operations; new destruction is refused when the quota is reached; nothing protected is ever evicted. |
| 8 | Accept the proof rule; tighten the log | Event id = `(operation_id, transition_ordinal)`, bound to a SHA-256 of the canonical payload. The same id with a different digest is corruption: poison plus incident, never dedup. Define the record framing (length + checksum). On open, the sink truncates an incomplete or bad-checksum final record before any append. Exactly one serialized writer. At most one duplicate per ambiguous append. "Found after restart" is never proof. |
| 9 | Accept | Flush-before-return stays. The barrier I/O runs off Tokio workers (`spawn_blocking`) with a bounded timeout; on timeout the row stays `Prepared` with `destruction_audit_unavailable`. Group commit allowed only if each acknowledgement covers its own sequence number. Latency exposed as a metric. |

### Soundness and top risks
Sound enough to build 8b-3a..c on only after the amendments. Top 3 risks:
1. Authorization bypass through the stranded-`Prepared` resubmission (a consumed or stale challenge becomes authority).
2. Resurrection: stale `RepoInstance` clones, lazy `repo_wal()` `create_dir_all`, Windows handle semantics can recreate storage after deletion; the retire-fence argument rests on a false 3-site `admit` inventory.
3. Staged exposure and lock order: a destructive dispatch could become reachable before publisher wiring (8b-3c), or a drain could join tasks under control.

### Amendments that must land on the #196 branch before merge
- **A1 (authorization):** at Prepare (i), challenge DELETE-IF-MATCHES (challenge_id + digest) and the `Prepared` PUT-IF-ABSENT happen in ONE system-store transaction under the held admissions. If the challenge fails, no row is written (insert-if-absent `Rejected`, or just an error). Delete the C6 "resubmit same id" remedy. A resubmission can find a row only if authorization succeeded.
- **A2 (archival):** rewrite item 7 per the table. Update 131 3e.7, 131a 11.7 and 131b Appendix B. Mark 131c C8 and F1b as superseded.
- **A3:** delete the legacy backfill (131:75). Pre-8b-3a data directories are refused with a typed `storage_format_unsupported` error. No migrations.
- **A4:** delete the unreachable `UnsupportedBackend` row variant (131:125).
- **A5:** re-derive the retire fence from the full `admit` inventory (131a rows 27-30) and delete the 131:440 "only 3 sites" claim. Linearization point: `retire` sets the flag under the repo admission mutex and drains permits. Every `admit`, `admit_with_reservation` and `admit_pair` checks the flag under the same mutex. `repo_wal()` creation requires a live permit. The operation token is the exclusion mechanism; epochs only detect stale work.
- **A6:** the item 4 drain contract, verbatim.
- **A7:** the item 3 durable-mutation API and recovery table.
- **A8:** the item 6 migration interlock, with the case made mandatory.
- **A9:** the item 8 log contract.
- **A10:** the item 9 execution rule.
- **A11:** item 1 recorded as DECIDED.
- **A12:** item 2 refusal plus the follow-up entry.
- **A13:** refresh every citation against current master (`src/sync/replication/...`, the `follower_loop` path). Replace every "REQUIRES #188" with OBSERVED merged references and re-verify line numbers.
- **A14 (staging gate):** the 8b-3b executor and authority are `pub(crate)` and not dispatchable from wire or admin. Destructive dispatch becomes reachable only in 8b-3c, together with all publisher wiring. State this as an acceptance criterion of 8b-3b and 8b-3c.
- **A15:** one normative set (131, 131a 9-11, 131b) with the "round N" tags consolidated into final DECIDED text. 131c and 131d are marked "historical, non-normative". Add a header citing this decision record (DEC-3).

Then #196 goes ready and is squash-merged. Implementation briefs for 8b-3a..e cite the merged file.

### Reviewer disagreements
- #2: ops accepted unchanged; @oh sided with endstate and adversary (modify).
- #3: endstate accepted; @oh sided with ops and adversary (modify, atomic transaction plus recovery table).
- #196 timing: all three said "merge after folding in"; @oh agrees.
- A1, A3 and A5 come from single reviewers (endstate for A1 and A3, adversary for A5); each verified and adopted.

### Implementation order
Amendment commits (docs; review is the only check, CI trivial) -> #196 ready -> merge -> 8b-3a -> 8b-3b -> 8b-3c -> 8b-3d -> 8b-3e (each CI-only). Independent of the other questions.

### What would change my mind
A durable protection-transfer protocol that provably cannot reopen old storage would allow archival with release. A product decision that destruction is a global-maintenance operation would allow `stop_all`.

---

