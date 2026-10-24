# P0–P4 revalidation, cycle 2: skeptical second opinion (2026-10-24)

## Goal

Re-decide every assigned review card of `docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/P<n>.md`
against the CURRENT source of this checkout (detached `origin/master`), and emit one ledger row per card.
This is a read-only claim revalidation, not a new audit and not a fix.

## Context

- The P files consolidate the independent cycle-2 revalidation of 2026-10-08, frozen at source snapshot
  `e3765c935fc71655ee1ec0160cf180607935d89b` (call it BASE). P is the plan priority (P0 most urgent),
  not severity. Each card is a `### <ID> — <title>` section with a **Verdict**, scoped risk, origin links,
  trigger/consequence, direction and **Current file:line evidence** links (those links are BASE line numbers).
- About 240 commits landed on master after BASE. Many are fixes that may close cards (transaction
  integrity B02–B14, snapshot isolation SNAP-*, limits W*, replication S*/S5c-X/SEED, visibility VIS-*,
  dependency/advisory bumps, native-code removal), and many are pure layout splits that MOVED code into new
  files/folders without behaviour change (`refactor(...): split ... to the layout limits`, engine stages
  B1–F). A path missing at HEAD usually means the code moved, not that the defect is gone: find where the
  symbol lives now (grep the function/type name; `git log BASE..HEAD -- <old path>` names the moving commit).
- `git log --oneline e3765c93..HEAD` lists the candidate fixing commits; read their diffs with the git read
  tool when a card's code changed.


## Cycle 2 specifics

A first pass already produced a verdict per assigned card (shown under each card as FIRST-PASS). Your job is
an INDEPENDENT second opinion that tries to break it:

- For FIRST-PASS `confirmed-open`: assume it may have been closed by one of the ~240 commits after BASE.
  Take the key symbols/functions of the mechanism, search HEAD for them and read their history
  (`git log --oneline e3765c93..HEAD -- <current path>` / `-S<symbol>` through the git read tool) and read the
  diffs of anything touching the mechanism. Only keep `confirmed-open` if the exact trigger still works at HEAD
  after reading the current guard/caller code. Common closers: transaction/commit-tail and visibility work
  (B02–B14, SNAP-*, VIS-*), limits (W*), replication/follower (S*, S5c-X, SEED-*), shutdown/durability,
  wasm compile cache and private-dir work, dependency bumps.
- For FIRST-PASS `fixed` / `partially-fixed` / `refuted` / `not-applicable`: verify the cited commit really
  removes the named trigger (read the diff), and that the remainder (partially-fixed) is stated exactly. A
  fix that only moves code or adds a test does not close a defect.
- Keep the first-pass verdict only when you could not refute it; say `AGREE` or `CHANGE` at the start of the
  reason. If you change a verdict, the reason must name the evidence that the first pass missed.

## Per card

1. Read the whole card in the P file (line range given in the assignment).
2. Locate the current code of every mechanism it names; read the callers/guards needed to decide.
3. Decide the CURRENT status with the existing vocabulary:
   - `confirmed-open` — the mechanism/obligation still exists at HEAD in the stated scope;
   - `fixed` — the named mechanism is removed/guarded at HEAD (name the commit);
   - `partially-fixed` — say which part remains;
   - `refuted` — positive counter-evidence at HEAD that the claim is wrong (including if the prior verdict was wrong);
   - `not-applicable` — intentional policy/ownership/removed feature, no violated obligation;
   - `unverified` — only with the exact missing premise named; never a polite "safe".
   Prior closed verdicts (`fixed`/`refuted`/`not-applicable`) are checked for regression and for whether the
   reasoning still holds; keep them if they hold.
4. Re-assess risk only if the evidence changed it (`high`/`medium`/`low`), otherwise copy it.
5. Evidence is CURRENT `path:line` at HEAD (one or two strongest locations), plus the commit (short sha and
   PR number from its subject) that changed the outcome, or `none`.

Passing tests, a test file's existence, or a green CI label are not proof of a fix. Be decisive and
specific; one sentence of reasoning per card.

## Rules

- READ-ONLY. Do not create, edit or delete any file. ⛔ NEVER run `git reset` / `checkout` / `clean` /
  `stash` / `restore` / `rm`, or any git command that mutates the working tree or index.
- Never compile, build, run tests, benches, clippy or the project: all verification happens in GitHub CI.
- No network access; no sub-agents.
- Time budget: emit the final ledger by 75 minutes at the latest; cards you could not decide in time go to
  `NOT-DONE` (they will be re-queued), never guessed.

## Final message format (mandatory, exact)

The final message contains exactly one ledger block; every assigned ID appears exactly once, either as a row
or in `NOT-DONE`:

```
<<<LEDGER
| ID | prior | now | risk | evidence at HEAD | changed by | reason |
| RV-EXAMPLE-0000ABCD | confirmed-open | fixed | medium | crates/x/src/y.rs:120 | abc1234 (#97) | The guard added in ... closes the trigger. |
LEDGER>>>
COVERED: <rows>/<assigned>
NOT-DONE: <space-separated IDs or none>
```

Table cells must not contain `|` or line breaks. After the block, at most 10 lines of cross-card notes
(for example a fix that closes several cards, or a new defect observed while reading, marked `NEW:`).
