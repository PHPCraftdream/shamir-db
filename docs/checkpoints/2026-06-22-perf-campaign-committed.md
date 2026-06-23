בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-22 (perf campaign implemented + committed; awaiting push)

## Session summary

Resumed from `2026-06-22-0500`, then ran a full /babygoal perf-implementation
campaign driven by /crush sub-agents (one crush session per finding, sequential,
under /opti measure-first). The 10 ROI-ranked findings from the earlier oxx
perf-hunt workflow were decomposed into 7 implementation tasks (#179-#185;
#6/#8/#9 deferred as M-effort low-value). All 7 are DONE, verified by me under
strict zero-trust review, and committed as 5 atomic commits. babysit cron
(35447d7a) was armed at the start and cancelled at the end (TaskList empty).

The 7 findings, each independently gate-green then committed:
- #1 `$in @ref` semi-join O(N²)→O(N): materialize ref column once into a TSet
  cached on the FilterNode via lock-free OnceLock (keyed by value index), O(1)
  coercion-aware probe. ~13-19× at N=10k, curve linearized.
- #4 defer value_qv decode to DeliverMode::Records (both delivery paths).
- #2 compile subscription filter once at subscribe; growth-safe via a NEW O(1)
  monotonic Interner::generation() (atomic load, not len()=64 shard locks),
  recompile-on-growth. \$in N=100 −28%, N=50 −18%.
- #3 in-memory tickets-invalid-before cache (scc::HashMap<[u8;16],AtomicU64>)
  warmed at open(), updated at the single read_modify_write choke point.
  SECURITY: enumeration of all durable bump sites independently verified
  exhaustive (no delete path); revocation-survives-restart test added.
- #5 elide WAL deep-clone (begin_grouped borrows the Arc). Byte-identical WAL.
- #7 single-pass phantom-predicate validation (O(P×W)→O(W), one shared EBR
  Guard), both pre_commit sites share predicate_conflicts_batch.
- #10 apply_distinct_qv Vec<bool> keep-mask, drop redundant FxHashSet<usize>.
  all-unique N=1000 −10.8%.

Zero-trust review caught defects in 3 of 7 crush passes before acceptance:
#1 (added std::sync::Mutex on the hot path + silently changed Int↔F64 coercion
→ re-delegated to OnceLock + restored coercion); #4 (missed a sibling decode on
the second delivery path → re-delegated to gate both); #2 (crush died on a
network stall mid-impl leaving non-compiling half-wired code + used len() as the
growth detector → re-delegated to finish + switch to generation()). This is the
measure-first + verify-everything discipline working as intended.

Final holistic gate (all 5 commits together): fmt --all=0, clippy --workspace
--all-targets -D warnings=0, lib 3662/3662; per-finding @oracle --full (1436),
@server --full (535), 12/12 user_directory integration all green earlier.

Bench wins (honest, /opti before/after): real multiplicative — #1 (13-19×,
scales with data), #2 (−18..−28% on Regex/\$in fan-out). Modest — #4 (−6% fan),
#10 (−11% on distinct fn). NOT bench-visible (waste removed on uncovered paths,
stated honestly, not sold as measured) — #3 (no macro-bench path; ~1-3µs→~10-20ns
analytical), #5 (fsync-bound, in the noise), #7 (Serializable-only, no bench).

Nothing in flight. No /loop or babysit timers. TaskList empty.

## Active goal

None (`/goal` not set). babysit cron cancelled. TaskList empty.

## TaskList

Empty. All 7 implementation tasks (#179-#185) completed + the list cleared.
#6/#8/#9 from the perf-hunt roadmap were deliberately NOT created (deferred:
M-effort, low-value — re-evaluate only if a profile flags them).

## Decisions

- **Implemented 7 of 10 findings via sequential /crush sub-agents** (one session
  per finding) under /opti; deferred #6/#8/#9 (M-effort, ~1.02-1.3× low-value).
- **Committed as 5 atomic commits, not 7** — two finding-pairs (#2+#4 share
  bridge.rs; #5+#7 share pre_commit.rs) genuinely share a file, so combining each
  pair into one well-described commit is honester than risky hunk-splitting. Each
  finding was independently gate-green at task close, so each commit is valid.
- **Zero-trust review is mandatory on every crush pass** — it caught real defects
  in 3/7 (hot-path Mutex, silent semantics change, missed sibling, network-death
  half-wire, suboptimal growth signal). Never accept the agent's "exit 0" report
  without reading the diff + re-running the gate.
- **#3 security: independently verified the bump-site enumeration is exhaustive**
  (all durable writes via read_modify_write/insert_user, no delete path) before
  accepting the cache as authoritative.
- **Did NOT push** — awaiting explicit "пуш". Did NOT auto-commit the perf-hunt
  roadmap checkpoint (checkpoint-skill convention).

## Open questions

- **Push?** 5 perf commits ahead of origin/master (+ the 2 prior VersionWindow
  commits already there from earlier). Awaiting "пуш" — pre-push hook re-runs the
  gate.
- **Commit the roadmap doc?** `docs/checkpoints/2026-06-22-perf-hunt-roadmap.md`
  is untracked (durable knowledge: the 10-finding ROI roadmap). Offer to fold
  into a `docs(perf):` commit if wanted.
- **Macro-bench sweep?** Offered to run general end-to-end benches
  (read_path_matrix, db_handler_rps, select_pipeline) to show aggregate campaign
  impact + confirm no regressions. Not yet run — awaiting the user's go.

## Repo state

```
?? docs/checkpoints/2026-06-22-perf-hunt-roadmap.md
```

```
e145489 perf(query): apply_distinct_qv keep-mask — drop redundant index set
cfd83f7 perf(tx): elide WAL deep-clone + single-pass phantom-predicate validation
8f1ce70 perf(auth): in-memory tickets-invalid-before cache — kill per-request fjall+msgpack
093ef3e perf(subscriptions): compile filter once at subscribe + defer value_qv decode
111baae perf(query): $in @ref semi-join O(N²)→O(N) — materialize ref column once
```

5 commits ahead of origin/master, nothing pushed. One untracked docs file
(perf-hunt roadmap). Working tree otherwise clean. babysit cancelled.
