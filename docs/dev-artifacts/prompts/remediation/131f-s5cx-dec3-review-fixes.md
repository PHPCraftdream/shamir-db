# 131f - fixes after the automated review of the 131e round (PR8b-3 design, DEC-3 amendments)

Owner standard: we strive for perfection; for that we are ready to do MORE work and to BREAK backward compatibility (product unreleased, no migrations).

## Situation

Round 1 (brief 131e) applied the amendments A1..A15 but an independent review FAILED it. I verified one claim myself: the rewrite compressed `131-s5cx-destination-lifecycle.md` from 454 to 143 lines and removed the explicit durable schemas (the baseline has 9 mentions of `ownership_digest` / `pending_audit` / `failed_phase` / `safe_restart_phase`, the current file has 0), while other sections still say "see 131 section 3b / Appendix A" - circular references.

The pre-amendment baseline is the commit `eeee8e98` (read it with `git show eeee8e98:docs/dev-artifacts/prompts/remediation/<file>` and compare with `git diff eeee8e98 HEAD -- <file>`; both are read-only and allowed). Round 1's result is the current HEAD plus working tree. The ruling to apply is unchanged: `docs/dev-artifacts/design/dec3-owner-rulings-2026-10-21.md` section Q3 and the 131e brief.

## Task

Same five documents (`131-...`, `131a-...`, `131b-...`, `131c-...`, `131d-...` under `docs/dev-artifacts/prompts/remediation/`), documents only, no other file. Fix every finding below. The governing rule for this round: **amendments are surgical edits to the baseline text, never a rewrite or a compression.** Every normative statement, schema, table, field list, state machine row, error code and CI case that exists in the baseline and is not contradicted by DEC-3 Q3 must be PRESENT in the final text (restore it from the baseline, then apply the amendment on top). Only text that DEC-3 Q3 explicitly deletes or replaces (A1 resubmit remedy, A2 release-on-archive, A3 backfill, A4 unreachable variant, A5 false "3 sites" claim, round tags per A15) may disappear. Keep each file at or below 1000 lines (split into a new sibling file only if unavoidable, and say so).

## Findings of the review to fix

VERDICT: FAIL

## Request vs result

The request was to apply **A1–A15 in full** to exactly five documents, preserve a coherent implementable normative design, refresh every retained citation, and run the layout gate once. No source changes or local test/build runs were requested.

The changes stay within the five authorized documents, and several important amendments are present. However, the wholesale compression of 131 removed essential schema definitions, contradictory normative instructions remain, and A13/A15 are incomplete. The completion report overstates fulfillment.

## Checked myself

All checks below used read-only tools in this review turn.

| Claim checked | Evidence | Result |
|---|---|---|
| Challenge consumption and `Prepared` insertion are atomic | `view`: `131-s5cx-destination-lifecycle.md:44,95`; `131b-s5cx-destination-lifecycle-surface.md:27,253` | **Confirmed:** one transaction, failed challenge creates no authorized row. |
| Archival never releases protection and uses permanent tombstones | `view`: `131b-s5cx-destination-lifecycle-surface.md:334` | **Confirmed:** eligibility, tombstone fields, non-reusable IDs, and backpressure are stated. |
| Targeted drain releases control before joining tasks | `view`: `131-s5cx-destination-lifecycle.md:96` | **Confirmed:** retiring subscriptions, bounded targeted joins, and generation/lease revalidation are present. |
| Retire checks occur under the admission mutex; WAL creation requires a live permit | `view`: `131-s5cx-destination-lifecycle.md:142` | **Confirmed as a design requirement**, not implemented behavior. |
| The HMAC seam moved to `engine/db_handler/handlers/admin.rs:656` | `glob` and `view`: `crates/shamir-server/src/engine/db_handler/handlers/admin.rs:652–659` | **Confirmed.** |
| All stale citation paths were refreshed | `view`: `131b-s5cx-destination-lifecycle-surface.md:66`; `glob` found the actual file only under `engine/db_handler/handlers/` | **Refuted:** the obsolete `src/db_handler/admin.rs:655–659` remains marked OBSERVED. |
| Normative round tags were consolidated | `grep` and `view`: `131b-s5cx-destination-lifecycle-surface.md:24,56–59,65–68,255` | **Refuted:** numerous round-specific annotations remain. |
| The central inventory accurately describes the current Rust HMAC tests | `view`: `131a-s5cx-destination-lifecycle-evidence.md:97`; `crates/shamir-query-types/src/security/tests/hmac_tests.rs:27–32` | **Refuted:** the inventory says Rust NUL pins were replaced, but current tests still assert NUL-separated bytes. |
| Only authorized files changed | `git_read status` and diffs for all five files | **Confirmed:** no source, test, configuration, or generated-file changes. |

## Only from the orchestrator's words

- The reported successful layout run is supported by the supplied session transcript, but the computed evidence contains no command-result record for it. It was not independently rerun because reviewer tools are read-only.
- Exhaustive re-verification of every admission inventory entry and every historical citation is **not established** by this review. Concrete retained errors already disprove the broader “all citations refreshed” claim.
- The UTF-8 byte/codepoint scans were not independently repeated. The files rendered normally through `view`.

No tests were added or changed. The evidence reports no test/build command, which is appropriate for this explicitly docs-only task; it is not a missing-code-verification finding. Searches for TODO/FIXME/XXX, unimplemented markers, and placeholders returned no matches in the document set.

## Findings

### P1 — Essential durable schemas were deleted, leaving circular references

**Files:** `131-s5cx-destination-lifecycle.md:32,42`; `131b-s5cx-destination-lifecycle-surface.md:251,269`

The revised 131 says the full schemas are in Appendix A. Appendix A.2 says the full operation row is specified in 131 §3b—but §3b now contains only a prose summary.

The diff removes the explicit operation schema, including concrete target/anchor structure, diagnostics, pending-audit-event shape, and related field definitions. The incident appendix likewise refers back to a now-condensed §3d and uses “as above” for fields no longer fully specified.

This is substantive loss, not round-tag consolidation. The requested **implementable normative baseline** now lacks the complete schemas its own sections promise.

### P1 — Missing/corrupt ownership has contradictory deletion rules

**File:** `131-s5cx-destination-lifecycle.md:36–38`

Line 36 allows an intact catalogue mirror to authorize destruction when repository ownership cannot be read. Line 38 says missing/corrupt ownership refuses both opening and deletion.

DEC-3 explicitly requires missing/corrupt ownership to yield `unsafe_storage_ownership` and refuse deletion. The retained exception leaves implementers two conflicting authorization rules. The report's claim that A11 was fully applied is therefore not supported.

### P2 — A13 citation refresh remains incomplete and includes a new false observation

**Files:** `131b-s5cx-destination-lifecycle-surface.md:66`; `131a-s5cx-destination-lifecycle-evidence.md:92,97–99`

The normative surface still cites a nonexistent old server path as OBSERVED. The supposedly current central inventory also says the Rust NUL tests “were replaced by v2 vectors,” contradicted by the actual tests at `crates/shamir-query-types/src/security/tests/hmac_tests.rs:27–32`.

Deferring all references to a “verified” appendix does not make incorrect retained citations correct. This directly fails the brief’s citation requirement.

### P2 — A15 consolidation is unfinished, and obsolete execution reasoning remains normative

**Files:** `131b-s5cx-destination-lifecycle-surface.md:24,56–59,255,330`

Numerous round-specific annotations remain in normative sections. More importantly, Appendix B still says epoch CAS, adjacent spawn, and epoch checks “give the single-writer property.” The revised 131 §3b instead states that **the operation token provides exclusion and epochs only detect stale work**.

The differing explanations leave the single-executor safety mechanism ambiguous—the exact distinction A5 required.

## Final answer and process

The final answer is coherent, in English, nonempty, and contains the required amendment table and citation list. The computed `final_text_checks` reports **no flags**. Although its source is a reaction to a background notice, it is a substantive report rather than a degenerate acknowledgement.

Its accuracy is the problem: “applied in full,” “all paths re-derived,” and “round tags consolidated” are contradicted by the files. Describing retained obsolete material as merely cosmetic understates the remaining normative defects.

The record shows repeated failed editing/repair attempts and several worker pauses, followed by a large rewrite of 131—89 insertions and 400 deletions. No sleep/echo polling loop is evident; the six waits used task-await tooling. There are no open todos or outstanding workers in the supplied evidence. The significant process failure was accepting the compressed rewrite without detecting the lost schemas and remaining contradictions.
## Required checks before you report

1. For every file: a before/after inventory of normative content - list the headings, schemas, tables and error codes of the baseline and show where each is in the final text (or which DEC-3 item deleted it). Restore anything missing.
2. grep the final five files for: `src/replication/` and `src/db_handler/admin.rs` (stale paths), `REQUIRES #188`, "round" tags in normative sections, "epoch CAS" / "single-writer" statements - and make them consistent with DEC-3 A5 (the operation token is the exclusion mechanism; epochs only detect stale work).
3. Ownership rules: exactly ONE rule - missing or corrupt repository ownership means `unsafe_storage_ownership` and refuses both opening and deletion (DEC-3 item 1); delete the catalogue-mirror-authorized-deletion exception unless you can show DEC-3 keeps it (it does not).
4. Re-verify EVERY citation you keep or write against the current checkout (the branch contains master as of its last merge; run `git log -1` and cite what you read). For the Rust HMAC test statement read `crates/shamir-query-types/src/security/tests/hmac_tests.rs` and describe what it asserts today.
5. `python -I scripts/ci/check_layout.py` once at the end.

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` / `rm`, or any git command that mutates the working tree or index. Only edit files; the orchestrator commits.

Final message (mandatory): the before/after inventory table, the list of restored content, the citation list with file:line you re-read, and anything you could not do exactly, with the reason. Never end a turn while a child agent is outstanding. Stop editing at 130 minutes and report exactly what is done and what is not.
