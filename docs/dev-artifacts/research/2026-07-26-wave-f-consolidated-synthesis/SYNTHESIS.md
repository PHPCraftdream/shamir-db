# Wave F post-wave — consolidated synthesis of three independent reviews

Date: 2026-07-26. HEAD at review time: `5270aa92`.

Three independent read-only reviews of the same Wave F campaign (F-1..F-15,
#791-#805, follow-up #808) were run in parallel/sequence:

1. **`@oh`** (Agent, Opus high) — `docs/dev-artifacts/research/2026-07-26-wave-f-post-review/REPORT.md`
2. **`/crush`** (independent session) — `docs/dev-artifacts/research/2026-07-26-wave-f-post-review-crush/REPORT.md`
3. **New-wave release review** (deeper static audit, diff `4d5436a0..HEAD`, 30
   commits/127 files) — `docs/dev-artifacts/research/2026-07-26-new-wave-release-review.md`

Review 3 is materially more thorough than 1/2 and reaches a stronger verdict:
**do not tag `0.1.0-alpha.1` on the current HEAD yet** — it found 5 P0s, three
of which (R2, R4, R5) were **missed entirely** by both of the earlier
reviews. I (orchestrator) personally spot-verified the three novel P0 claims
against the current source before trusting them (zero-trust discipline
applied to review artifacts, not just code artifacts):

| Claim | Verified? | Note |
|---|---|---|
| R5 — `SelectItem::Expression` silently produces nothing | **CONFIRMED** | `select_projection.rs:87-108`'s match has a bare `_ => {}` arm; `Expression` falls through it exactly as described. Not caught by either `@oh` or `/crush`. |
| R2 — bootstrap TTL fail-open in login path | **CONFIRMED** | `handshake.rs` has zero `bootstrap_token_expired` checks — only `bootstrap_username()` matching, then a best-effort, explicitly non-fatal rotation attempt. An expired-but-unrotated token still authenticates. Not caught by either `@oh` or `/crush`. |
| R4 — FK RESTRICT/CASCADE/SET NULL TOCTOU | **CONFIRMED, but pre-existing & self-documented** | `fk_restrict.rs:9-18`'s own doc comment already states this exact caveat verbatim ("tracked as a future task"). Real, but not a *new* discovery — a consciously accepted, already-documented tradeoff that predates this wave. Downgrading from "surprise P0" to "known, worth re-litigating given release stakes." |

## Cross-reference: what each review found, deduped

| Consolidated ID | Severity (post-verification) | Found by | One-line |
|---|---|---|---|
| **C1** (= R1 = `@oh` N-1) | **P0/P1** (three-way confirmed) | all three | F-1's schema-typed keyset gate proves nothing about rows written *before* the schema rule was bound — `add_schema_rule`/`set_table_schema` never scan existing data. Already tracked as **task #810 (F-17)**. |
| **C2** (= crush NF-1, R1's addendum) | P2 | crush, R1 addendum | `Bin` accepted by the same gate but can never actually seek (`safe_seek_key` always `None` for `Bin`) — functionally safe (offset fallback), just a pointless/misleading acceptance. Already tracked as **task #811 (F-18)**. |
| **C3 = R2** | **P0 — NEW** | review 3 only | Bootstrap TTL is fail-open in the login path: no expiry check before proof acceptance; rotation-on-login is best-effort/non-fatal, so an expired token can still authenticate if rotation errors (storage failure, race, etc.). **Needs a new task.** |
| **C4 = R3** | **P0 — deeper than crush's NF-3** | review 3 (crush found a narrower P3 slice) | Schema DDL still isn't atomic end-to-end: `interner_mgr.persist()` failure after `save_table_meta` has **no rollback at all**; `compile_table_schema`'s live-registry mutation (register/bind) isn't undone by the EXTERNAL catalogue rollback on a later failure. Crush's NF-3 only covered the narrow "step-1-registered-but-never-bound, in-memory-only, near-zero-likelihood" leak (P3) — R3 is the broader claim that catalogue and live validator state can diverge for real. **Needs a new task; reconcile with existing #817 (F-24) rather than duplicate.** |
| **C5 = R4** | P1 (real, pre-existing, self-documented) | review 3 only | FK RESTRICT/CASCADE/SET NULL check-then-act outside tx scope, verified against the code's own doc comment. Not a hidden defect — already tracked as a "future task" in-code. Decision needed: fix now, or explicitly downgrade FK actions to "experimental" in docs until fixed. |
| **C6 = R5** | **P0 — NEW** | review 3 only | `SelectItem::Expression` accepted end-to-end (wire DTO → parser → TS public type) but silently produces no projected value — confirmed via direct code read. Classic silent-wrong-result, exactly the bug class Wave F exists to eliminate. **Needs a new task.** |
| **C7 = R6** | P1 (F-9's known residual, but review 3 wants a REAL fix, not just documentation) | `@oh` (via #813/F-20 doc-only), review 3 (wants atomic in-flight lease) | Escalates the scope of **task #813 (F-20)** — currently scoped as "just document in KNOWN_LIMITATIONS," review 3 argues this should be a real fix (atomic lease combining expiry-check + lock + removal). |
| **C8 = R7** | **P1 — broader than crush's NF-2** | review 3 (crush only found the SDK-exposure angle) | Corrupt-record diagnostics are still incomplete/non-uniform in the engine itself, not just missing from SDKs: `try_project_page_only_bytes` (`read_exec.rs:2561`) and `apply_select_value_bytes` (`read_exec.rs:2603`) convert decode errors to silent `Null`/skip instead of reporting; `table_manager_streaming.rs:319` excludes malformed rows with no diagnostic; `read_index_scan.rs`/`read_temporal.rs` never had a `corrupt_records` channel wired at all. F-10 only touched 14 sites in one function of `read_exec.rs` — this is a materially larger uncovered surface than "two sibling table-manager files," which was F-10's own stated scope-out. **Needs a new task, separate from #815 (F-22, which is SDK-exposure-only and still valid on its own).** |
| **C9 = R8** | P1 (deeper than my ktav-only task) | `@oh`/review 1 implicitly (F-11 follow-up), review 3 (wants safe-by-default + warning + metrics) | Escalates **task #814 (F-21)** beyond "add the key to server.example.ktav" — review 3 wants a finite *default* (not just documented-but-optional), a startup warning when `None`, and reserved-bytes metrics. Keep #814 as the cheap doc/config fix; a broader default-safety design is a separate, bigger task. |
| **C10 = R9** | P1/P2 (broader than `@oh`'s N-2) | `@oh` (backup() only), review 3 (also restore()'s dir-fsync-failure-still-returns-success + wording) | Escalates **task #812 (F-19)**: not just "add fsync to backup()," but also decide whether `restore()`'s existing dir-fsync failure (currently logged-only, method still returns `Ok`) should propagate in a strict mode, and whether `KNOWN_LIMITATIONS.md`'s durability language should read "best-effort" rather than implying full crash-durability. |
| **C11 = R10** | P1 — NEW, small | review 3 only | `.github/workflows/release.yml`'s changelog gate accepts `[Unreleased]` **or any** version heading — now that a real `[0.1.0-alpha.1]` section exists, it should require the EXACT `## [${TAG_VERSION}]` heading, or a future `alpha.2` tag could pass CI using alpha.1's stale notes. |
| **C12 = R11** | P2 — NEW, multi-part | review 3 only | Repo/doc hygiene: README still says "Create/Drop User/Role" (role objects were removed), README's "binaries not available yet" will go stale the moment a tag ships, `AGENTS.md` says 10 default crates vs the real 23, `KNOWN_LIMITATIONS.md` needs updates for F-15/R1/R4/R5/R6/R7/R9, ~716 tracked `docs/dev-artifacts` files (393 prompt briefs + checkpoints) hurt public-repo signal/noise, a stray local tag (`backup/pre-history-rewrite-2026-07-14`) needs to stay unpublished, untracked checkpoint/log files need excluding from any release branch. |
| **C13 = R12** | P2/perf, already documented, **no new task** | review 3 only | Cursor isn't a real engine-level iterator (full `AsOf` rescan per page) — already accurately described in `KNOWN_LIMITATIONS.md` and CHANGELOG, and review 3 itself says this is *not* urgent for alpha. |

## What this changes about the already-created task list (#810-#817)

- **#810 (F-17)** — unchanged in substance, now **three-way confirmed**. Review
  3 adds a concrete, cheap **interim** recommendation I didn't have when I
  paused to ask about scope: for alpha, the safest fix is to simply **exclude
  the schema-typed gate from keyset eligibility entirely (always fall back to
  offset)** rather than attempt the harder "validated-through-version" design
  right now — defer the harder fix post-alpha. This resolves the scope
  question I was about to ask the user about, with a concrete recommendation
  from independent analysis rather than a bare "your call."
- **#811 (F-18)** — unchanged, now three-way confirmed (crush + R1 addendum).
- **#812 (F-19)** — scope should widen per C10 (see above) before writing its
  brief.
- **#813 (F-20)** — scope should change from "just document" to "consider a
  real atomic-lease fix" per C7, at least for the cursor-reaper half; the
  F-11/server.example.ktav documentation half is unaffected.
- **#814 (F-21)** — keep as the cheap ktav fix; C9 argues for a separate,
  bigger "safe default + warning + metrics" task on top.
- **#815 (F-22)** — still valid as scoped (SDK exposure only); C8 is a
  materially bigger, separate engine-side task, not a duplicate.
- **#816 (F-23)**, **#817 (F-24)** — unaffected; #817 should note C4 as
  related-but-broader context (review 3's R3 makes a stronger, wider claim
  than the narrow leak #817 already tracks).

## Net severity picture

- **Confirmed P0 (block-worthy for a "0.1.0-alpha.1" tag), by my own
  independent verification, not just review 3's say-so:**
  - C1/R1 (schema-typed keyset gate) — three-way confirmed, task exists (#810).
  - C3/R2 (bootstrap TTL fail-open in login path) — confirmed by me, **no
    task exists yet**.
  - C6/R5 (`SelectItem::Expression` silently ignored) — confirmed by me, **no
    task exists yet**.
- **Real but pre-existing/self-documented, not a "surprise":**
  - C5/R4 (FK TOCTOU) — confirmed by me, already flagged in-code as a known,
    accepted, tracked-for-later tradeoff. Worth a release-scoping decision
    (fix now vs. explicitly mark FK actions "experimental" in public docs
    until fixed), not necessarily an emergency.
- **P0-adjacent design gap, deeper than what #813/#817 already track:**
  - C4/R3 (schema DDL atomicity — interner-persist has no rollback; live
    registry not restored by catalogue rollback).
- Everything else (C2, C7-C13) is P1/P2 scope-widening or net-new-but-lower-
  severity findings against already-tracked or easily-tracked follow-up work.

## Recommendation

Given three independent reviews (two agent-driven, one deeper static audit)
converge on "the campaign's actual fixes are sound, but the release is not
yet tag-ready," and given I have personally confirmed the two genuinely new
P0s (R2, R5) plus the pre-existing-but-real R4, I recommend treating this as
a real gate rather than optional polish. The concrete next step needs the
user's direction on scope/pace (see the question I'm about to ask), not a
unilateral decision to open 6+ more tasks and start spending agent-hours
without confirming the campaign's shape has genuinely changed from "wrap up
with a review" to "one more correctness-focused wave before tagging."
