# Documentation Accuracy 6a — fix 08-interconnect.md's stale "нет кода" status

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Context

First item of "Этап 6 — Documentation accuracy"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 09: `docs/guide-docs/guide/08-interconnect.md` claims
replication, network changefeed, and live subscriptions are all
**unimplemented roadmap items**. This is now **false** — all three are
real, tested, wire-exposed features. This is a docs-only correction; no
code changes.

## What the doc currently claims vs. reality (verified against code)

`08-interconnect.md`'s "Что не существует" table (lines 44-55) lists:

| Row | Doc's claim | Reality (verified) |
|---|---|---|
| Leader-follower репликация | ❌ Нет кода | **Implemented.** `crates/shamir-server/src/replication/` (`follower_loop.rs`, `supervisor.rs`, `source.rs`, `wire_source.rs`, `in_process.rs`, `prod_factory.rs`), wire protocol `crates/shamir-query-types/src/wire/repl.rs` (`ReplRequest::{Hello,Pull}`, `ReplResponse`, VR-style epoch fencing), dispatch at `crates/shamir-server/src/db_handler/repl_handler.rs`. Tests: `replication::tests::{follower_loop_tests,supervisor_tests}` (lib), `crates/shamir-server/tests/repl_pull_e2e.rs` (e2e). |
| Network changefeed (pull API over wire) | ❌ Нет кода | **Implemented** — this IS `ReplRequest::Pull` (`repl.rs`): pulls a batch of changelog events for one repo from `from_version`, with long-poll (`wait_ms`) support. Not a separate mechanism from replication; it's the same wire op the doc's own "Шаг 1" describes as future work. |
| Live subscriptions (server-push) | ❌ Design doc, status: PROPOSED | **Implemented.** `crates/shamir-server/src/subscriptions/` (`bridge.rs`, `registry.rs`, `target_match.rs`, `filter_eval.rs`, `reactive.rs`, `push.rs`, `decode_cache.rs`, `deliver_cache.rs`, `payload.rs`). Wire-level ops exist as `BatchOp::Subscribe(SubscribeOp)` / `BatchOp::Unsubscribe(UnsubscribeOp)` (`crates/shamir-query-types/src/batch/batch_op.rs`), plus a full publication/subscription DDL model (`CreateSubscription`/`DropSubscription`/`AlterSubscription`/`ListSubscriptions` ops, referencing `REPLICATION.md §5.5`). Tests: `subscriptions::tests::{bridge_tests,registry_tests,target_match_tests,filter_eval_tests,filter_lens_parity_tests,cache_eviction_tests,cache_depth_probe_tests,reactive_limits_tests}` — an extensive, real suite. |

Rows that are STILL accurate (do not touch these):

| Row | Status |
|---|---|
| P2P-протокол / gossip | Still ❌ — confirm no gossip/mesh code exists before leaving as-is. |
| Peer discovery | Still ❌ — same. |
| Chat-протокол | Still ❌ — same. |
| `shamir-interconnect` crate | Still ❌ — confirm it's not among the 23 workspace crates listed in this project's own `CLAUDE.md` §"Workspace" before leaving as-is. |

**Also stale, cited directly by this doc**: `08-interconnect.md` line 92
links to `docs/dev-artifacts/roadmap/LIVE_SUBSCRIPTIONS.md` as
"status: PROPOSED" — that roadmap doc's own header (line ~4) still says
`**Status:** design doc (revision 2026-06-09)`, which is equally stale
given the subscriptions module above is real and tested. Update this
doc's status line too (small, adjacent fix — don't rewrite the whole
design doc, just correct the status header to reflect that the feature
shipped, mirroring whatever phrasing convention this project uses
elsewhere for "designed then implemented" docs — check e.g. how
`docs/dev-artifacts/roadmap/TEMPORAL.md` or a similar shipped-roadmap-item
doc phrases its own status line, if one exists, for consistency).

## The task

1. Read `docs/guide-docs/guide/08-interconnect.md` in full — it has a
   strong "honest roadmap, nothing works yet" framing throughout (the
   warning banner at lines 11-15, "Это конечная цель", "Не жди P2P в
   ближайших релизах" at line 153, etc.). You need to **rebalance** this
   framing, not delete it — P2P/gossip/chat genuinely still don't exist,
   so the doc's core message ("I is the least-built floor") stays true.
   What changes is the middle ground: repl/changefeed/subscriptions have
   moved from "roadmap" to "shipped, tested, but not yet P2P/gossip".
2. Rewrite the "Что не существует" table (lines 44-55): move
   Leader-follower replication, Network changefeed, and Live subscriptions
   OUT of this table into a new "Что уже реализовано" section (or fold
   them into the existing "1. Что существует сегодня" section, extending
   it past the current Changefeed/Транспорты subsections) — your call on
   exact structure, but the reader must come away knowing these three are
   real and tested, with pointers to the actual module paths and test
   files from the table above.
3. Update the "Планируемая архитектура" section (lines 56-74) — "Шаг 1"
   (network changefeed) and "Шаг 3" (leader-follower replication) are
   described as future steps with example wire shapes; correct these to
   describe what's ACTUALLY implemented today (the real
   `ReplRequest::Pull`/`ReplResponse` shapes from `repl.rs`, not the
   illustrative `changes_since`/`next_cursor` sketch at lines 82-84, which
   doesn't match the real wire op's field names — `from_version`/`limit`/
   `wait_ms`, not `cursor`/`next_cursor`). "Шаг 2" (live subscriptions) and
   "Шаг 4" (P2P/gossip/chat) — Шаг 2 needs the same "this shipped" update
   (with pointers to `BatchOp::Subscribe`/`SubscribeOp`), Шаг 4 stays as
   pure roadmap (unchanged).
4. Update the warning banner (lines 11-15) and "Что важно знать уже
   сейчас" section (lines 148-157) to stop claiming "ни один... не имеет
   рабочего кода" — narrow the claim to P2P/gossip/chat specifically,
   which is still true.
5. Fix `docs/dev-artifacts/roadmap/LIVE_SUBSCRIPTIONS.md`'s status header
   (small, adjacent fix per the citation above).
6. Leave the P2P/gossip/peer-discovery/chat/`shamir-interconnect`-crate
   rows and the "Этап F — последний" framing (§3, lines 122-134) UNTOUCHED
   — verify these claims are still true (no gossip code, no
   `shamir-interconnect` crate) before leaving them, but do not remove or
   soften them if they check out.

## Out of scope

- Do NOT implement any P2P/gossip/peer-discovery/chat code — this is a
  documentation-only correction of what ALREADY exists in code.
- Do NOT touch `docs/dev-artifacts/roadmap/STAGES.md` or `PLAN.md` beyond
  reading them for context — this brief is scoped to
  `08-interconnect.md` (+ the one adjacent `LIVE_SUBSCRIPTIONS.md` status
  line it directly cites).
- Do NOT touch anything from the already-completed Этапы 1-5 — this brief
  is scoped to this one doc-accuracy item only.

## Verification (MANDATORY before you report done)

- No `cargo test`/`clippy`/`fmt` gate applies (docs-only) — state this
  explicitly.
- Re-verify every code citation you add (module paths, test names) by
  actually checking the files exist and contain what you claim — don't
  copy this brief's citations verbatim without confirming them yourself
  (they were checked during brief-writing, but re-confirm; the module
  structure may have shifted).
- Confirm the P2P/gossip/peer-discovery/chat/`shamir-interconnect`-crate
  claims are STILL accurate (still no code) before leaving those rows
  unchanged — search the workspace to be sure, don't assume.
- Re-read the final doc end-to-end as a new reader would — confirm the
  document's overall message is now internally consistent (no
  contradiction between an early "nothing works" banner and a later
  "these three things work" section).
