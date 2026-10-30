# 265 - P3 and P4 revalidation of the active findings (read-only, docs only)

## Goal

Re-decide every active card of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3.md` (340 cards, ids `L3-*`) and `P4.md` (169 cards, ids `L4-*`) against current master `96d694f5` after the P0, P1 and P2 remediation waves, and record the result in `P3-REVALIDATION-2026-10-30.md` and `P4-REVALIDATION-2026-10-30.md`. Active = status `confirmed-open`, `partially-fixed` or `unverified` in the "Активные / partial / unverified facets" table (509 cards in total); the cards already `fixed` / `refuted` / `not-applicable` and the higher-risk-only redirects are out of scope. No fix is started by this stage. Same method as brief 264 (P2): two passes, read-only reviewer sessions, strict jsonl output, exact coverage check.

## Rules for every reviewer session

- Read-only: no edits, no builds, no tests, no git commands that mutate the tree or index.
- A card is closed only with the code (and preferably the commit) that removes the named mechanism; a card stays open only with a concrete reachable path in current code.
- Output ends with one fenced `jsonl` block, one object per card of the batch, so the result can be parsed and checked for exact coverage.
- At most 5 sessions run at the same time.

## Pass 1 - reviewer prompt header (verbatim), 19 sessions

P3/P4 REVALIDATION (S.H.A.M.I.R. DB). The P0, P1 and P2 remediation waves have been delivered after the previous revalidation of P3 and P4 (2026-10-24, made against commit b0e1ddf7): PRs #311-#405 (P0/P1), the P2 sprint (PR #406, ONE squash commit aaa22258 that fixed most P2 cards; its report lists every P2 card that was NOT completed) and the CI trigger PR #407. Decide, for EACH card below, whether the finding is still real on the code that is live today (this checkout is a detached HEAD of current master 96d694f5).

Where the cards live: docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3.md (ids starting L3-) and P4.md (ids starting L4-). Find a card with `grep -n "^### <ID>" <that file>` and read its whole section (mechanism, exact trigger/reachability, counter-evidence, file:line proof, recipe/oracle). The card text was written against an OLDER tree: file:line references are stale (code was split into subfolders by the layout campaign, many files moved, and the P2 sprint rewrote a lot). Locate the code by symbol/name with grep, never trust the old line numbers. Also read the section "Ревалидация 2026-10-24" paragraph inside the card when present.
Most P3/P4 cards are FACETS of a P0/P1/P2 mechanism (the "Accepted canon" link in the card). The parent may have been fixed since; read it in P0.md/P1.md/P2.md and, for P2 parents, in docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P2-REVALIDATION-2026-10-29.md and P2-SPRINT-REPORT-2026-10-30.md (every P2 card not listed there as partial/deferred/skipped/not done was implemented in aaa22258). A fixed parent does NOT close the facet by itself: decide the facet's own mechanism from the live code.
Project rules: CLAUDE.md in the repo root (layout limits, mod.rs = re-exports only, imports at top, one file = one primary export, tests in tests/ dirs, lock-free ideology, builder-only queries).
History tools you may use (read-only): `git log --oneline b0e1ddf7..HEAD -- <path>` and `git log -S'<symbol>' --oneline`, `git show <sha> -- <path>` (the P2 sprint is the single commit aaa22258). A card is FIXED only when you can point at the code (and ideally the commit) that removes the named mechanism; "the file changed" is not evidence.

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


Batches (cards are sorted by crate, then risk; the card list of each batch is appended to the header at launch):

- `B01`: 27 cards (20 P3 + 7 P4) - shamir-connect
- `B02`: 27 cards (8 P3 + 19 P4) - shamir-connect, shamir-server, shamir-client
- `B03`: 27 cards (21 P3 + 6 P4) - shamir-client
- `B04`: 27 cards (18 P3 + 9 P4) - shamir-client, shamir-client-node, shamir-transport-tcp
- `B05`: 27 cards (10 P3 + 17 P4) - shamir-transport-tcp, shamir-transport-ws, shamir-transport-ipc
- `B06`: 27 cards (19 P3 + 8 P4) - shamir-transport-ipc, shamir-storage
- `B07`: 27 cards (20 P3 + 7 P4) - shamir-storage, shamir-wal
- `B08`: 27 cards (15 P3 + 12 P4) - shamir-wal, shamir-tx, shamir-engine
- `B09`: 27 cards (21 P3 + 6 P4) - shamir-engine, shamir-db
- `B10`: 27 cards (21 P3 + 6 P4) - shamir-db, shamir-index
- `B11`: 27 cards (18 P3 + 9 P4) - shamir-index, shamir-types
- `B12`: 27 cards (15 P3 + 12 P4) - shamir-types, shamir-collections
- `B13`: 27 cards (26 P3 + 1 P4) - shamir-tunables, shamir-query-types
- `B14`: 27 cards (19 P3 + 8 P4) - shamir-query-types, shamir-query-builder
- `B15`: 27 cards (20 P3 + 7 P4) - shamir-query-builder-macros, shamir-funclib
- `B16`: 26 cards (12 P3 + 14 P4) - shamir-funclib, shamir-wasm-host
- `B17`: 26 cards (16 P3 + 10 P4) - shamir-wasm-host, shamir-sdk
- `B18`: 26 cards (22 P3 + 4 P4) - shamir-sdk, shamir-sdk-macros, shamir-numa
- `B19`: 26 cards (19 P3 + 7 P4) - shamir-numa, shamir-bench-utils

## Pass 2 - skeptical reviewer prompt header (verbatim)

P3/P4 REVALIDATION, SECOND (SKEPTICAL) PASS (S.H.A.M.I.R. DB). A first-pass reviewer has already decided each card below against the code that is live today (this checkout is a detached HEAD of current master 96d694f5). The first pass may be wrong in BOTH directions: it may call a finding FIXED/REFUTED while the mechanism still exists, or call it STILL_OPEN while the code already handles it. Your job is to try to REFUTE the first-pass claim and to decide independently from the code.

Where the cards live: docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P3.md (ids L3-) and P4.md (ids L4-). Find a card with `grep -n "^### <ID>" <that file>` and read its whole section (mechanism, exact trigger/reachability, counter-evidence, recipe/oracle). Its file:line references are stale (the layout campaign split many files; PRs #311-#405 and the P2 sprint commit aaa22258 changed a lot): locate the code by symbol name with grep, never trust old line numbers. Most cards are facets of a P0/P1/P2 mechanism: a fixed parent does NOT close the facet by itself. History tools (read-only): `git log --oneline b0e1ddf7..HEAD -- <path>`, `git log -S'<symbol>' --oneline`, `git show <sha> -- <path>`.
Project rules: CLAUDE.md in the repo root.

Procedure for EACH card: (1) from the card text alone, state the invariant/mechanism that must be absent for the card to be closed; (2) look at the live code and decide your own verdict (STILL_OPEN | PARTIAL | FIXED | REFUTED | CHANGED | UNVERIFIABLE) BEFORE weighing the first-pass claim; (3) compare: AGREE or DISAGREE with the first pass; on DISAGREE explain precisely what the first pass missed, with file:line. A FIXED verdict needs the exact code that removes the mechanism AND a check that no other path reaches the same mechanism; a STILL_OPEN verdict needs a concrete reachable path in current code (not only that a symbol still exists). Also check the proposed recipe: is it sound, is the size (S <=1 file/<40 lines, M few files, L cross-crate/design) right, does it really need an owner decision (public contract, wire format, dependency, policy)?
Be decisive, no speculation, quote file:line. Do not edit anything, do not build or run tests. NEVER skip a card; if short of time mark UNVERIFIABLE with reason "not reached".

OUTPUT FORMAT (strict): 1) SECOND-PASS SUMMARY: counts of AGREE/DISAGREE, list every DISAGREE with one line. 2) One compact paragraph per card (max ~6 lines): `<ID> - <your verdict> - AGREE|DISAGREE`, evidence file:line, remark on recipe/size. 3) At the very end ONE fenced block tagged `jsonl`, exactly one JSON object per card of this batch: id, verdict2, agree (true|false), evidence2 (file:line), size2 (S|M|L|-), owner_decision2 (true|false), risk2 (high|medium|low|nit), note (one line; on disagree say what the first pass missed). Double quotes, no trailing commas.

First-pass claims (id | first verdict | first risk | first evidence | first recipe | first note):

Selection rule: every `FIXED` and `CHANGED` verdict of pass 1; every `PARTIAL`; every `UNVERIFIABLE`; every `high` / `medium` `STILL_OPEN`; and a stratified random sample of the `low` / `nit` `STILL_OPEN` verdicts (about 14% per crate, seed 20261030). Sessions of about 20-25 cards each. If the disagreement rate on the sample exceeds 10%, widen the sample.

## Aggregation

The reports are generated from the validated jsonl results (every id exactly once per pass); pass-2 values for fix size, risk and owner-decision flag replace pass-1 values where they exist. Paths in the reports are repo-relative.
