# Brief for F-28 Step 6 (#833, P2) — document the final FK atomicity state

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context — this is the LAST step of a 6-step campaign (F-28, #821)

All 5 implementation steps are already landed on `master`:

- **Step 1** (#828, commit `02464f12`): inverted control on the implicit-tx
  path (`RepoInstance::begin_implicit_batch_tx`/`commit_implicit_batch_tx`),
  fixing D2 — autocommit inserts/updates into an FK-constrained table were
  being wrongly rejected because the resolver was passed as `None`.
- **Step 2** (#829, commit `1b629584`): threaded `tx: &TxContext` through
  the reverse-FK probe/plan functions (`fk_restrict.rs`, `fk_actions.rs`,
  `fk_on_update.rs`), switching their reads to `list_stream_tx`/
  `read_one_tx_bytes(Some(tx))` — fixing D1, a deterministic bug where a
  transactional `[delete child; delete parent]` batch under RESTRICT was
  wrongly rejected, and CASCADE could silently orphan a child inserted
  earlier in the same transaction.
- **Step 3** (#830, commit `0de003c2`): a spike deciding the cross-
  transaction race-closure mechanism — recommended and adopted S3-C
  (targeted Serializable isolation + SSI footprint widening) over S3-A (a
  per-table barrier lock). Decision memo:
  `docs/dev-artifacts/research/f28-s3-mechanism-decision.md`.
- **Step 4** (#831, commit `7496f207`): built `FkReverseCache`
  (`crates/shamir-engine/src/repo/fk_reverse_cache.rs`) — a cached,
  O(1)-lookup per-repo reverse-FK map replacing the O(tables) scan the
  discovery functions used to repeat on every delete/cascade-recursion-
  level, plus the O(1) role flags (`is_fk_parent_with_action`, `is_fk_child`)
  Step 5 needed.
- **Step 5** (#832, commit `6800aa0e`): implemented S3-C — a
  `TxContext.footprint_tokens` widening on `build_footprint_from_tx`, a
  footprint/publish ordering fix on the opt-in AsyncIndex commit path, and
  the real production wiring (Serializable-isolation upgrade at implicit
  delete/update-begin time for an FK-parent table; `require_footprint_for`
  at insert/update-staging time for an FK-child table), with a bounded
  retry (`retry_on_tx_conflict`) and a fix for a subtle "never-yet-interned
  FK field skips the scan (and its SSI predicate)" gap. Proven via
  deterministic end-to-end race-closure tests
  (`crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs`).

**Net result**: the original documented TOCTOU in `fk_restrict.rs` (a
concurrent insert into a child table between the RESTRICT check and the
parent delete's commit could create a dangling reference) is now CLOSED —
both the in-transaction read-your-own-writes gap (Step 2) and the
cross-transaction race (Step 5) are addressed. Two additional bugs found
along the way (D1, D2 — see Steps 1/2 above) were also fixed, independent
of the originally-suspected TOCTOU.

**Residual scope** (state this honestly, do not overclaim): the fix is
scoped to the IMPLICIT (autocommit) delete/update path — an EXPLICIT
transaction that the caller opens as `Snapshot` for its own reasons does
not get an automatic Serializable upgrade; a caller wanting the same
protection for an explicit tx opens it `Serializable` itself, and the same
footprint/predicate machinery already protects that case once wired.

## What to do

Read `docs/guide-docs/KNOWN_LIMITATIONS.md` §2 "Schemas" (search for
"`foreign_key`: single field, same-repo target only" — the existing bullet
at ~line 78-86, which currently ends with "No composite FK, no deferred
constraints, and no self-referential cascade exist either" and makes NO
mention of the TOCTOU or its closure) and the adjacent F-24 bullet (~line
91+, for the established style/voice this file already uses for a
"CLOSED (F-NN, #NNN)" entry).

Add ONE new bullet (or extend the existing `foreign_key` bullet if that
reads more naturally — use judgment, but don't duplicate content) that:

1. States the ORIGINAL problem precisely (the documented TOCTOU: a
   concurrent insert into a child table between the RESTRICT/CASCADE/SET
   NULL/ON UPDATE check and the triggering operation's commit).
2. States it is now CLOSED, citing F-28 (#821) and its 5 steps (#828-#832)
   by number, briefly describing the mechanism (S3-C: targeted
   Serializable isolation + SSI footprint widening, decided via the
   Step 3 spike memo — link/cite
   `docs/dev-artifacts/research/f28-s3-mechanism-decision.md`).
3. Mentions the two ADDITIONAL bugs found and fixed along the way (D1: an
   in-transaction read-your-own-writes gap; D2: autocommit FK inserts
   being wrongly rejected due to a `None` resolver) — briefly, since they
   are now closed and this is a historical record, not open scope.
4. States the residual scope precisely: closed for the implicit/autocommit
   path; an explicit tx that stays Snapshot by the caller's own choice is
   unaffected unless the caller opens it Serializable itself.
5. Cross-references the deterministic proof:
   `crates/shamir-engine/src/query/batch/tests/fk_race_closure_tests.rs`.

Also check: does §1 "Transactions" (or wherever this file discusses
Serializable/SSI guarantees generally) need a cross-reference to this new
FK-specific bullet, or vice versa? Only add a cross-reference if it reads
naturally — don't force one.

Finally, once this doc update lands, mark the F-28 umbrella task complete
(this is the last step) — note in your summary that #821 should now be
closed by the orchestrator.

## Constraints

- Doc-only change — no source files.
- Match this file's existing voice/style exactly (see the F-24 bullet and
  the corrupt-records bullet nearby for the established "CLOSED (F-NN,
  #NNN)" pattern this campaign already uses throughout this file).
- Do not invent new residual limitations beyond what's stated above — if
  your own reading of the Step 5 diff surfaces something NOT covered by
  this brief's "residual scope" section, flag it in your summary rather
  than silently adding new claims to the doc.

## Verification the orchestrator will run

Read-through for accuracy against the actual Step 1-5 diffs (already
reviewed in full by the orchestrator during those steps) — no test gate
beyond confirming the file is still valid markdown.
