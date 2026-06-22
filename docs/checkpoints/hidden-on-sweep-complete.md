בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — Hidden-O(N) sweep complete

Date: 2026-06-22.

## Session summary

The hidden-O(N) sweep campaign (a follow-on to Op #2 + the post-Op#2
cleanup) is complete. Goal: hunt and kill the class of defect that bit the
drainer twice — a hidden O(N) cost disguised as O(1), and a spurious O(W)
triggered by a wrong predicate. Five stages, single-context main-thread,
strict TDD. Along the way a real pre-existing wire-contract regression on
origin/master was found and fixed (pagination), and its sibling, and the
whole pagination bug-class was eliminated structurally.

**The unifying theme:** don't patch instances, kill classes. Every fix
turned a recurring footgun into an invariant — the cache range-drain, the
shared `fast_path_pagination` helper + contract test, and the clippy
scc-len ban are the same move at three scales.

## Stages

| Stage | Outcome | Commit |
|---|---|---|
| 0 — overlay GC measurement | `gc_upto` O(N) cliff is REAL in code but NEVER bites: Op #2 keeps overlay depth ≤3 even on fjall under burst. Stage 1 (version-major index) DROPPED as gold-plating. | 0a9571f |
| 1 — version-major overlay index | DELETED (Stage 0 verdict: theoretical cliff) | — |
| 2 — subscription cache eviction | `DashMap::retain` O(cache) per watermark → `scc::TreeIndex` with CV-first key, `remove_range` O(evicted+log N). 6 parity tests. | 3eb7601 |
| 3 — scc len() guard | `clippy.toml` bans `scc::*::len` (it's `iter().count()`, O(N)); ~18 existing sites audited + annotated; CLAUDE.md pillar #3 note. | 0f5de6b |

## Bonus — pagination wire-contract regression (found during Stage 2 verification)

A pre-existing `@server --full` failure (`read_with_filter_order_limit`)
turned out to be a real regression on origin/master: the #128 top-K heap
`LIMIT` fast path hardcoded `pagination: None`. Its sibling — the
sorted-index `ORDER BY+LIMIT` fast path (`read_order_limit_fast`) — had the
same bug. Both were #128 LIMIT optimizations that forgot pagination.

Fixed structurally: both fast paths now route through one
`exec::fast_path_pagination` helper, and `limit_queries_all_emit_pagination_contract`
(a completeness critic over the LIMIT fast-path surface) makes future drift
impossible without tripping the test.

| Fix | Commit |
|---|---|
| #171 top-K pagination | 604cc47 |
| #172 sorted-index pagination + helper consolidation + contract test | a37c950 |

`@server --full` is green again (531/531, was 530/531 — first clean run
since the b2b1280 backend swap).

## Process note — git accident, owned and cleaned

Mid-session a stray `; git stash pop` (unconditional after a failed
`git stash push` on an untracked path) popped an unrelated OLD stash
(`stash@{0}`, mpack! WIP) and left conflict markers in 7 shamir-engine
query files. Root-caused, cleaned with one `git checkout HEAD -- <paths>`
(zero loss — the conflicted stash stays retained in `git stash list`). Two
false alarms were retracted: it was NOT pre-session corruption, and no
green gate was stale. Lesson logged: never use the unconditional
`; git stash pop` bisect pattern; the copy-to-tmp + `git checkout` idiom is
safe.

## Gate (final)

- `clippy --workspace --all-targets -- -D warnings`: clean (scc-len ban active)
- lib tests: 3646/3646
- `@oracle --full`: 1422/1422
- `@server --full`: 531/531
- fmt: clean

## Active goal / TaskList

No `/goal`. TaskList: all hidden-O(N) tasks (#166, #168, #169, #170) +
the pagination tasks (#171, #172) complete. #167 deleted (gold-plating).
Nothing pending.

## Decisions

- **Stage 1 deleted by measurement, not preference** — Stage 0 probe drove
  the gold-plating verdict (overlay depth ≤3 in every realistic scenario).
- **clippy disallowed-methods over a grep test** — type-aware, zero false
  positives, integrates with the existing gate. The annotations ARE the
  documentation.
- **Pagination fixed structurally (one helper + contract test)**, not as
  two point-patches — two instances of one bug = a structural bug.
- **Per-stage commits** — each stage / fix is a self-contained commit; the
  cross-cutting lint change is its own `chore(clippy)`.

## Open questions

- **Push.** 5 commits ahead of origin/master, unpushed: 3eb7601, 0a9571f,
  604cc47, a37c950, 0f5de6b. Plus 2 from earlier this session (92311d4,
  b68fa1c — Op #2 cleanup). Awaiting `пуш`.
- **Dangling doc.** `docs/checkpoints/2026-06-22-0145.md` (post-Op#2
  checkpoint) is still untracked from a prior turn — fold into a docs
  commit or leave.
- **Old stashes.** `git stash list` holds 3 pre-session WIP stashes
  (mpack!, cycles 9-11, test-extraction). Not mine to resolve — surface to
  the user whether to drop/apply/keep.

## Repo state

```
?? docs/checkpoints/2026-06-22-0145.md
?? docs/checkpoints/hidden-on-sweep-complete.md
```

```
0f5de6b chore(clippy): ban scc O(N) len() — hidden-O(N) sweep Stage 3
a37c950 fix(read): sorted-index ORDER BY+LIMIT must emit pagination — #128 sibling
604cc47 fix(read): top-K ORDER BY+LIMIT must emit pagination — #128 regression
0a9571f perf(tx,engine): hidden-O(N) Stage 0 — overlay GC measurement (cliff is theoretical)
3eb7601 perf(subscriptions): decode/deliver cache eviction → TreeIndex range-drain
```
