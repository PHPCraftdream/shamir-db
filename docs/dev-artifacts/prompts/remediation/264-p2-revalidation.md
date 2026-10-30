# 264 - P2 revalidation of the active findings (read-only, docs only)

## Goal

Re-decide every active card of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md` (339 cards: `confirmed-open`, `partially-fixed`, `unverified`, plus `XXS-NEW-TX-PREFIX`) against current master after the P0/P1 remediation waves, and record the result in `P2-REVALIDATION-2026-10-29.md`. No fix is started by this stage.

## Rules for every reviewer session

- Read-only: no edits, no builds, no tests, no git commands that mutate the tree or index.
- A card is closed only with the code (and preferably the commit) that removes the named mechanism; a card stays open only with a concrete reachable path in current code.
- Output ends with one fenced `jsonl` block, one object per card of the batch, so the result can be parsed and checked for exact coverage.

## Pass 1 - reviewer prompt header (verbatim), 13 sessions

P2 REVALIDATION (S.H.A.M.I.R. DB). The P0 and P1 remediation waves have been delivered (PRs #311-#405 landed on master after the previous P2 revalidation of 2026-10-24, which was made against commit b0e1ddf7). Decide, for EACH card below, whether the finding is still real on the code that is live today (this checkout is a detached HEAD of current master).

Where the cards live: docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md. Find a card with `grep -n "^### <ID>" <that file>` and read its whole section (mechanism, exact trigger/reachability, counter-evidence, file:line proof, recipe/oracle). The card text was written against an OLDER tree: file:line references are stale (code was split into subfolders by the layout campaign, many files moved). Locate the code by symbol/name with grep, never trust the old line numbers. Also read the section "Ревалидация 2026-10-24" paragraph inside the card when present.
Project rules: CLAUDE.md in the repo root (layout limits, mod.rs = re-exports only, imports at top, one file = one primary export, tests in tests/ dirs, lock-free ideology, builder-only queries).
History tools you may use (read-only): `git log --oneline b0e1ddf7..HEAD -- <path>` and `git log -S'<symbol>' --oneline`, `git show <sha> -- <path>`. A card is FIXED only when you can point at the code (and ideally the commit) that removes the named mechanism; "the file changed" is not evidence.

For EACH card decide exactly one verdict:
- STILL_OPEN: the named mechanism/obligation is present at HEAD (cite file:line of the live code).
- PARTIAL: a concrete part was fixed; say what remains.
- FIXED: the mechanism is gone (cite file:line and, when you can, the commit).
- REFUTED: the premise is false at HEAD (positive counter-evidence, cite it).
- CHANGED: the code was rewritten so the card's shape no longer fits but a related issue remains; describe it in one line.
- UNVERIFIABLE: cannot be decided statically (say what is missing).
For every card that is not FIXED/REFUTED give a fix recommendation: size S (<=1 file / <40 lines), M (a few files), L (cross-crate or design decision); kind code|test|doc|dep|decision; and a one-line recipe. Flag `owner_decision: true` when the fix changes a public contract, wire format, a dependency, or needs a policy choice. Re-grade the risk (high|medium|low|nit) if the old grade is wrong, with a short reason.

Work in risk order (high, medium, then low, nit) and NEVER skip a card: if you run short of time mark the remainder UNVERIFIABLE with reason "not reached". Be decisive; no speculation. Do not edit anything, do not build or run tests.

OUTPUT FORMAT (strict):
1. A short BATCH SUMMARY (counts per verdict, top 3 most important still-open items, groups of cards that should be fixed together in one PR, and any cross-batch dependency you noticed).
2. One compact paragraph per card (max ~6 lines): `<ID> - <VERDICT>` then evidence (file:line), recipe, size/kind.
3. At the very end ONE fenced code block tagged `jsonl` with exactly one JSON object per card of this batch (no omissions, no extras), keys: id, verdict, evidence (string, file:line or commit), size (S|M|L|-), kind (code|test|doc|dep|decision|-), fix (one line), owner_decision (true|false), risk (high|medium|low|nit), note (one short line). Use double quotes, no trailing commas, no comments inside the block.

Batches (card ids are taken from the active-mechanisms table of P2.md, grouped by crate):

- `R01-index-a`: 22 cards
- `R02-index-b`: 23 cards
- `R03-connect`: 27 cards
- `R04-storage-ipc`: 27 cards
- `R05-db-server`: 29 cards
- `R06-client-node`: 29 cards
- `R07-engine-numa`: 26 cards
- `R08-tx-sdk`: 26 cards
- `R09-wal-tcp`: 31 cards
- `R10-types-qbmacros`: 25 cards
- `R11-querytypes-builder`: 32 cards
- `R12-wasm-ws`: 27 cards
- `R13-funclib`: 15 cards

## Pass 2 - skeptical reviewer prompt header (verbatim), 6 sessions

P2 REVALIDATION, SECOND (SKEPTICAL) PASS (S.H.A.M.I.R. DB). A first-pass reviewer has already decided each card below against the code that is live today (this checkout is a detached HEAD of current master 7f04e096). The first pass may be wrong in BOTH directions: it may call a finding FIXED/REFUTED while the mechanism still exists, or call it STILL_OPEN while the code already handles it. Your job is to try to REFUTE the first-pass claim and to decide independently from the code.

Where the cards live: docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2.md. Find a card with `grep -n "^### <ID>" <that file>` and read its whole section (mechanism, exact trigger/reachability, counter-evidence, recipe/oracle). Its file:line references are stale (the layout campaign split many files; PRs #311-#405 changed a lot): locate the code by symbol name with grep, never trust old line numbers. History tools (read-only): `git log --oneline b0e1ddf7..HEAD -- <path>`, `git log -S'<symbol>' --oneline`, `git show <sha> -- <path>`.
Project rules: CLAUDE.md in the repo root.

Procedure for EACH card: (1) from the card text alone, state the invariant/mechanism that must be absent for the card to be closed; (2) look at the live code and decide your own verdict (STILL_OPEN | PARTIAL | FIXED | REFUTED | CHANGED | UNVERIFIABLE) BEFORE weighing the first-pass claim; (3) compare: AGREE or DISAGREE with the first pass; on DISAGREE explain precisely what the first pass missed, with file:line. A FIXED verdict needs the exact code that removes the mechanism AND a check that no other path reaches the same mechanism; a STILL_OPEN verdict needs a concrete reachable path in current code (not only that a symbol still exists). Also check the proposed recipe: is it sound, is the size (S <=1 file/<40 lines, M few files, L cross-crate/design) right, does it really need an owner decision (public contract, wire format, dependency, policy)?
Be decisive, no speculation, quote file:line. Do not edit anything, do not build or run tests. NEVER skip a card; if short of time mark UNVERIFIABLE with reason "not reached".

OUTPUT FORMAT (strict): 1) SECOND-PASS SUMMARY: counts of AGREE/DISAGREE, list every DISAGREE with one line. 2) One compact paragraph per card (max ~6 lines): `<ID> - <your verdict> - AGREE|DISAGREE`, evidence file:line, remark on recipe/size. 3) At the very end ONE fenced block tagged `jsonl`, exactly one JSON object per card of this batch: id, verdict2, agree (true|false), evidence2 (file:line), size2 (S|M|L|-), owner_decision2 (true|false), risk2 (high|medium|low|nit), note (one line; on disagree say what the first pass missed). Double quotes, no trailing commas.

Batches:

- `S1-fixed-changed`: 29 cards
- `S2-partial`: 32 cards
- `S3-unverif-index-storage`: 20 cards
- `S4-medium-high-rest`: 14 cards
- `S5-sample-a`: 20 cards
- `S6-sample-b`: 20 cards

Selection rule: S1 = every `FIXED` and `CHANGED` verdict of pass 1; S2 = every `PARTIAL`; S3 = every `UNVERIFIABLE` plus the `high`/`medium` `STILL_OPEN` cards of shamir-index and shamir-storage; S4 = the remaining `high`/`medium` `STILL_OPEN`; S5/S6 = a stratified random sample of the `low`/`nit` `STILL_OPEN` verdicts (about 14% per crate, seed 20261029), split in two. If the disagreement rate on the sample exceeds about 5%, the second pass is extended to all `low`/`nit` verdicts.

## Aggregation

The report is generated from the validated jsonl results (every id exactly once per pass); pass-2 values for fix size and owner-decision flag replace pass-1 values where they exist. Paths in the report are repo-relative.
