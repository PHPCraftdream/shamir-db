# 131h - last fixes after the review of round 3 (PR8b-3 design, DEC-3)

Owner standard: we strive for perfection; for that we are ready to do MORE work and to BREAK backward compatibility (product unreleased, no migrations).

## Situation

Round 3 (brief 131g) is the HEAD commit of this branch. The review of round 3 found two remaining P1 design gaps and two P2 items (text below, verbatim). The orchestrator has DECIDED the design points; implement exactly these decisions in the documents (same five files under `docs/dev-artifacts/prompts/remediation/`, documents only, surgical edits, nothing that exists may be compressed or dropped, at most 1000 lines per file, every citation re-read against the checkout).

## Decisions to write into the documents

1. **Same-event coalescing and a durable duplicate budget (review P1-A).** The serialized sink writer keeps an in-flight table `event_id -> shared completion`. A second submission of an event id that is in flight JOINS that completion (no second append) and receives the same result. After an `Acked` completion, a later submission of the same event id returns the recorded acknowledgement (from the durable `audit_pending`/ack state) without appending. A duplicate record can therefore arise only from a re-write after a CLASSIFIED-AMBIGUOUS outcome (failed or timed-out append, crash before an acknowledgement is recorded). Durable budget: every `audit_pending` entry carries `append_attempts: u8`, incremented inside the same `commit_durable` transaction that precedes each re-write; the number of duplicates of an event is at most `append_attempts - 1`; `MAX_APPEND_ATTEMPTS = 3`; when the cap is reached the operation parks as `Blocked(audit_unavailable)` until an operator acts - no silent retry loop. Update the "at most one duplicate per ambiguous append" wording accordingly ("per classified-ambiguous outcome, bounded in total by the attempt cap") and CI case 16-19 in 131a (add the coalescing case and the attempt-cap case; keep numbering coherent).
2. **Binding lookup after compaction (review P1-B).** The compaction tombstone additionally carries `subscription`, `binding_generation` and `incident_refs` (the incident ids of the removal record). A new permanent compact system-store table `destruction_bindings` maps `(subscription, binding_generation) -> operation_id`, is written in the same transaction that creates a `SubscriptionRemoval` operation, is never deleted by compaction, and is the lookup used by retry/status; after compaction it resolves to the tombstone. Update the schema in 131b (Appendix A) and the prose/recovery text in 131.
3. **Bounded-join failure (review P2-A).** If the bounded join of a timed-out blocking append ALSO times out, the sink writer enters the `Parked` state: it accepts no further appends for ANY event, new requests fail fast with `destruction_audit_unavailable`, the blocking task handle is retained and re-polled non-blockingly on an interval; completion -> classify -> resume. A process restart treats the in-flight append as ambiguous and the boot replay applies decision 1. Operation rows stay `Prepared`/`Blocked` exactly as the state machine already says. Define the client-visible response (bounded, never waiting on the blocking task) and add a CI case.
4. **Citations (review P2-B).** Fix `core.rs:394` -> `core.rs:395` in 131:344; fix the nonexistent `131b section 6d/A.9` reference in 131a:31 to the correct section; fix the `repo_create_locks` accessor citation in 131a:148 (signature `core.rs:665`, returned field `:666` - re-read before writing). Then re-read at least 25 OTHER citations you did not touch and report them in the final message as a list with the file:line you read.

## Review findings (verbatim)

VERDICT: FAIL

## Request vs result

Brief 131g required surgical fixes for all four P1 design gaps and four P2 inconsistencies, citation re-verification, corresponding CI requirements, and one layout-gate run.

The changes remain within the authorized documents and substantially improve the design. Atomic R1 deletion/phase updates, explicit audit framing, no-rename protection rules, and several citation corrections are present. However, the audit duplicate guarantee and post-compaction binding lookup remain underspecified. The claim that the brief was applied **in full** is therefore premature.

## Checked myself

The following checks used read-only tools during this review turn.

| Check | Evidence | Result |
|---|---|---|
| Concrete audit-record framing | `view`, `131b-s5cx-destination-lifecycle-surface.md:323–327` | **Confirmed:** field order, widths, checksum coverage, size limit, and final-record truncation are specified. |
| R1 delete and phase update are atomic | `view`, `131-s5cx-destination-lifecycle.md:304,314–315` | **Confirmed:** one `commit_durable` transaction deletes the generation-matched subscription and records `RowDeleted`; recovery reads both together. |
| Rename/removal is no longer sufficient to complete blocked removal | `view`, `131-s5cx-destination-lifecycle.md:320`; `131b-s5cx-destination-lifecycle-surface.md:340` | **Confirmed:** verified destruction or durable identity-bound incident handoff is required. |
| Ownership writers distinguish creation from opening | `view`, `131-s5cx-destination-lifecycle.md:75` | **Confirmed:** opening existing storage never mints ownership. |
| `Prepared` no longer implies completed audit barrier | `view`, `131-s5cx-destination-lifecycle.md:138` | **Confirmed:** durable authorization and barrier status are separate. |
| Unsupported-format diagnostic is documented | `view`, `131b-s5cx-destination-lifecycle-surface.md:90` | **Confirmed:** variant, code, surfaces, and remedy are stated outside the eleven lifecycle DDL codes. |
| Corrected source citations | `view`, `core.rs:606,657–658`; `system_store/store.rs:172`; `system_store/catalogue.rs:33–45`; server `admin.rs:656–659` | **Confirmed:** inspected references match actual source. |
| CI cases 16–19 exist | `view`, `131a-s5cx-destination-lifecycle-evidence.md:28–31` | **Confirmed:** serialization, timeout ownership, duplicate bound, and framing cases are specified. |
| Preserve binding-keyed lookup after compaction | `view`, `131b-s5cx-destination-lifecycle-surface.md:257,340` | **Not fully satisfied:** preservation is asserted, but the compacted representation or surviving lookup index is not defined. |
| Scope | `git_read status` and diffs | **Confirmed:** only 131, 131a, and 131b changed; no source, tests, configuration, or generated files changed. |

## Only from the orchestrator's words

- The layout gate reportedly returned exit 0. The supplied transcript shows that result, but the computed evidence contains no separate command-result entry. This reviewer did not run commands.
- Exhaustive citation re-reading, including the required 25 untouched citations, is not independently established by the final report. Several sampled corrections are confirmed.
- Exact UTF-8 codepoint counts were not independently repeated; the inspected documents render normally.

No executable tests were added or changed. The evidence reports no test/build run, which is appropriate for this expressly documents-only task. The new CI cases are future acceptance requirements, not executed tests.

## Findings

### P1 — Serialization is incorrectly treated as same-event deduplication

**Files:** `131-s5cx-destination-lifecycle.md:215–217`; `131a-s5cx-destination-lifecycle-evidence.md:28–30`

The new contract says routing requests through one serialized queue prevents two submitters from producing two records for the same event ID. Serialization alone does not do that: it can append both requests sequentially.

Case 16 requires exactly one record and the same acknowledgement for both submitters, but the design specifies no same-ID coalescing or acknowledged-result reuse mechanism. Likewise, “once per ambiguous attempt” and “once per boot” do not establish a durable duplicate budget across repeated crashes or an ambiguous reconciliation rewrite.

**Inferred consequence:** an implementation can satisfy the queue ordering literally while violating the stated duplicate guarantee. P1-1 required this mechanism to be settled, not merely asserted.

### P1 — Compaction still discards the binding lookup key and identity-reference structure

**Files:** `131b-s5cx-destination-lifecycle-surface.md:257,340`; `131-s5cx-destination-lifecycle.md:284–295`

The full removal record now carries frozen IDs and incident references, which is an improvement. However, the specified replacement tombstone still contains only:

`operation_id, kind, final_state, frozen storage_object_ids, sha256(full row), archived_at, archived_by`

It omits `subscription`, `binding_generation`, and the incident-reference representation used for `None` destinations. No permanent binding-to-operation index is defined. A hash of the removed row cannot recover those fields.

The statement that binding-keyed retry/status remains “UNCHANGED after compaction” therefore lacks a supporting durable representation. This leaves P1-4 incomplete.

### P2 — Timeout ownership stops short of defining bounded-join failure

**Files:** `131-s5cx-destination-lifecycle.md:216`; `131b-s5cx-destination-lifecycle-surface.md:326`

The writer must wait for actual completion using a bounded join, then classify the result. The documents do not specify what happens if that join also times out, nor distinguish a bounded client response from potentially unbounded ownership of a still-running blocking task.

The prohibition on further appends is conservative, but the terminal parked/degraded state and reconciliation path remain undefined. This is still an implementation decision within the P1-1 timeout scenario.

### P2 — Citation and cross-reference cleanup is not complete

**Files:** `131-s5cx-destination-lifecycle.md:344`; `131a-s5cx-destination-lifecycle-evidence.md:31,148`

- M:344 still attributes publication/recovery to `core.rs:394`; actual publication is at `core.rs:395`.
- E:31 points to `131b §6d/A.9`, but no §6d exists.
- E:148 cites the `repo_create_locks` accessor at `:664–665`; the signature is at `core.rs:665`, and its returned field is at `:666`.

These contradict the report’s blanket citation-completion claim, although the principal corrected references inspected here are accurate.

## Final answer and process

The final answer is coherent, English, nonempty, and a substantive report. The computed `final_text_checks` has **no flags**; its background-notice origin did not produce a degenerate acknowledgement.

It nevertheless overstates completion and does not supply the detailed re-read citation list requested by `131g:206`. It also describes changed-line totals as “lines,” rather than distinguishing insertions and deletions.

The evidence shows one delegation and one task-await call, no sleep/echo polling loop, no open todos, and no outstanding worker. Stub-marker searches returned no matches. Verification was primarily textual spot-checking; it confirmed many requested additions but did not establish that the new audit and compaction guarantees follow from the specified mechanisms.
⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.

Final message (mandatory): for each decision 1-4 the file and section where it is written; the citation list (file:line you re-read) with at least 25 entries; anything you could not do exactly, with the reason. Never end a turn while a child agent is outstanding. Stop editing at 130 minutes and report exactly what is done and what is not.
