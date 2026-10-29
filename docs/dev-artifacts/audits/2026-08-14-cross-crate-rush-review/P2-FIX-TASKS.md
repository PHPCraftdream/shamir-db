# P2 fix sprint - execution board

Companion of [P2-REVALIDATION-2026-10-29.md](P2-REVALIDATION-2026-10-29.md) (verified verdicts, evidence and recipes per card at master `7f04e096`) and [P2.md](P2.md) (the original cards). Every active P2 card (306) belongs to exactly one task below; the 8 statically undecidable cards belong to P2-U01. Status: `[ ]` todo, `[~]` executor running, `[r]` in review, `[c]` in CI, `[x]` merged, `[-]` dropped after re-check.

## Managing the sprint

- This file is the source of truth for the sprint. Session tasks are labelled `[P2-NN]` and are only pointers; the executor briefs in `docs/dev-artifacts/prompts/remediation/p2/` carry the full per-card detail. Update the row and the tracking block of a task in the same step that changes its state, so nothing depends on chat memory.
- Gate: nothing in this sprint starts until the owner gives the go-ahead. The owner has so far asked only for verification and recording; no fix has been started.
- Before launching, reviewing or merging a task: read its section here (wave, `after`, shared files, acceptance), its brief, and the card text in [P2.md](P2.md) (`grep -n "^### <ID>"`).
- Owner-decision cards (OD) are collected in "Owner questions" below and asked in one round per wave before the wave starts. The answers go into the "Decisions" section of the brief and into the decision log. A task never waits for an unanswered card: the card is skipped and stays open.
- After a task is merged: re-decide its cards with one read-only reviewer pass, update the report table, tick the box, and remove its worktree and branch.

## Rules for every task

- Own worktree and branch (`worktrees/p2-NN-<slug>`, branch `fix/p2-NN-<slug>`); the brief is committed before the executor is launched.
- Compilation, builds and tests run only on GitHub CI (in the fork rmyva/shamir-db, then landed upstream). Locally only `cargo fmt -p <crate> -- --check`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo check` of dependents when a public signature changed, and `python -I scripts/ci/check_layout.py`. Every brief repeats this verbatim, including the ban on git mutation by the executor.
- Every behavioral fix ships with a regression test that fails on the old code (named after the card id, red first in CI). Documentation-only cards need no test.
- No new dependency, no `#[allow]`, no visibility widening beyond the card, layout limits (7 entries per folder, 1000 lines per file), `mod.rs` re-exports only, imports at the top, tests in `tests/` dirs, multi-line JSON literals in tests.
- A card found already fixed or wrong is reported with evidence, not forced.

## Pipeline per task

1. Brief is final and the task's owner questions are answered (or knowingly skipped). Record it in the tracking block.
2. Executor (rush, own worktree) with the brief; no builds or tests. Record branch and session.
3. Orchestrator reads the diff: scope, layout, rules, tests that fail on the old code, no silent behavior change. Local gates are re-run on the touched crates.
4. Independent read-only reviewer on the branch (verdict first; only a concrete reachable defect or compile error blocks). Follow-up rounds until PASS or PASS_WITH_NOTES.
5. Merge the accepted branches of the wave into one integration branch, push it to the fork, open a non-draft PR there and wait for the full CI run (one run per wave).
6. A failing test or flake is read from the CI log, fixed in code and re-pushed (never reproduced locally, never retried until green).
7. Land upstream with the manual-status procedure (below), clean up worktrees and branches, tick the box, run the post-merge re-decide.

## Waves and order

One wave is one integration branch and one full CI run. A task starts only when everything in its `after` column is merged; file overlaps are listed so the integration merge is not a surprise.

| Wave | Tasks | Why |
|---|---|---|
| 1 | P2-01, P2-02, P2-03, P2-05, P2-08, P2-09, P2-11 | Highest risk and the hub crates first; pairwise disjoint files |
| 2 | P2-04, P2-06, P2-12, P2-15, P2-16, P2-18, P2-19 | Depend on wave 1 (shared files): engine after tx, types after db/storage/client, net after client, infra after storage |
| 3 | P2-07, P2-10, P2-13, P2-17 | Depend on waves 1-2: index runtime after fts and types, connect after client and net, query-types after engine, sdk after wasm-host |
| 4 | P2-14 | Query-builder after engine and sdk |

Overlaps between tasks of the same wave (only P2-04 / P2-12 on types/value.rs) are small and are resolved in the integration merge.

P2-U01 (undecidable cards) can run at any time because it needs CI evidence, not code edits.

## Shared files between tasks (conflict matrix)

Derived from the evidence paths of the cards at the verification commit; an executor may touch more files, so treat this as a floor.

| Tasks | Shared files (cards) |
|---|---|
| P2-01 / P2-07 | `shamir-types/src/core/interner/interner.rs` (P2-01: 39178D2D; P2-07: 002DDDA6) |
| P2-01 / P2-12 | `shamir-db/src/shamir_db/shamir_db/access/authorize.rs` (P2-01: 01C2041B, 2C736C9E; P2-12: 71AFA12F, A7B1FCEC); `shamir-types/src/core/interner/interner.rs` (P2-01: 39178D2D; P2-12: 2A360D16, 973FEEFD); `shamir-types/src/entries/access/policy.rs` (P2-01: 01C2041B; P2-12: 71AFA12F) |
| P2-01 / P2-16 | `shamir-db/src/shamir_db/shamir_db/core.rs` (P2-01: 3EB43AC7, 7C59E01B, D5441CCE; P2-16: HOST-4669FDA6) |
| P2-04 / P2-12 | `shamir-types/src/types/value.rs` (P2-04: 95ECC2FE; P2-12: 313627EB) |
| P2-04 / P2-13 | `shamir-engine/src/query/batch/control/authorized.rs` (P2-04: 3425FDE6; P2-13: TYPES-0A0D9092) |
| P2-04 / P2-14 | `shamir-engine-query/src/query/filter/evaluation/filter_node.rs` (P2-04: 7CA0C179; P2-14: BUILDER-320AD085); `shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs` (P2-04: 62C6F2B1, 6CCD199D; P2-14: BUILDER-FE2003D1) |
| P2-06 / P2-07 | `shamir-index/src/fts/tokenizer.rs` (P2-06: 0695AADA, 53CEF405, 605EEC5D; P2-07: 32C98E9F) |
| P2-07 / P2-12 | `shamir-types/src/core/interner/interner.rs` (P2-07: 002DDDA6; P2-12: 2A360D16, 973FEEFD) |
| P2-08 / P2-12 | `shamir-types/src/types/ids/record_id.rs` (P2-08: 38F208C4, 9794FD53; P2-12: FEB0B5E4) |
| P2-08 / P2-19 | `shamir-collections/src/lib.rs` (P2-08: 8FEF1C5A; P2-19: 4ED3931B) |
| P2-09 / P2-12 | `shamir-types/src/codecs/interned/messagepack.rs` (P2-09: 9DEFDE43; P2-12: E82C6BE1, F070E432) |
| P2-09 / P2-18 | `shamir-client/src/client/api.rs` (P2-09: 17C0D288, 267BD36E, 3D598780, 66E9A5FC; P2-18: TCP-79910BA5); `shamir-client/src/client/transport.rs` (P2-09: 0157DABC, 153A118E, 56E4E99D; P2-18: 8999F6B7) |
| P2-10 / P2-18 | `shamir-connect/src/client/handshake.rs` (P2-10: E4631CD9; P2-18: E8E1D11B, TCP-5CE7BA5B); `shamir-connect/src/common/primitives/domain_tags.rs` (P2-10: 74705EAB, E6CF11C8; P2-18: TCP-EC4F02F5) |
| P2-14 / P2-17 | `shamir-sdk/src/api/db.rs` (P2-14: BUILDER-9E172C69; P2-17: 8BC846C6, MACROS-5D1F61B0) |
| P2-16 / P2-17 | `shamir-wasm-host/src/wasm/wasm_function.rs` (P2-16: HOST-98551A44, HOST-C5833F4E; P2-17: 723F5FD2) |

## Board

| Box | ID | Slug | Wave | After | Session | Risk | Cards | Effort | OD | Branch | PR / fork run | Reviewer | Landed |
|---|---|---|---:|---|---|---|---:|---:|---:|---|---|---|---|
| [ ] | P2-01 | db | 1 | - | #1667 | high:1 medium:2 low:8 nit:7 | 18 | 38 | 5 | - | - | - | - |
| [ ] | P2-02 | tx | 1 | - | #1668 | high:1 medium:1 low:10 nit:6 | 18 | 44 | 4 | - | - | - | - |
| [ ] | P2-03 | client-node | 1 | - | #1669 | high:1 medium:2 low:3 nit:3 | 9 | 19 | 3 | - | - | - | - |
| [ ] | P2-04 | engine | 2 | P2-02 | #1670 | medium:1 low:7 nit:5 | 13 | 32 | 1 | - | - | - | - |
| [ ] | P2-05 | index-vector | 1 | - | #1671 | medium:5 low:4 nit:4 | 13 | 26 | 3 | - | - | - | - |
| [ ] | P2-06 | index-fts-hash | 2 | - | #1672 | medium:4 low:5 | 9 | 28 | 6 | - | - | - | - |
| [ ] | P2-07 | index-runtime | 3 | P2-06, P2-12 | #1673 | medium:2 low:4 nit:7 | 13 | 24 | 2 | - | - | - | - |
| [ ] | P2-08 | storage | 1 | - | #1674 | medium:1 low:11 nit:9 | 21 | 43 | 7 | - | - | - | - |
| [ ] | P2-09 | client | 1 | - | #1675 | medium:2 low:8 nit:6 | 16 | 33 | 2 | - | - | - | - |
| [ ] | P2-10 | connect | 3 | P2-09, P2-18 | #1676 | low:10 nit:16 | 26 | 45 | 9 | - | - | - | - |
| [ ] | P2-11 | wal | 1 | - | #1677 | low:7 nit:8 | 15 | 28 | 5 | - | - | - | - |
| [ ] | P2-12 | types | 2 | P2-01, P2-08, P2-09 | #1678 | medium:1 low:8 nit:5 | 14 | 28 | 4 | - | - | - | - |
| [ ] | P2-13 | query-types | 3 | P2-04 | #1679 | low:10 nit:8 | 18 | 41 | 5 | - | - | - | - |
| [ ] | P2-14 | query-builder | 4 | P2-04, P2-17 | #1680 | low:7 nit:13 | 20 | 38 | 4 | - | - | - | - |
| [ ] | P2-15 | funclib | 2 | - | #1681 | medium:1 low:2 nit:12 | 15 | 25 | 1 | - | - | - | - |
| [ ] | P2-16 | wasm-host | 2 | - | #1682 | medium:1 low:7 nit:6 | 14 | 22 | 7 | - | - | - | - |
| [ ] | P2-17 | sdk | 3 | P2-16 | #1683 | low:7 nit:4 | 11 | 24 | 5 | - | - | - | - |
| [ ] | P2-18 | net | 2 | P2-09 | #1684 | low:9 nit:22 | 31 | 50 | 10 | - | - | - | - |
| [ ] | P2-19 | infra | 2 | P2-08, P2-05, P2-06 | #1685 | low:8 nit:4 | 12 | 23 | 2 | - | - | - | - |
| [ ] | P2-U01 | undecidable | any | - | #1649 | - | 8 | - | - | - | - | - | - |

## Tasks

### P2-01 - shamir-db: authorization fail-open, function/interner costs, curl gateway, facade cleanup

- Session task: #1667. Brief: `docs/dev-artifacts/prompts/remediation/p2/01-db.md`. Crates: `shamir-db`.
- Wave 1; after: none. Cards 18 (high 1, medium 2, low 8, nit 7), effort 38, owner-decision cards 5.
- Shares files with: P2-07, P2-12, P2-16.

**Goal.** Close the authorization fail-open (a failed group-membership read must deny, not fall through to the Other permission bits), remove the repeated catalogue and settings scans on function invocation, stop materializing the whole interner for every InternerTouch, bound the egress response body, and clean up the facade conventions.

**Acceptance.**

- A regression test where the group lookup fails under mode 0704 shows access is denied for both Group and Other read, and a log line carries the context (card 01C2041B).
- Function invocation loads the function metadata once per call and does not repeat the ancestor settings scans (observable through a counter or test seam).
- InternerTouch computes the epoch without materializing the dictionary; explicit-id and publication-gap semantics are unchanged.
- Owner-decision cards are implemented only with a recorded answer.

**Coordination.**

- RV-DB-39178D2D needs an allocation-free published-id maximum on the interner (crates/shamir-types/src/core/interner/interner.rs); P2-12 also edits that file. Keep the accessor small and expect a trivial merge in the integration branch.
- RV-DB-01C2041B edits access/authorize.rs, which P2-12 touches too (71AFA12F, A7B1FCEC); P2-12 starts after P2-01 is merged.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-DB-01C2041B](P2.md#rv-db-01c2041b) | high | M/code | no | `resolve_in_group` silently converts group-lookup errors into `false` without a log |
| [RV-DB-39178D2D](P2.md#rv-db-39178d2d) | medium | L/code | no | `InternerTouch` computes the epoch via a full interner traversal per touch |
| [RV-DB-2C736C9E](P2.md#rv-db-2c736c9e) | medium | M/code | no | Function invocation scans the function catalogue twice plus two settings scans per call |
| [RV-DB-C2DCAC96](P2.md#rv-db-c2dcac96) | low | L/code | yes | Builder-only query-construction rule bypassed across the facade (30+ hand-assembled wire ops, no exception comments; bu… |
| [RV-DB-0541AF4D](P2.md#rv-db-0541af4d) | low | M/code | yes | Egress response body read without any size cap |
| [RV-DB-2A0B65CB](P2.md#rv-db-2a0b65cb) | low | M/code | no | Per-invocation gateway construction allocates and intersects allowlists via `Vec::contains` |
| [RV-DB-3EB43AC7](P2.md#rv-db-3eb43ac7) | low | M/code | no | Boot path pairs repos with their tables via an O(repos × tables) nested scan |
| [RV-DB-830B18FC](P2.md#rv-db-830b18fc) | low | M/code | no | Repeated curl resolve lines discard earlier validated addresses |
| [RV-DB-A05E9048](P2.md#rv-db-a05e9048) | low | M/code | no | `foreign_key_dto_from_qv` silently coerces unknown `on_delete`/`on_update` strings to `NoAction` -- and then re-persist… |
| [RV-DB-A571C2F8](P2.md#rv-db-a571c2f8) | low | M/code | no | `handle_start_migration`'s duplicate-start guard is check-then-act over the `active_migrations` map |
| [RV-DB-140F9E99](P2.md#rv-db-140f9e99) | low | S/dep | yes | Dead TLS/password-hash dependencies with a stale "kept compiling" rationale |
| [RV-DB-0065D2A2](P2.md#rv-db-0065d2a2) | nit | L/decision | yes | Catalogue `wasm_hash` and `version` fields are dead, and the hash is not integrity-grade |
| [RV-DB-7C59E01B](P2.md#rv-db-7c59e01b) | nit | M/decision | yes | Nit: intentionally-leaked per-key lock maps are the only unbounded-growth sites — documented, but key count is unbounde… |
| [RV-DB-CBB8403D](P2.md#rv-db-cbb8403d) | nit | M/code | no | Blanket `#![allow(deprecated)]` with no named reason |
| [RV-DB-D5441CCE](P2.md#rv-db-d5441cce) | nit | M/code | no | Function-local `use` statements in production code violate the imports-at-top rule |
| [RV-DB-425999DD](P2.md#rv-db-425999dd) | nit | S/code | no | Minor nits (grouped) |
| [RV-DB-77C5A24C](P2.md#rv-db-77c5a24c) | nit | S/code | no | Stray empty statement and dead comment in `curl_gateway.rs` |
| [RV-DB-C93A58BF](P2.md#rv-db-c93a58bf) | nit | S/doc | no | `shamir_db::shamir_db` module inception suppressed with an unannotated allow |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-01-db` / `worktrees/p2-01-db` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-02 - shamir-tx (+ engine A8 capture marking): lock registry, snapshot churn, tombstone ambiguity, capture coverage

- Session task: #1668. Brief: `docs/dev-artifacts/prompts/remediation/p2/02-tx.md`. Crates: `shamir-tx`.
- Wave 1; after: none. Cards 18 (high 1, medium 1, low 10, nit 6), effort 44, owner-decision cards 4.
- Shares files with: none.

**Goal.** Make the A8 referenced-id capture marking exact per staged key, stop the pessimistic lock registry from growing without bound, let snapshot opening stop blocking reclamation, settle the empty-value versus tombstone ambiguity, and batch timestamp issuance.

**Acceptance.**

- Mixed staging (raw write A, captured write B, then the mark) still persists A's referenced ids above the watermark (regression named after 905A1F59).
- The lock registry returns to its baseline size after N one-shot keys; snapshot-open churn no longer prevents reclamation (progress test).
- Empty-value versus tombstone contract is defined and covered (after the owner answer).

**Coordination.**

- Owns crates/shamir-engine/src/tx/preparation/pre_commit/prelock.rs; P2-04 must not edit it.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-ENGINE-905A1F59](P2.md#rv-engine-905a1f59) | high | L/code | no | A8 captured-ID flag covers only the latest staging call, not every staged record |
| [RV-TX-D1C20D85](P2.md#rv-tx-d1c20d85) | medium | M/code | no | MvccStore::locks registry never evicts empty entries — unbounded growth under pessimistic workloads |
| [RV-ENGINE-FEF268D2](P2.md#rv-engine-fef268d2) | low | L/code | no | A8 pre-commit scan re-decodes every staged record value on every commit |
| [RV-TX-4A1FDAAE](P2.md#rv-tx-4a1fdaae) | low | L/decision | yes | project_event: per-record heap clone of the table-name String |
| [RV-TX-56651C4A](P2.md#rv-tx-56651c4a) | low | L/code | yes | Changefeed journal writer: one sequential put await per event |
| [RV-TX-7BB0D16C](P2.md#rv-tx-7bb0d16c) | low | L/code | no | A10 in-flight barrier can starve GC under sustained snapshot-open churn |
| [RV-TX-21B69CC7](P2.md#rv-tx-21b69cc7) | low | M/code | no | Vectored reads: sequential fallback/cold awaits and redundant re-probe |
| [RV-TX-3B9F4D3B](P2.md#rv-tx-3b9f4d3b) | low | M/code | no | history_of: N sequential lookup_ts point-reads |
| [RV-TX-4D85CE16](P2.md#rv-tx-4d85ce16) | low | M/code | no | VersionedOverlay::gc_upto: full-tree collect-then-remove |
| [RV-TX-782352E1](P2.md#rv-tx-782352e1) | low | M/code | yes | Empty Bytes as tombstone sentinel is unguarded on the public write path |
| [RV-TX-D70EC152](P2.md#rv-tx-d70ec152) | low | M/code | no | remap_inner_value_bytes re-encodes rows that changed nothing |
| [RV-TX-ED8562B0](P2.md#rv-tx-ed8562b0) | low | M/code | yes | tx_id = 0 non-tx write sentinel is unenforced against zero-seeded id allocators |
| [RV-TX-43CD8A27](P2.md#rv-tx-43cd8a27) | nit | M/code | no | Mid-function use statements in production code |
| [RV-TX-C935EAB9](P2.md#rv-tx-c935eab9) | nit | M/test | no | Test placement deviates from documented per-module tests layout |
| [RV-TX-D2989766](P2.md#rv-tx-d2989766) | nit | M/code | no | set_versioned_many and append_only: duplicate key vector |
| [RV-TX-FB4FD0FE](P2.md#rv-tx-fb4fd0fe) | nit | M/test | no | metrics.rs has no test coverage in this crate |
| [RV-TX-3FC2FA1D](P2.md#rv-tx-3fc2fa1d) | nit | S/code | no | lookup_ts swallows history.get errors as unknown age with no log |
| [RV-TX-5508B0C1](P2.md#rv-tx-5508b0c1) | nit | S/doc | no | RepoChangefeed::new panics outside a Tokio runtime; writer task is detached |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-02-tx` / `worktrees/p2-02-tx` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-03 - shamir-client-node: integer policy, builder contract, credential lifetime, binding hygiene

- Session task: #1669. Brief: `docs/dev-artifacts/prompts/remediation/p2/03-client-node.md`. Crates: `shamir-client-node`.
- Wave 1; after: none. Cards 9 (high 1, medium 2, low 3, nit 3), effort 19, owner-decision cards 3.
- Shares files with: none.

**Goal.** Give the Node binding a lossless integer policy shared with the TS DTOs, steer execute(object) onto the builder path, make the credential-lifetime claims true, and remove redundant decode/copy and false documentation.

**Acceptance.**

- Independent fixtures at +-(2^53+1) round-trip exactly; BigInt input is accepted; safe numbers are unchanged; the policy matches the TS DTOs and builders.
- execute(object) either goes through the builder types or its documentation no longer teaches hand-assembled wire shapes (owner answer).
- Secrets the binding controls are zeroized and the documentation claims match reality.

**Coordination.**

- Keep zeroization consistent with RV-CLIENT-8997B366 in P2-09.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-CLIENT-NODE-C42AA40A](P2.md#rv-client-node-c42aa40a) | high | L/code | yes | Default object-level MessagePack conversion loses integer fidelity and rejects BigInt inputs |
| [RV-CLIENT-NODE-144A5920](P2.md#rv-client-node-144a5920) | medium | L/code | yes | `execute(object)` teaches hand-assembled wire shapes; no builder, no exported BatchRequest/BatchResponse types |
| [RV-CLIENT-NODE-684A48EB](P2.md#rv-client-node-684a48eb) | medium | S/doc | no | "Mirrors the Rust SDK 1:1" is false; resumption ticket getters dead-end with no resume path |
| [RV-CLIENT-NODE-050EF626](P2.md#rv-client-node-050ef626) | low | M/code | no | `create_scram_user` wrapper msgpack-decodes 16 raw user_id bytes and discards the result |
| [RV-CLIENT-NODE-4E3D7A44](P2.md#rv-client-node-4e3d7a44) | low | M/code | no | Secret copies in the napi struct are not zeroized (`resumption_ticket`, `session_id`, `pin`) |
| [RV-CLIENT-NODE-F64125FA](P2.md#rv-client-node-f64125fa) | low | M/code | no | Redundant decode + copy on every `repl`/`createScramUser` success, and a Uint8Array copy per `decodeOrThrow` |
| [RV-CLIENT-NODE-C35E4678](P2.md#rv-client-node-c35e4678) | nit | M/code | no | "Zeroised in the native side" overstates what the binding controls |
| [RV-CLIENT-NODE-31D3BBE5](P2.md#rv-client-node-31d3bbe5) | nit | S/doc | yes | Version drift: Cargo `0.1.0-alpha.1` vs npm/generated-loader `0.1.0` |
| [RV-CLIENT-NODE-4B69BB13](P2.md#rv-client-node-4b69bb13) | nit | S/doc | no | Sanctioned-exception comment present at `repl` but missing at the identical `execute` rmp-serde boundary |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-03-client-node` / `worktrees/p2-03-client-node` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-04 - shamir-engine: numeric membership parity, schema one_of, cache/token costs, cleanup

- Session task: #1670. Brief: `docs/dev-artifacts/prompts/remediation/p2/04-engine.md`. Crates: `shamir-engine`.
- Wave 2; after: P2-02. Cards 13 (medium 1, low 7, nit 5), effort 32, owner-decision cards 1.
- Shares files with: P2-12, P2-13, P2-14.

**Goal.** Make the optimized contains_all and schema one_of paths agree with the dynamic semantics (coercing numerics, NaN), and trim cache/token overhead and small structural debts in the engine.

**Acceptance.**

- Table-driven tests show the optimized contains_all agrees with dynamic membership for integers, floats and NaN.
- The precomputed schema one_of accepts exactly what the fallback equality accepts, including NaN.
- Cache/token overhead changes keep results identical; no edits to pre_commit/prelock.rs (owned by P2-02).

**Coordination.**

- Starts after P2-02 is merged (prelock.rs). Shares filter_node.rs and resolve.rs with P2-14 (P2-14 starts after P2-04). Shares types/value.rs with P2-12 in the same wave: the overlap is small (95ECC2FE vs 313627EB); resolve it in the integration merge.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-ENGINE-7CA0C179](P2.md#rv-engine-7ca0c179) | medium | M/code | no | ContainsAllSet's seen-index optimization loses coercing numeric membership |
| [RV-ENGINE-3425FDE6](P2.md#rv-engine-3425fde6) | low | L/decision | yes | Engine boundary performs no authorization — enforcement is a single upstream wrapper |
| [RV-ENGINE-3C71057D](P2.md#rv-engine-3c71057d) | low | L/code | no | FK exists_in_table fallback: full parent-table scan per child record |
| [RV-ENGINE-62C6F2B1](P2.md#rv-engine-62c6f2b1) | low | M/code | no | Content-key cache remediation introduces per-evaluation formatting allocation and AST traversal |
| [RV-ENGINE-95ECC2FE](P2.md#rv-engine-95ecc2fe) | low | M/code | no | Precomputed one_of rejects allowed NaN values accepted by the original equality contract |
| [RV-ENGINE-B442D16F](P2.md#rv-engine-b442d16f) | low | M/code | no | Shadow drain applies entries via per-entry set/remove round-trips |
| [RV-ENGINE-BE59E793](P2.md#rv-engine-be59e793) | low | M/code | no | doctor::repair deep-clones the materialized table per index rebuild |
| [RV-ENGINE-BE70B015](P2.md#rv-engine-be70b015) | low | M/code | no | one_of rule: linear Vec::contains with materializing clone per record |
| [RV-ENGINE-CEBE3101](P2.md#rv-engine-cebe3101) | nit | L/code | no | read_history issues one awaited history_of per matched record |
| [RV-ENGINE-06B39B8F](P2.md#rv-engine-06b39b8f) | nit | M/code | no | table_token re-derives a hash of the immutable table name |
| [RV-ENGINE-6CCD199D](P2.md#rv-engine-6ccd199d) | nit | M/code | no | Grouped per-row small-allocation nits |
| [RV-ENGINE-931318A1](P2.md#rv-engine-931318a1) | nit | M/code | no | Per-record/per-rule small allocations across schema validation |
| [RV-ENGINE-D0CF9DFA](P2.md#rv-engine-d0cf9dfa) | nit | M/code | no | FkReverseCache::get_or_build_by_parent — unbounded CAS-loss retry while holding build_lock |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-04-engine` / `worktrees/p2-04-engine` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-05 - shamir-index (vector): snapshot compat/integrity, SQ8 alignment, replay/streaming, brute-force lifecycle

- Session task: #1671. Brief: `docs/dev-artifacts/prompts/remediation/p2/05-index-vector.md`. Crates: `shamir-index`.
- Wave 1; after: none. Cards 13 (medium 5, low 4, nit 4), effort 26, owner-decision cards 3.
- Shares files with: none.

**Goal.** Make the vector snapshot honest and safe: a real v1 decode path (or an honest claim), no panic on corrupt-but-decodable sidecars, no unaligned u32 lane loads on AArch64, batched delta replay, streaming dump/load, and a brute-force adapter with an observed lifecycle.

**Acceptance.**

- A snapshot built from real historical v1 bytes decodes (or the compatibility claim is removed, per owner answer); the existing vacuous test is replaced.
- A decodable sidecar with inconsistent parameter lengths returns an error instead of panicking.
- SQ8 loads are correct for unaligned byte slices on every target (tests on aligned and unaligned offsets); delta replay is batched; snapshot dump/load no longer holds the whole graph twice (owner answer).

**Coordination.**

- Owns files under crates/shamir-index/src/vector/**.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-INDEX-4DC29EFB](P2.md#rv-index-4dc29efb) | medium | L/code | yes | Vector snapshot dump/load fully materializes the graph and sidecar in RAM |
| [RV-INDEX-40BCFF22](P2.md#rv-index-40bcff22) | medium | M/code | no | Vector snapshot v1 back-compat is claimed but has no working decode path, and the only test is vacuous |
| [RV-INDEX-9E0EC9A5](P2.md#rv-index-9e0ec9a5) | medium | M/code | yes | Snapshot load path can panic on corrupt-but-decodable persisted data; the sidecar carries no checksum |
| [RV-INDEX-C3FBD7AC](P2.md#rv-index-c3fbd7ac) | medium | M/code | no | AArch64 safe SQ8 byte-slice callers can reach an alignment-requiring u32 lane load |
| [RV-INDEX-FDC0161A](P2.md#rv-index-fdc0161a) | medium | M/code | no | Vector delta-log replay on restart applies ops one at a time |
| [RV-INDEX-0256E6B5](P2.md#rv-index-0256e6b5) | low | M/code | no | `BruteForceAdapter::search` runs an O(N·dim) exact scan inline on the async runtime (pillar 2: CPU-bound → `spawn_block… |
| [RV-INDEX-55A6887F](P2.md#rv-index-55a6887f) | low | M/code | no | `BruteForceAdapter` deep-clones the whole snapshot on every drained write batch |
| [RV-INDEX-6BE64AFC](P2.md#rv-index-6be64afc) | low | M/code | no | `HnswAdapter` f32 small-index search clones every vector per query |
| [RV-INDEX-8533E947](P2.md#rv-index-8533e947) | low | M/code | yes | Snapshot load joins persisted `basename`/`qbasename` into temp file paths unsanitized; manifest/sidecar carry no integr… |
| [RV-INDEX-B23D8FE7](P2.md#rv-index-b23d8fe7) | nit | M/test | no | Inline `#[cfg(test)] mod tests` inside an implementation file |
| [RV-INDEX-B424A8D5](P2.md#rv-index-b424a8d5) | nit | M/code | no | Actor `shutdown` discards the join result (panic payload), and `BruteForceAdapter::shutdown` adds a lock-poisoning expe… |
| [RV-INDEX-54363D3B](P2.md#rv-index-54363d3b) | nit | S/doc | no | `BruteForceAdapter::join: std::sync::Mutex<Option<JoinHandle>>` lacks the inline contention-model comment CLAUDE.md req… |
| [RV-INDEX-ABD60B4B](P2.md#rv-index-abd60b4b) | nit | S/doc | no | Stale crate-root invariant: "NO `std::sync::Mutex` / `RwLock` / `parking_lot`" |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-05-index-vector` / `worktrees/p2-05-index-vector` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-06 - shamir-index (FTS, backends, hash identity): persisted hash identity, IndexError, FTS degradation

- Session task: #1672. Brief: `docs/dev-artifacts/prompts/remediation/p2/06-index-fts-hash.md`. Crates: `shamir-index`.
- Wave 2; after: none. Cards 9 (medium 4, low 5), effort 28, owner-decision cards 6.
- Shares files with: P2-07.

**Goal.** Settle persisted hash identity for unique and FTS keys (keyed hash or value comparison, with a migration or rebuild policy), keep structured errors through IndexError, make FTS degradation visible, and bound the n-gram tokenizer.

**Acceptance.**

- Unique and FTS identity no longer rest on a correlated or unkeyed hash alone, or the owner records an accepted risk with a migration/rebuild policy.
- IndexError keeps the structured DbError identity across boundaries (owner answer on the public error contract).
- Corrupt FTS posting values and silent fallbacks are logged and surfaced; the n-gram tokenizer output is bounded.

**Coordination.**

- Owns src/fts/**, src/backends/**, base_index/infra and index_manager_unique. tokenizer.rs is also edited by P2-07 (32C98E9F): P2-07 starts after P2-06.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-INDEX-2BA3B945](P2.md#rv-index-2ba3b945) | medium | L/code | yes | `IndexError` is stringly-typed; structured `DbError`s are flattened at every boundary |
| [RV-INDEX-53CEF405](P2.md#rv-index-53cef405) | medium | L/decision | yes | FTS posting keys hash untrusted token text with unkeyed `FxHasher` (`token_hash`) |
| [RV-INDEX-9E472725](P2.md#rv-index-9e472725) | medium | L/decision | yes | Regular + unique index keys use two correlated FxHasher streams as "collision resistance"; unique constraints are enfor… |
| [RV-INDEX-872016D9](P2.md#rv-index-872016d9) | medium | M/code | no | `FtsRankedBackend::plan_update` tokenizes the old record twice |
| [RV-INDEX-0695AADA](P2.md#rv-index-0695aada) | low | L/decision | yes | `NgramTokenizer` output is unbounded — indexing-time memory/write amplification from one long token |
| [RV-INDEX-605EEC5D](P2.md#rv-index-605eec5d) | low | L/decision | yes | Persisted posting keys depend on FxHasher output stability, with no version coupling and a caret-pinned dependency |
| [RV-INDEX-9733F1F3](P2.md#rv-index-9733f1f3) | low | M/code | no | Corrupt FTS posting values are silently replaced with `tf=1, doc_len=1` |
| [RV-INDEX-F39434C2](P2.md#rv-index-f39434c2) | low | M/code | no | Silent degradation fallbacks with no logging |
| [RV-INDEX-FBB72544](P2.md#rv-index-fbb72544) | low | M/code | yes | FunctionalBackend Map hashing is insertion-order dependent (byte-identity floor violation) |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-06-index-fts-hash` / `worktrees/p2-06-index-fts-hash` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-07 - shamir-index (managers, runtime registry, model): posting cache, DROP sweeps, registry scans, docs

- Session task: #1673. Brief: `docs/dev-artifacts/prompts/remediation/p2/07-index-runtime.md`. Crates: `shamir-index`.
- Wave 3; after: P2-06, P2-12. Cards 13 (medium 2, low 4, nit 7), effort 24, owner-decision cards 2.
- Shares files with: P2-01, P2-06, P2-12.

**Goal.** Give the posting cache a byte budget, page the DROP sweeps, replace the O(N) registry scans, and clean up docs and convention debts in the index managers and runtime model.

**Acceptance.**

- The posting cache has a byte budget with an eviction test; DROP sweeps page their key list; registry lookups are not O(N) per call.
- No local use statements or inline cfg(test) modules remain in the touched files; docs match the shipped behavior.

**Coordination.**

- Starts after P2-06 (tokenizer.rs) and P2-12 (interner.rs, for 002DDDA6).

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-INDEX-002DDDA6](P2.md#rv-index-002ddda6) | medium | M/code | no | `IndexExpr::Scalar` evaluation constructs a fresh `Interner` per row |
| [RV-INDEX-95108B6D](P2.md#rv-index-95108b6d) | medium | M/code | yes | Posting cache is bounded by entry count, not bytes |
| [RV-INDEX-12B88835](P2.md#rv-index-12b88835) | low | M/code | no | `.unwrap()` on `SystemTime::duration_since(UNIX_EPOCH)` on four DDL success paths |
| [RV-INDEX-1443E02B](P2.md#rv-index-1443e02b) | low | M/code | no | `plan_records_created_batch` bypasses the in-flight-online-build dirty-set capture that every other write path performs |
| [RV-INDEX-3854F3CC](P2.md#rv-index-3854f3cc) | low | M/decision | yes | `lease_by_field_and_kind` is an O(N) full-registry scan on every index2 read dispatch |
| [RV-INDEX-ECE5EBA5](P2.md#rv-index-ece5eba5) | low | M/code | no | DROP-sweep paths materialize the entire index key list before removing (O(index-size) memory spike, one giant batch) |
| [RV-INDEX-32C98E9F](P2.md#rv-index-32c98e9f) | nit | M/code | no | `use` statements inside function/block bodies across six production files |
| [RV-INDEX-858BB5E1](P2.md#rv-index-858bb5e1) | nit | M/doc | no | `ReaderDrainGate` doc invariant ("Never acquire any other lock while holding a `ReadGuard`") is contradicted in letter… |
| [RV-INDEX-97E36480](P2.md#rv-index-97e36480) | nit | M/doc | no | Bincode ordinal-stability contract is documented on some persisted enums but missing on others |
| [RV-INDEX-AFA66311](P2.md#rv-index-afa66311) | nit | M/test | no | Feature-gated inline loom test module in an implementation file |
| [RV-INDEX-B683BA68](P2.md#rv-index-b683ba68) | nit | M/doc | no | Comment nits: typo and a stale cross-file doc reference |
| [RV-INDEX-20415F00](P2.md#rv-index-20415f00) | nit | S/doc | no | Stale lifecycle doc contradicts the shipped `IndexState` wire enum; minor `IndexRecordKey` API warts |
| [RV-INDEX-DE92095C](P2.md#rv-index-de92095c) | nit | S/code | no | `apply_index_ops_at_commit` does a linear backend find per provenance group |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-07-index-runtime` / `worktrees/p2-07-index-runtime` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-08 - shamir-storage: range termination, rename copy recovery, cache/test/doc cleanup

- Session task: #1674. Brief: `docs/dev-artifacts/prompts/remediation/p2/08-storage.md`. Crates: `shamir-storage`.
- Wave 1; after: none. Cards 21 (medium 1, low 11, nit 9), effort 43, owner-decision cards 7.
- Shares files with: P2-12, P2-19.

**Goal.** Stop the default range filter at the upper bound, recover from a partial copy on the production rename path, remove the environmental thread-start panic, and tidy storage docs and tests.

**Acceptance.**

- The default range filter stops polling after the first key above the end bound (oracle: no further polls).
- A partial copy on the production rename path is recovered or rolled back with a test; thread-start failure no longer panics.
- Documentation and test-organization cards are closed without behavior change.

**Coordination.**

- record_id.rs (38F208C4, 9794FD53) is also edited by P2-12; collections lib.rs (8FEF1C5A) by P2-19. P2-12 and P2-19 start after P2-08.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-STORAGE-0786A4D6](P2.md#rv-storage-0786a4d6) | medium | M/code | no | Trait-default range filter scans PAST the upper bound forever |
| [RV-STORAGE-38F208C4](P2.md#rv-storage-38f208c4) | low | L/decision | yes | Fresh random 128-bit id claim behind skipping the insert collision probe is false |
| [RV-STORAGE-9478B37E](P2.md#rv-storage-9478b37e) | low | L/decision | yes | Repo::copy_store default impl leaves a partially-populated destination on failure |
| [RV-STORAGE-0A9E3A41](P2.md#rv-storage-0a9e3a41) | low | M/code | yes | Error-source chains flattened; thread-spawn failure panics instead of DbResult |
| [RV-STORAGE-2F8C6A8C](P2.md#rv-storage-2f8c6a8c) | low | M/code | no | moka cache built with default RandomState instead of workspace THasher |
| [RV-STORAGE-B7839F16](P2.md#rv-storage-b7839f16) | low | M/doc | no | Public-API rustdoc drift: prefetch promise, phantom engines, stale KeyBytes status |
| [RV-STORAGE-BE72410B](P2.md#rv-storage-be72410b) | low | M/code | no | Raw key bytes — including attacker-influenced indexed values — embedded in error messages |
| [RV-STORAGE-CC5C79BA](P2.md#rv-storage-cc5c79ba) | low | M/code | yes | Store names passed to the durable engine unvalidated |
| [RV-STORAGE-1A5A9374](P2.md#rv-storage-1a5a9374) | low | S/doc | yes | NIT: cross-path write ordering inside FjallStore rests only on fjall's internal journal mutex arrival order |
| [RV-STORAGE-8FEF1C5A](P2.md#rv-storage-8fef1c5a) | low | S/doc | no | User-influenced keys enter non-keyed FxHash maps despite the documented no untrusted hash inputs premise |
| [RV-STORAGE-9794FD53](P2.md#rv-storage-9794fd53) | low | S/code | no | Cross-crate wire-format literal duplicated privately: [0,0,0,0] system-record prefix re-encoded in storage_mirrored |
| [RV-STORAGE-DCBCB721](P2.md#rv-storage-dcbcb721) | low | S/doc | no | Stale module doc in key_bytes.rs claims the type is unused and that RecordKey = Bytes |
| [RV-STORAGE-88C4F2CC](P2.md#rv-storage-88c4f2cc) | nit | L/decision | yes | Interface polish nits |
| [RV-STORAGE-3D6F974D](P2.md#rv-storage-3d6f974d) | nit | M/test | no | storage_membuffer_tests.rs packs three unrelated fixture topics into nested inline mods instead of topic files |
| [RV-STORAGE-6FEF6FDD](P2.md#rv-storage-6fef6fdd) | nit | M/code | no | Duplicate private RecordStream alias re-declared instead of importing the canonical one |
| [RV-STORAGE-B26793CA](P2.md#rv-storage-b26793ca) | nit | M/doc | no | Nit — stale narration of retired mechanisms in #535 test/hook docs and mid-body imports |
| [RV-STORAGE-B6B3CBB6](P2.md#rv-storage-b6b3cbb6) | nit | M/code | no | Minor allocations/clones on batched paths |
| [RV-STORAGE-B7703B8B](P2.md#rv-storage-b7703b8b) | nit | M/doc | no | Orphaned Tests banner comments left behind after tests moved out |
| [RV-STORAGE-F26D060C](P2.md#rv-storage-f26d060c) | nit | M/code | no | Function-local imports violate the mandatory Imports at the top rule |
| [RV-STORAGE-FB7C0794](P2.md#rv-storage-fb7c0794) | nit | M/code | yes | Nit: KeyBytes::Deserialize allocates an unbounded blob before any size check |
| [RV-STORAGE-458CFAD8](P2.md#rv-storage-458cfad8) | nit | S/doc | no | Drifted line-number reference in a comment |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-08-storage` / `worktrees/p2-08-storage` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-09 - shamir-client: malformed rows and response frames, interner traversal, resume options, cleanup

- Session task: #1675. Brief: `docs/dev-artifacts/prompts/remediation/p2/09-client.md`. Crates: `shamir-client`.
- Wave 1; after: none. Cards 16 (medium 2, low 8, nit 6), effort 33, owner-decision cards 2.
- Shares files with: P2-12, P2-18.

**Goal.** Turn malformed id-keyed rows and undecodable response frames into errors instead of empty records and hung waiters, zeroize the resume ticket, and cut interner traversal and encode copies.

**Acceptance.**

- A malformed id-keyed row yields a typed error; an undecodable response frame fails its waiters promptly instead of hanging them, and is logged above debug level.
- The resume ticket and its wire copy are Zeroizing (owner answer); interner traversal and encode copies are reduced (allocation test or measured).

**Coordination.**

- api.rs and transport.rs are also edited by P2-18 (79910BA5, 8999F6B7): P2-18 starts after P2-09. messagepack.rs (9DEFDE43) is also edited by P2-12.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-CLIENT-A911BA9E](P2.md#rv-client-a911ba9e) | medium | L/code | no | Malformed id-keyed rows can be returned successfully as empty or partial records |
| [RV-CLIENT-56E4E99D](P2.md#rv-client-56e4e99d) | medium | M/code | no | Undecodable response frames are dropped at `debug` level; default-unbounded waiters hang |
| [RV-CLIENT-8997B366](P2.md#rv-client-8997b366) | low | L/code | yes | `ResumeOptions::ticket` and its wire copy are not `Zeroizing` |
| [RV-CLIENT-267BD36E](P2.md#rv-client-267bd36e) | low | M/code | no | `get_ddl_op_status`: comment/doc/behavior three-way contradiction, dead `DbResponse::Error` arm, zero coverage |
| [RV-CLIENT-3D598780](P2.md#rv-client-3d598780) | low | M/code | no | Ambient interner-epoch advertisement is all-or-nothing per batch |
| [RV-CLIENT-5E5B78A0](P2.md#rv-client-5e5b78a0) | low | M/code | no | The claimed cursor mutex-skip optimization still locks on every yielded record |
| [RV-CLIENT-9DEFDE43](P2.md#rv-client-9defde43) | low | M/code | no | `collect_map_keys` skips `Value::Set` (asymmetric with `qv_has_fn_marker`) |
| [RV-CLIENT-C93ADFED](P2.md#rv-client-c93adfed) | low | M/doc | no | Crate-level doc example no longer compiles against `ConnectOptions`; drift is invisible because doctests are disabled |
| [RV-CLIENT-EB08BA4C](P2.md#rv-client-eb08ba4c) | low | M/code | no | `collect_field_names` clones every field-name key of every record before dedup |
| [RV-CLIENT-462B7413](P2.md#rv-client-462b7413) | low | S/code | no | `encode_record_idmsgpack` pays an avoidable full-record copy per INSERT (`Bytes::to_vec`) |
| [RV-CLIENT-153A118E](P2.md#rv-client-153a118e) | nit | M/code | no | Push frames pay up to two failed envelope parses before `PushEnvelope` decode |
| [RV-CLIENT-17C0D288](P2.md#rv-client-17c0d288) | nit | M/code | no | Wrapping request IDs can replace an outstanding sender or miscorrelate a late response; debug panic is refuted |
| [RV-CLIENT-66E9A5FC](P2.md#rv-client-66e9a5fc) | nit | M/test | no | Perf claims are unmeasured: no benches, no allocation-behaviour tests |
| [RV-CLIENT-F1276114](P2.md#rv-client-f1276114) | nit | M/test | no | `connect_timeout_fires` depends on 10.255.255.1:9 being a silent black hole |
| [RV-CLIENT-0157DABC](P2.md#rv-client-0157dabc) | nit | S/doc | yes | Dead public API: `ClientError::RequestIdMismatch` is never constructed anywhere in the workspace |
| [RV-CLIENT-E39C43D4](P2.md#rv-client-e39c43d4) | nit | S/code | no | Mid-body `use` statements in three `src/tests/` files |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-09-client` / `worktrees/p2-09-client` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-10 - shamir-connect: auth oracles and vectors, ticket bounds, session/password-change invariants, fallible entropy

- Session task: #1676. Brief: `docs/dev-artifacts/prompts/remediation/p2/10-connect.md`. Crates: `shamir-connect`.
- Wave 3; after: P2-09, P2-18. Cards 26 (low 10, nit 16), effort 45, owner-decision cards 9.
- Shares files with: P2-18.

**Goal.** Correct the authentication-message oracles and add byte-exact vectors without changing wire bytes, bound and version the ticket format, keep session identity and cached HMAC keys coherent, and make entropy/clock failures fallible.

**Acceptance.**

- The auth-message oracles use the real 13-byte tag and 148-byte message; wire bytes do not change; byte-exact vectors cover change-password and bootstrap payloads.
- The ticket version is validated on issuance and covered by a version-3 rejection test; ciphertext length cannot truncate silently.
- Session id, cached HMAC key and change-password nonce are coherent through the public lifecycle; entropy and clock failures are fallible, with downstream migration (owner answers).

**Coordination.**

- Starts after P2-09 and P2-18 (handshake.rs, domain_tags.rs, downstream client/server migration when an API becomes fallible).

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-CONNECT-2B5AF3F2](P2.md#rv-connect-2b5af3f2) | low | L/code | yes | Session id is zero-initialized and stamped externally; changepw takes an unchecked second id |
| [RV-CONNECT-387EAA05](P2.md#rv-connect-387eaa05) | low | L/code | yes | Library expect panics for OS RNG failure and pre-epoch clock |
| [RV-CONNECT-7D41FB60](P2.md#rv-connect-7d41fb60) | low | L/code | yes | Folded bundle items |
| [RV-CONNECT-3380EB1C](P2.md#rv-connect-3380eb1c) | low | M/code | yes | Long-lived server secrets are plain arrays outside the zeroization policy |
| [RV-CONNECT-74705EAB](P2.md#rv-connect-74705eab) | low | M/test | no | Registered auth-message header and length oracles contradict the actual domain tag |
| [RV-CONNECT-781E7475](P2.md#rv-connect-781e7475) | low | M/test | no | No byte-exact vectors for auth_message_cp and bootstrap payload |
| [RV-CONNECT-E0A24E86](P2.md#rv-connect-e0a24e86) | low | M/code | yes | Ticket wire version is a magic literal with asymmetric encrypt/decrypt validation |
| [RV-CONNECT-F34827AC](P2.md#rv-connect-f34827ac) | low | M/code | yes | Ticket ciphertext length silently truncates at u16 |
| [RV-CONNECT-F43EB41E](P2.md#rv-connect-f43eb41e) | low | M/test | no | Consolidated error-path test gaps |
| [RV-CONNECT-CD221FBC](P2.md#rv-connect-cd221fbc) | low | S/test | no | Vacuous variant assertions in the all-zero nonce tests |
| [RV-CONNECT-3BAF631D](P2.md#rv-connect-3baf631d) | nit | M/code | yes | start_change_password_challenge accepts an all-zero client_nonce_cp |
| [RV-CONNECT-699EBFCF](P2.md#rv-connect-699ebfcf) | nit | M/code | no | Mid-function use statements violate imports-at-top |
| [RV-CONNECT-C8866EB1](P2.md#rv-connect-c8866eb1) | nit | M/doc | no | Stale doc references to nonexistent artifact paths and old test name |
| [RV-CONNECT-E4631CD9](P2.md#rv-connect-e4631cd9) | nit | M/code | yes | validate_client_kdf_safe returns String; its diagnostic is discarded |
| [RV-CONNECT-349809B3](P2.md#rv-connect-349809b3) | nit | S/doc | no | Username normalization documentation promises sharp-s folding contradicted by pinned dependency and registered oracle |
| [RV-CONNECT-4496810D](P2.md#rv-connect-4496810d) | nit | S/code | no | Stale Debug label reports permissions as &lt;RwLock&gt; |
| [RV-CONNECT-66EA32BD](P2.md#rv-connect-66ea32bd) | nit | S/code | no | RequestEnvelopeRef hard-codes 32 instead of SESSION_ID_BYTES |
| [RV-CONNECT-68CAD3B6](P2.md#rv-connect-68cad3b6) | nit | S/code | no | lockout.rs DashMaps use the default RandomState hasher |
| [RV-CONNECT-850A9AFD](P2.md#rv-connect-850a9afd) | nit | S/code | no | Dead suppressors keeping unused imports alive in server/handshake.rs |
| [RV-CONNECT-8B168854](P2.md#rv-connect-8b168854) | nit | S/code | no | AuditError is hand-rolled with Display equal to Debug |
| [RV-CONNECT-90EC2E05](P2.md#rv-connect-90ec2e05) | nit | S/doc | no | RateLimiter trait doc says sliding-window |
| [RV-CONNECT-A57D515B](P2.md#rv-connect-a57d515b) | nit | S/doc | no | Argon2Semaphore exposes blocking Mutex+Condvar acquisition as an async adoption trap |
| [RV-CONNECT-C75B520F](P2.md#rv-connect-c75b520f) | nit | S/test | no | PushKind round-trip omits Ready |
| [RV-CONNECT-C7F16BFB](P2.md#rv-connect-c7f16bfb) | nit | S/dep | yes | Unused unicode-normalization dependency |
| [RV-CONNECT-E6CF11C8](P2.md#rv-connect-e6cf11c8) | nit | S/code | no | AuthMessage fixed-capacity arithmetic |
| [RV-CONNECT-E71CB30A](P2.md#rv-connect-e71cb30a) | nit | S/doc | no | finalize_change_password doc header describes verification |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-10-connect` / `worktrees/p2-10-connect` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-11 - shamir-wal: leader starvation, retention pin, bounded recovery, rollback schedule, cleanup

- Session task: #1677. Brief: `docs/dev-artifacts/prompts/remediation/p2/11-wal.md`. Crates: `shamir-wal`.
- Wave 1; after: none. Cards 15 (low 7, nit 8), effort 28, owner-decision cards 5.
- Shares files with: none.

**Goal.** Prevent leader starvation in group commit, keep the zero-version retention pin across mixed batches and segments, bound recovery work, stop the destructive rollback schedule, and correct benchmark claims.

**Acceptance.**

- Sustained arrivals cannot keep the elected leader's append future from returning (test); P0 cancellation/fail-stop ticket semantics are preserved.
- Zero-version retention pins survive mixed zero/positive batches and segments (test); recovery is bounded; the rollback schedule is no longer destructive; benchmark claims match what is covered.

**Coordination.**

- Keeps P0 cancellation/fail-stop ticket semantics and the P1 corruption/typed-I/O policies.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-WAL-A25EDE9D](P2.md#rv-wal-a25ede9d) | low | L/code | yes | replay materializes the entire WAL as decoded entries in one Vec |
| [RV-WAL-59E3DBA1](P2.md#rv-wal-59e3dba1) | low | M/code | no | Leader tenure is unbounded under sustained arrival — the leader's own append future cannot resolve |
| [RV-WAL-67475C86](P2.md#rv-wal-67475c86) | low | M/code | no | truncate_below partial failure discards progress and strands claimed files until restart |
| [RV-WAL-79483DA7](P2.md#rv-wal-79483da7) | low | M/code | no | WalSegment::append_batch rollback and bytes_written accounting are unsafe under concurrent callers, while the bench cla… |
| [RV-WAL-8021EB80](P2.md#rv-wal-8021eb80) | low | M/code | no | MemSink batch-max tagging and engine truncation coverage |
| [RV-WAL-CC3131A9](P2.md#rv-wal-cc3131a9) | low | M/code | no | Recovery reads whole segment files without a size cap |
| [RV-WAL-DD492F5C](P2.md#rv-wal-dd492f5c) | low | M/doc | yes | Unauthenticated WAL replay is an injection surface; the data directory is trusted assumption is implicit and undocument… |
| [RV-WAL-0843B96B](P2.md#rv-wal-0843b96b) | nit | M/code | yes | pub mod segment_meta exports nothing public |
| [RV-WAL-3735F642](P2.md#rv-wal-3735f642) | nit | M/code | no | MemSink holds a std::sync::Mutex across O(N) CPU work inside async fns |
| [RV-WAL-3F10E8A1](P2.md#rv-wal-3f10e8a1) | nit | M/decision | yes | Error strings embed absolute filesystem paths |
| [RV-WAL-5DFE355F](P2.md#rv-wal-5dfe355f) | nit | M/decision | yes | WalSegment::mark_poisoned is an un-gated public test hook |
| [RV-WAL-0CAFAF79](P2.md#rv-wal-0cafaf79) | nit | S/code | no | Vestigial, unexplained #[allow(dead_code)] on a public type |
| [RV-WAL-C37B110A](P2.md#rv-wal-c37b110a) | nit | S/doc | no | Per-segment seq counter restarts at 0 on reopen — returned last_seq can collide with pre-existing frames |
| [RV-WAL-C66039FC](P2.md#rv-wal-c66039fc) | nit | S/test | no | Mid-function use statements in tests (imports-at-top rule) |
| [RV-WAL-FE36ADFC](P2.md#rv-wal-fe36adfc) | nit | S/doc | no | WalOpV2::IndexPut/IndexDel serialize idx_id as a constant 0 with semantics deferred |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-11-wal` / `worktrees/p2-11-wal` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-12 - shamir-types: interner hydration validation, decoder arithmetic, logging safety, cleanup

- Session task: #1678. Brief: `docs/dev-artifacts/prompts/remediation/p2/12-types.md`. Crates: `shamir-types`.
- Wave 2; after: P2-01, P2-08, P2-09. Cards 14 (medium 1, low 8, nit 5), effort 28, owner-decision cards 4.
- Shares files with: P2-01, P2-04, P2-07, P2-08, P2-09.

**Goal.** Reject conflicting interner hydration state while keeping legitimate recaptures, close the 32-bit binary-length overflow, stop logging raw resource names unescaped, and fix docs and test hygiene.

**Acceptance.**

- Conflicting hydrated ids/names are rejected while identical delta recaptures and the contiguous-prefix cursor keep working (H13).
- The 32-bit binary-length arithmetic is checked like the string case; raw resource names are not interpolated unescaped into logs.

**Coordination.**

- Starts after P2-01, P2-08 and P2-09 (shared files). Touches shamir-db authorize.rs (71AFA12F, A7B1FCEC): rebase on P2-01. Shares types/value.rs with P2-04 in the same wave (small overlap, resolve in the integration merge).

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-TYPES-973FEEFD](P2.md#rv-types-973feefd) | medium | L/code | no | `Interner::with_state` silently collapses duplicate ids/names in hydrated state |
| [RV-TYPES-5E4BFC76](P2.md#rv-types-5e4bfc76) | low | L/decision | yes | Generic `bincode::from_bytes` helper lacks the depth caps its sibling decoders have |
| [RV-TYPES-A7B1FCEC](P2.md#rv-types-a7b1fcec) | low | L/code | yes | Authorization helpers allocate fresh Strings + Vecs per check on the access-gate path |
| [RV-TYPES-71AFA12F](P2.md#rv-types-71afa12f) | low | M/code | no | Log-forging surface: raw resource names rendered into trace/denial lines |
| [RV-TYPES-E82C6BE1](P2.md#rv-types-e82c6be1) | low | M/code | no | `merge_storage_bytes` allocates intermediate Vecs per NEW set_map entry before copying into the output buffer |
| [RV-TYPES-EFA1B8F4](P2.md#rv-types-efa1b8f4) | low | M/code | yes | `mpack!` rejects negative (or any multi-token) literals as OBJECT values — asymmetric with arrays/lists, uncovered by t… |
| [RV-TYPES-F070E432](P2.md#rv-types-f070e432) | low | M/code | no | `pos + len` unchecked additions in the tree decoder's `read_str`/`read_bin` |
| [RV-TYPES-FE40CB54](P2.md#rv-types-fe40cb54) | low | M/code | yes | `SecretString`: constant-time-comparison footgun plus avoidable `unsafe` |
| [RV-TYPES-FEB0B5E4](P2.md#rv-types-feb0b5e4) | low | S/doc | no | Predictable RecordId random tail and embedded timestamps — invariant undocumented |
| [RV-TYPES-178ED8CB](P2.md#rv-types-178ed8cb) | nit | S/test | no | Mid-function imports scattered through test files |
| [RV-TYPES-2A360D16](P2.md#rv-types-2a360d16) | nit | S/doc | no | README describes an obsolete locking model for `Interner` |
| [RV-TYPES-313627EB](P2.md#rv-types-313627eb) | nit | S/code | no | `QueryValue::set_path` builds the error-message path prefix during successful traversals too |
| [RV-TYPES-8AC2CFC5](P2.md#rv-types-8ac2cfc5) | nit | S/test | no | Nit: vacuous self-comparison assertion in `from_ts_produces_unique_ids_with_same_timestamp` |
| [RV-TYPES-A31DDF76](P2.md#rv-types-a31ddf76) | nit | S/code | no | Dead "Tests" section banner left behind after inline-test extraction |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-12-types` / `worktrees/p2-12-types` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-13 - shamir-query-types: conversion loss, equality, serializer/deserializer parity, pagination, planner allocations, cleanup

- Session task: #1679. Brief: `docs/dev-artifacts/prompts/remediation/p2/13-query-types.md`. Crates: `shamir-query-types`.
- Wave 3; after: P2-04. Cards 18 (low 10, nit 8), effort 41, owner-decision cards 5.
- Shares files with: P2-04.

**Goal.** Remove silent conversion loss, non-transitive equality and serializer/deserializer incompatibility in the request/response DTOs, fix pagination equality, reduce planner allocations, and clean up docs and tests.

**Acceptance.**

- No silent conversion loss: lossy conversions error or are explicit; the RU7 decimal-string projection is preserved.
- Equality is consistent (transitive) or documented; serializer and deserializer round-trip for the affected types; pagination equality is fixed; planner allocations are reduced without result changes.

**Coordination.**

- Starts after P2-04 (authorized.rs).

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-QUERY-TYPES-0A0D9092](P2.md#rv-query-types-0a0d9092) | low | L/code | no | Three separate full-tree recursive walks per request: `is_write`, `distinct_repos`, `collect_required_access` |
| [RV-QUERY-TYPES-182C703E](P2.md#rv-query-types-182c703e) | low | L/code | yes | thiserror convention deviation: hand-rolled Display/Error impls and `Result<(), String>` public APIs |
| [RV-QUERY-TYPES-4F767AA5](P2.md#rv-query-types-4f767aa5) | low | L/code | yes | `From<QueryValue> for FilterValue` silently substitutes `Null` in release builds |
| [RV-QUERY-TYPES-3D922433](P2.md#rv-query-types-3d922433) | low | M/code | no | `Pagination::eq` (`After`) — two msgpack encodes per equality comparison |
| [RV-QUERY-TYPES-4E49851D](P2.md#rv-query-types-4e49851d) | low | M/code | no | Plan-time marker decode pays a msgpack round-trip per `$query`/`$fn`/`$cond`/`$expr` marker |
| [RV-QUERY-TYPES-5F9D5468](P2.md#rv-query-types-5f9d5468) | low | M/code | yes | QueryRecord PartialEq violates transitivity through the opaque-row Null sentinel |
| [RV-QUERY-TYPES-C000A440](P2.md#rv-query-types-c000a440) | low | M/code | no | `Pagination`'s `PartialEq` can panic via `key_bytes`' `expect` |
| [RV-QUERY-TYPES-D1C4DEAB](P2.md#rv-query-types-d1c4deab) | low | M/code | no | Batch planner — redundant alias-set clone and repeated String re-cloning through the plan |
| [RV-QUERY-TYPES-F05184D8](P2.md#rv-query-types-f05184d8) | low | M/code | yes | `InsertedRecord` with a non-map `fields` and no `id` serializes to a shape its own deserializer rejects |
| [RV-QUERY-TYPES-02448485](P2.md#rv-query-types-02448485) | low | S/code | no | `QueryRecord::get_value_{i64,u64,bool}` — deep-clones the whole `Inserted` record per scalar lookup |
| [RV-QUERY-TYPES-CC015A3C](P2.md#rv-query-types-cc015a3c) | nit | L/decision | yes | Per-construction `"main"` String allocations |
| [RV-QUERY-TYPES-0172CFB1](P2.md#rv-query-types-0172cfb1) | nit | M/code | no | Mid-function `use` statements in implementation files (imports-at-top breach) |
| [RV-QUERY-TYPES-4B4C84BF](P2.md#rv-query-types-4b4c84bf) | nit | M/test | no | Mid-function `use` statements in standalone test files |
| [RV-QUERY-TYPES-881BC94F](P2.md#rv-query-types-881bc94f) | nit | M/doc | no | Doc/wire mismatches and stale references |
| [RV-QUERY-TYPES-9223F400](P2.md#rv-query-types-9223f400) | nit | M/code | no | `is_false` helper defined four times with three visibilities and two referencing conventions |
| [RV-QUERY-TYPES-D687DBA3](P2.md#rv-query-types-d687dba3) | nit | M/code | no | `FieldPath` type alias defined in `filter/mod.rs` |
| [RV-QUERY-TYPES-BFFB2B3A](P2.md#rv-query-types-bffb2b3a) | nit | S/doc | no | `InsertedRecord::serialize` — per-record `Vec` collect + sort + base58, contradicting the "allocation-free" module claim |
| [RV-QUERY-TYPES-D319B6E3](P2.md#rv-query-types-d319b6e3) | nit | S/doc | no | `InsertOp` carries two parallel record channels with unspecified result ordering |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-13-query-types` / `worktrees/p2-13-query-types` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-14 - shamir-query-builder (+ macros): copying, reference collection, macro emission/parsing/docs, cleanup

- Session task: #1680. Brief: `docs/dev-artifacts/prompts/remediation/p2/14-query-builder.md`. Crates: `shamir-query-builder`, `shamir-query-builder-macros`.
- Wave 4; after: P2-04, P2-17. Cards 20 (low 7, nit 13), effort 38, owner-decision cards 4.
- Shares files with: P2-04, P2-17.

**Goal.** Cut builder copying and reference collection, fix macro emission, diagnostics and parsing/allocation, correct macro docs, and clean up the builder crate.

**Acceptance.**

- Builder copying/reference collection is reduced with unchanged output (existing fixtures stay green).
- Macro emission, diagnostics and parsing changes keep builder-only expansion paths and clause-keyword disambiguation; docs are corrected.

**Coordination.**

- Starts after P2-04 (filter_node.rs, resolve.rs) and P2-17 (sdk api/db.rs).

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-QUERY-BUILDER-FE2003D1](P2.md#rv-query-builder-fe2003d1) | low | L/decision | yes | `lit_u64`'s decimal-`String` encoding for `u64 > i64::MAX` is only specified for equality |
| [RV-QUERY-BUILDER-8BEB53C0](P2.md#rv-query-builder-8beb53c0) | low | M/code | no | `try_build`'s conservative fallback re-serializes `Call`/`Subscribe` ops that are already typed |
| [RV-QUERY-BUILDER-9E172C69](P2.md#rv-query-builder-9e172c69) | low | M/code | yes | `Batch::build()` deep-clones the entire request; this sits on the SDK/client send path |
| [RV-QUERY-BUILDER-MACROS-02993B77](P2.md#rv-query-builder-macros-02993b77) | low | M/dep | yes | Nit: emitted `::shamir_query_builder` absolute paths break if a downstream renames the dependency |
| [RV-QUERY-BUILDER-MACROS-13A0D25A](P2.md#rv-query-builder-macros-13a0d25a) | low | M/code | no | Field-path and alias spans are discarded; downstream errors point at the macro call site |
| [RV-QUERY-BUILDER-2A648617](P2.md#rv-query-builder-2a648617) | low | S/doc | no | `Batch::switch` guard construction is O(K^2) deep clones and O(K^2) emitted filter nodes |
| [RV-QUERY-BUILDER-320AD085](P2.md#rv-query-builder-320ad085) | low | S/test | no | `switch` with zero cases emits `when: Not{Or{[]}}`; empty-`Or` truth value is unpinned and untested |
| [RV-QUERY-BUILDER-0E30EB25](P2.md#rv-query-builder-0e30eb25) | nit | M/doc | no | Stale doc references |
| [RV-QUERY-BUILDER-202E00CD](P2.md#rv-query-builder-202e00cd) | nit | M/code | no | Duplicate re-exports: the same items re-exported in both a sibling file and its `mod.rs` |
| [RV-QUERY-BUILDER-21560B4A](P2.md#rv-query-builder-21560b4a) | nit | M/doc | no | `val::func`/`val::expr` collide by name with `select::func`/`select::expr` for glob-import users |
| [RV-QUERY-BUILDER-7A0F3500](P2.md#rv-query-builder-7a0f3500) | nit | M/code | no | `ddl/` applies one-file-one-export inconsistently — family files bundle many unrelated public builders |
| [RV-QUERY-BUILDER-A548E006](P2.md#rv-query-builder-a548e006) | nit | M/code | no | All four `macro_rules!` definitions live inline in `macros/mod.rs` |
| [RV-QUERY-BUILDER-C861C539](P2.md#rv-query-builder-c861c539) | nit | M/test | no | `cursor` module's tests use a bare `tests.rs` instead of the documented `tests/` directory + manifest |
| [RV-QUERY-BUILDER-DE20E96B](P2.md#rv-query-builder-de20e96b) | nit | M/dep | yes | Five error enums hand-roll `Display` + `std::error::Error` despite the workspace thiserror rule |
| [RV-QUERY-BUILDER-E2602921](P2.md#rv-query-builder-e2602921) | nit | M/code | no | Imports not at top: one production-code site + a pervasive function-local `use` pattern in tests |
| [RV-QUERY-BUILDER-MACROS-1AF72B7B](P2.md#rv-query-builder-macros-1af72b7b) | nit | M/code | no | Unjustified `pub`, redundant wrapper, duplicated field-path emitter |
| [RV-QUERY-BUILDER-MACROS-A690BEA8](P2.md#rv-query-builder-macros-a690bea8) | nit | M/code | no | Per-predicate-call String/Vec micro-allocations in filter lowering |
| [RV-QUERY-BUILDER-MACROS-EC1B86B3](P2.md#rv-query-builder-macros-ec1b86b3) | nit | M/code | no | Where-clause tokens are captured and re-parsed -- 2x token traffic plus a full copy of every group |
| [RV-QUERY-BUILDER-MACROS-83FE5F55](P2.md#rv-query-builder-macros-83fe5f55) | nit | S/doc | no | Doc: "All five forms" — there are six |
| [RV-QUERY-BUILDER-MACROS-E78E1666](P2.md#rv-query-builder-macros-e78e1666) | nit | S/doc | no | Doc drift: "all 19 predicate calls" (there are 17); `vector_similarity_ef`/`_opts` unreachable from the DSL |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-14-query-builder` / `worktrees/p2-14-query-builder` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-15 - shamir-funclib: numeric order contract, Argon2 oracle, documentation and test maintenance

- Session task: #1681. Brief: `docs/dev-artifacts/prompts/remediation/p2/15-funclib.md`. Crates: `shamir-funclib`.
- Wave 2; after: none. Cards 15 (medium 1, low 2, nit 12), effort 25, owner-decision cards 1.
- Shares files with: none.

**Goal.** Decide and implement the cross-numeric ordering contract of compare (with the distinct-hashing contract), make the Argon2 saturation oracle deterministic, and fix docs and test/layout debts.

**Acceptance.**

- The cross-numeric order is either exact (boundary-triple and permutation oracles) or explicitly documented as lossy, and the distinct-hashing contract matches (owner answer).
- The Argon2 saturation oracle no longer depends on scheduling (permit-held rendezvous); documentation and test/layout cards are closed.

**Coordination.**

- Comparator fix must revisit the distinct-hashing contract retained by #330.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-FUNCLIB-0FCB1771](P2.md#rv-funclib-0fcb1771) | medium | M/code | yes | `compare`'s documented "total (transitive)" order is not strictly guaranteed on the lossy fallback paths |
| [RV-FUNCLIB-22DF7C3C](P2.md#rv-funclib-22df7c3c) | low | M/doc | no | `trusted_pure` gate: pub fields make the "explicit opt-in" convention-only; docs claim indexability the gate forbids |
| [RV-FUNCLIB-935392A1](P2.md#rv-funclib-935392a1) | low | M/test | no | Argon2 concurrency-cap test still relies on scheduling-dependent saturation after the claimed deterministic-flake fixes |
| [RV-FUNCLIB-110F6E17](P2.md#rv-funclib-110f6e17) | nit | M/code | no | Argon2id semaphore: poison-propagating `expect`s, and the in-flight counter leaks on panic |
| [RV-FUNCLIB-37BE26F9](P2.md#rv-funclib-37be26f9) | nit | M/code | no | Nits (test organization / dead code / minor drift) |
| [RV-FUNCLIB-3E2AD533](P2.md#rv-funclib-3e2ad533) | nit | M/code | no | `use` statements inside function bodies |
| [RV-FUNCLIB-7539483C](P2.md#rv-funclib-7539483c) | nit | M/test | no | `registry` contract tests nested under `math/tests/` |
| [RV-FUNCLIB-8447708B](P2.md#rv-funclib-8447708b) | nit | M/test | no | Single-line JSON literals in tests |
| [RV-FUNCLIB-9F35E215](P2.md#rv-funclib-9f35e215) | nit | M/doc | no | Doc/code drift: `random()` documented as returning `F64`, actually `Dec` |
| [RV-FUNCLIB-BDCBEE51](P2.md#rv-funclib-bdcbee51) | nit | M/test | no | Tests for five flat modules consolidated in `src/tests/` instead of per-module `tests/` dirs |
| [RV-FUNCLIB-06F1EB02](P2.md#rv-funclib-06f1eb02) | nit | S/code | no | `value_nav` int-step-into-map allocates a key `String` per step |
| [RV-FUNCLIB-50CD6B2E](P2.md#rv-funclib-50cd6b2e) | nit | S/doc | no | `ScalarRegistry::register` collision policy undocumented |
| [RV-FUNCLIB-7EDC7D1C](P2.md#rv-funclib-7edc7d1c) | nit | S/doc | no | `strings/substring` doc comment contradicts implemented (and tested) behavior |
| [RV-FUNCLIB-C1C6EEC7](P2.md#rv-funclib-c1c6eec7) | nit | S/doc | no | Doc drift: encode.rs header lists `str_escape_chars` as a registered function name |
| [RV-FUNCLIB-E6DE0D0F](P2.md#rv-funclib-e6de0d0f) | nit | S/code | no | `cast/to_bool` allocates a lowercased copy of the input per call |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-15-funclib` / `worktrees/p2-15-funclib` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-16 - shamir-wasm-host: runtime-compile SDK path, bounded compiler output, allocation and metadata cleanup

- Session task: #1682. Brief: `docs/dev-artifacts/prompts/remediation/p2/16-wasm-host.md`. Crates: `shamir-wasm-host`.
- Wave 2; after: none. Cards 14 (medium 1, low 7, nit 6), effort 22, owner-decision cards 7.
- Shares files with: P2-01, P2-17.

**Goal.** Make runtime Rust-source compilation independent of the build machine checkout, bound the compiler output, validate pooling limits against custom function limits, and clean up allocation and metadata handling.

**Acceptance.**

- compile_rust_source resolves the SDK from a supported, configurable source and fails with an actionable error when it is missing (owner answer).
- Compiler output retained in memory is bounded with a truncation marker, keeping the proc-tree lifecycle guarantees; pooling ceiling versus custom function limits is validated.

**Coordination.**

- wasm_function.rs is also edited by P2-17 (723F5FD2): P2-17 starts after P2-16. core.rs of shamir-db (4669FDA6) is also edited by P2-01.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-WASM-HOST-C17D12C8](P2.md#rv-wasm-host-c17d12c8) | medium | M/decision | yes | `compile_rust_source` hardwires the SDK path to the build machine's `CARGO_MANIFEST_DIR` |
| [RV-WASM-HOST-4669FDA6](P2.md#rv-wasm-host-4669fda6) | low | L/code | yes | Catalogue record decoding has silent fallbacks and no format versioning |
| [RV-WASM-HOST-203F2147](P2.md#rv-wasm-host-203f2147) | low | M/code | no | `host_call` rebuilds the secret-grants set on every nested call instead of cloning the `Arc` |
| [RV-WASM-HOST-98551A44](P2.md#rv-wasm-host-98551a44) | low | M/decision | yes | Pooling allocator hardcodes `max_memory_size` to `WasmLimits::default()`, silently capping configured limits |
| [RV-WASM-HOST-EB1654FA](P2.md#rv-wasm-host-eb1654fa) | low | M/code | yes | Unbounded stdout/stderr buffering of the guest `cargo build` |
| [RV-WASM-HOST-796ECAE4](P2.md#rv-wasm-host-796ecae4) | low | S/code | no | `compile.rs` degrades invalid-UTF-8 temp paths into `unwrap_or("")` arguments for cargo instead of a typed error |
| [RV-WASM-HOST-C5833F4E](P2.md#rv-wasm-host-c5833f4e) | low | S/code | no | Per-invocation input prep: deep params clone + msgpack encode + redundant `.to_vec()` copy |
| [RV-WASM-HOST-FE02BB1D](P2.md#rv-wasm-host-fe02bb1d) | low | S/code | yes | Panic inside the Argon2id blocking task is collapsed into `FunctionError::Cancelled`, discarding the panic payload |
| [RV-WASM-HOST-FF7C2D6E](P2.md#rv-wasm-host-ff7c2d6e) | nit | M/code | yes | Nits bundle (correctness-tdd finding 12, five items, distributed to their primary lenses) |
| [RV-WASM-HOST-741D48B5](P2.md#rv-wasm-host-741d48b5) | nit | S/doc | no | `wasm/mod.rs` doc list omits the sanitizer re-exports |
| [RV-WASM-HOST-8FA4CADD](P2.md#rv-wasm-host-8fa4cadd) | nit | S/code | no | Redundant artifact copies in `maybe_wasm_opt` |
| [RV-WASM-HOST-92EA3A88](P2.md#rv-wasm-host-92ea3a88) | nit | S/code | no | Mid-body `use` statements violate the "Imports at the top" rule (7 sites) |
| [RV-WASM-HOST-C43E9092](P2.md#rv-wasm-host-c43e9092) | nit | S/doc | no | Internal audit-tracking references leaked into public API docs |
| [RV-WASM-HOST-EBD32CA5](P2.md#rv-wasm-host-ebd32ca5) | nit | S/dep | yes | unused `serde` dependency |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-16-wasm-host` / `worktrees/p2-16-wasm-host` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-17 - shamir-sdk (+ sdk-macros): ABI contracts, macro consumer contract, conventions cleanup

- Session task: #1683. Brief: `docs/dev-artifacts/prompts/remediation/p2/17-sdk.md`. Crates: `shamir-sdk`, `shamir-sdk-macros`.
- Wave 3; after: P2-16. Cards 11 (low 7, nit 4), effort 24, owner-decision cards 5.
- Shares files with: P2-14, P2-16.

**Goal.** Document and enforce the SDK ABI contracts, support renamed-dependency consumers of the macros, and clean up conventions.

**Acceptance.**

- SDK ABI contracts are documented and enforced where the card asks; macros work for renamed-dependency consumers (owner answers).
- Convention cleanup closes without ABI changes beyond the cards.

**Coordination.**

- Starts after P2-16 (wasm_function.rs); P2-14 starts after P2-17 (sdk api/db.rs).

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-SDK-77261152](P2.md#rv-sdk-77261152) | low | L/decision | yes | Documented scalar "purity guarantee" is a token-match lint, trivially bypassed by a type alias |
| [RV-SDK-MACROS-44A9CC7A](P2.md#rv-sdk-macros-44a9cc7a) | low | L/decision | yes | `type_contains_ctx` purity check is lexical, not semantic |
| [RV-SDK-17BC120C](P2.md#rv-sdk-17bc120c) | low | M/code | no | Guest ABI builds slices from host-returned `(ptr, len)` with no sanity check (undocumented unsafe trust assumption) |
| [RV-SDK-E3875787](P2.md#rv-sdk-e3875787) | low | M/code | yes | `leak_result` truncates pointers on 64-bit host targets (latent UB in the host-testing ABI path) |
| [RV-SDK-MACROS-C8189C06](P2.md#rv-sdk-macros-c8189c06) | low | M/code | yes | Generated code hardcodes `shamir_sdk::` paths; `_attr` tokens silently ignored |
| [RV-SDK-9ABDB893](P2.md#rv-sdk-9abdb893) | low | S/test | no | `Value` edge cases: `visit_u64` wrap-around and NaN/Infinity untestable under `PartialEq` |
| [RV-SDK-MACROS-5D1F61B0](P2.md#rv-sdk-macros-5d1f61b0) | low | S/doc | no | `#[procedure]` doc example does not compile |
| [RV-SDK-723F5FD2](P2.md#rv-sdk-723f5fd2) | nit | L/doc | yes | Guest ABI passes pointers as signed `i32`; host rejects addresses ≥ 2 GiB |
| [RV-SDK-9D3236E4](P2.md#rv-sdk-9d3236e4) | nit | M/code | no | Function-body `use` imports in `src/tests/` violate the "Imports at the top" rule |
| [RV-SDK-8BC846C6](P2.md#rv-sdk-8bc846c6) | nit | S/doc | no | Convention nits |
| [RV-SDK-DDF49297](P2.md#rv-sdk-ddf49297) | nit | S/doc | no | `value.rs` module doc points at a test path that does not exist |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-17-sdk` / `worktrees/p2-17-sdk` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-18 - shamir-server + transports (tcp, ws, ipc): origin policy, loopback, exporter docs, test maintenance

- Session task: #1684. Brief: `docs/dev-artifacts/prompts/remediation/p2/18-net.md`. Crates: `shamir-server`, `shamir-transport-tcp`, `shamir-transport-ws`, `shamir-transport-ipc`.
- Wave 2; after: P2-09. Cards 31 (low 9, nit 22), effort 50, owner-decision cards 10.
- Shares files with: P2-09, P2-10.

**Goal.** Validate WebSocket origin patterns, align the loopback predicates of transport and server configuration, remove avoidable clones and staging costs in TCP, and fix exporter docs and transport/server test maintenance.

**Acceptance.**

- Configured WS origin patterns and supplied serialized origins are validated; one-level wildcard semantics are preserved (owner answer).
- Transport and server loopback predicates agree; TCP staging/clone costs and exporter docs are fixed; TCP secret zeroization and terminal cancellation behavior are preserved.

**Coordination.**

- Starts after P2-09 (client api.rs/transport.rs); P2-10 starts after P2-18 (connect handshake.rs, domain_tags.rs).

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-SERVER-E8E1D11B](P2.md#rv-server-e8e1d11b) | low | L/decision | yes | Replication client uses trust-on-first-use with no leader-key pinning |
| [RV-SERVER-A65118CC](P2.md#rv-server-a65118cc) | low | M/code | yes | Duplicated normative loopback predicate with shamir-transport-tcp |
| [RV-SERVER-F60C70B4](P2.md#rv-server-f60c70b4) | low | M/code | no | Concurrent namespace updates can clobber each other |
| [RV-TRANSPORT-IPC-FA481516](P2.md#rv-transport-ipc-fa481516) | low | M/decision | yes | chmod-after-bind race window (documented, spec-prescribed — report as accepted risk with a structural option) |
| [RV-TRANSPORT-TCP-17E11DF1](P2.md#rv-transport-tcp-17e11df1) | low | M/doc | no | Project-specific exporter is incorrectly described as strict RFC 9266 compliance |
| [RV-TRANSPORT-TCP-31C5E8A6](P2.md#rv-transport-tcp-31c5e8a6) | low | M/test | no | Two `read_exact` calls per frame: extra read round-trip on unbuffered plain-TCP streams |
| [RV-TRANSPORT-TCP-79910BA5](P2.md#rv-transport-tcp-79910ba5) | low | M/code | no | Allocating `read_frame`/`write_frame` remain the ergonomic defaults and are still on a production per-request hot path |
| [RV-TRANSPORT-WS-67E355C7](P2.md#rv-transport-ws-67e355c7) | low | M/code | no | `accept_browser_ws` clones the origin policy on every connection |
| [RV-TRANSPORT-WS-F0A54173](P2.md#rv-transport-ws-f0a54173) | low | M/code | yes | Wildcard origin matcher is raw string logic — accepts literal `*` and userinfo forms; unvalidated patterns fail silently |
| [RV-SERVER-8999F6B7](P2.md#rv-server-8999f6b7) | nit | M/test | no | Positional handshake DTOs independently mirrored with shamir-client |
| [RV-SERVER-97C240D7](P2.md#rv-server-97c240d7) | nit | M/decision | yes | `bootstrap_password` accepted as a CLI argument |
| [RV-TRANSPORT-IPC-400D803E](P2.md#rv-transport-ipc-400d803e) | nit | M/code | no | Bind-collision error text is OS-asymmetric and unmapped |
| [RV-TRANSPORT-TCP-16C543F3](P2.md#rv-transport-tcp-16c543f3) | nit | M/test | no | Transport wire structs and fixtures duplicated across the two e2e test files |
| [RV-TRANSPORT-TCP-4D95A5F8](P2.md#rv-transport-tcp-4d95a5f8) | nit | M/code | yes | Write-side frame cap is hardcoded to `MAX_FRAME_SIZE_DEFAULT`; reader/writer API asymmetry |
| [RV-TRANSPORT-TCP-5CE7BA5B](P2.md#rv-transport-tcp-5ce7ba5b) | nit | M/test | no | e2e tests run Argon2id inline on the test runtime thread while a peer task is spawned (fragile, no live failure) |
| [RV-TRANSPORT-TCP-B48A0EF2](P2.md#rv-transport-tcp-b48a0ef2) | nit | M/doc | no | Sync CPU-bound bootstrap helpers lack a spawn_blocking / bootstrap-only contract (pillar 2, letter) |
| [RV-TRANSPORT-WS-5AF6EE84](P2.md#rv-transport-ws-5af6ee84) | nit | M/doc | no | Error-semantics / doc-accuracy warts |
| [RV-SERVER-4512C85D](P2.md#rv-server-4512c85d) | nit | S/code | no | Vacuous dead-code discard of `ConnectError::AuthFailed` |
| [RV-TRANSPORT-IPC-9B233842](P2.md#rv-transport-ipc-9b233842) | nit | S/doc | no | Stale doc: "reject_remote_clients … is left at its default" while the code explicitly pins it |
| [RV-TRANSPORT-TCP-05C670C2](P2.md#rv-transport-tcp-05c670c2) | nit | S/test | no | Function-local `use` statements violate the imports-at-top rule (5 sites) |
| [RV-TRANSPORT-TCP-6062E565](P2.md#rv-transport-tcp-6062e565) | nit | S/doc | no | Exporter doc cluster misdescribes the adapter abstraction |
| [RV-TRANSPORT-TCP-9C706A16](P2.md#rv-transport-tcp-9c706a16) | nit | S/dep | yes | `tokio` dependency pulls `features = ["full"]` in a library crate |
| [RV-TRANSPORT-TCP-A13A3534](P2.md#rv-transport-tcp-a13a3534) | nit | S/doc | no | Docs advertise Unix-domain-socket binds for `Plain`; the API is `SocketAddr`/TCP-only |
| [RV-TRANSPORT-TCP-CD312411](P2.md#rv-transport-tcp-cd312411) | nit | S/code | no | Mixed spellings of the same upstream crate in one import block |
| [RV-TRANSPORT-TCP-EC4F02F5](P2.md#rv-transport-tcp-ec4f02f5) | nit | S/code | no | Spec wire constant duplicated from a direct dependency |
| [RV-TRANSPORT-TCP-FA40B59D](P2.md#rv-transport-tcp-fa40b59d) | nit | S/code | no | Speculative dead public consts with cargo-cult `#[allow(dead_code)]` |
| [RV-TRANSPORT-WS-3D98F208](P2.md#rv-transport-ws-3d98f208) | nit | S/doc | no | `BrowserOriginPolicy::empty()` doc references a nonexistent `accept_no_origin` mode |
| [RV-TRANSPORT-WS-460A02D6](P2.md#rv-transport-ws-460a02d6) | nit | S/dep | yes | Unused `[dev-dependencies]`: `hex`, `serde`, `serde_bytes`, `rmp-serde` |
| [RV-TRANSPORT-WS-5D3BB859](P2.md#rv-transport-ws-5d3bb859) | nit | S/code | yes | Attacker-controlled `Origin` echoed into the HTTP 403 response body |
| [RV-TRANSPORT-WS-B1243564](P2.md#rv-transport-ws-b1243564) | nit | S/code | yes | `Origin` header containing obs-text bytes is reported as `Missing` rather than rejected-as-present |
| [RV-TRANSPORT-WS-E6697C98](P2.md#rv-transport-ws-e6697c98) | nit | S/doc | no | Doc drift: `server_ws_config` claims a "4 KiB pre-auth logical check … enforced in `crate::framing::ws_recv_into`" — no… |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-18-net` / `worktrees/p2-18-net` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-19 - shamir-numa + shamir-bench-utils + shamir-collections: detection, generator contract, oracles, docs

- Session task: #1685. Brief: `docs/dev-artifacts/prompts/remediation/p2/19-infra.md`. Crates: `shamir-numa`, `shamir-bench-utils`, `shamir-collections`.
- Wave 2; after: P2-08, P2-05, P2-06. Cards 12 (low 8, nit 4), effort 23, owner-decision cards 2.
- Shares files with: P2-08.

**Goal.** Fix the Lcg generator endpoints (bump GENERATOR_REVISION), the NUMA detection, coverage and QEMU oracle, and the related docs.

**Acceptance.**

- Lcg::next_f32 stays in [0, 1) and next_range never reaches hi; GENERATOR_REVISION is bumped where generated data changes.
- NUMA detection/coverage and the QEMU oracle are corrected; the documentation cards are closed.

**Coordination.**

- Starts after P2-08 (collections lib.rs) and after P2-05 and P2-06 are integrated: RV-NUMA-6FDBD35C (decided) switches the shamir-index default constructors, deserialization and clone paths to a cached topology, so shamir-index is in scope for that card only. Within wave 2 this task is the last to start.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Title |
|---|---|---|---|---|
| [RV-NUMA-6FDBD35C](P2.md#rv-numa-6fdbd35c) | low | L/code | yes | `detect()` re-probes `/sys` on every call — no memoized variant, and consumers already call it per-instance construction |
| [RV-BENCH-UTILS-92355828](P2.md#rv-bench-utils-92355828) | low | M/code | yes | next_range can reach hi even when next_f32 is strictly inside its promised interval |
| [RV-BENCH-UTILS-F3BD32CC](P2.md#rv-bench-utils-f3bd32cc) | low | M/code | no | `Lcg::next_f32` violates its documented `[0, 1)` contract — can return exactly 1.0 |
| [RV-NUMA-553CB81D](P2.md#rv-numa-553cb81d) | low | M/test | no | Test-coverage claims vs reality: `detect()` untested off-Linux, `LinuxTopology` error paths untested, README references… |
| [RV-NUMA-79E4752D](P2.md#rv-numa-79e4752d) | low | M/doc | no | `load_local` does not meet its documented "identical to a bare `ArcSwap` — zero overhead" contract |
| [RV-NUMA-B44795F2](P2.md#rv-numa-b44795f2) | low | M/code | no | `detect()` degrades silently -- swallowed probe error has no observability |
| [RV-NUMA-CA89A9AE](P2.md#rv-numa-ca89a9ae) | low | M/doc | no | Stale "Фаза 1 scope" docs contradict shipped code |
| [RV-NUMA-DB772C02](P2.md#rv-numa-db772c02) | low | M/test | no | QEMU smoke oracle counts announcements instead of distinct NUMA nodes |
| [RV-NUMA-E3044ABE](P2.md#rv-numa-e3044abe) | nit | M/code | no | Redundant guards in `detect()` / `probe()` |
| [RV-BENCH-UTILS-88D1646B](P2.md#rv-bench-utils-88d1646b) | nit | S/doc | no | Cargo.toml `description` advertises the removed BENCH_QUICK tier feature |
| [RV-COLLECTIONS-4ED3931B](P2.md#rv-collections-4ed3931b) | nit | S/code | no | Redundant `use std::cmp::Eq` import |
| [RV-NUMA-81E50CBF](P2.md#rv-numa-81e50cbf) | nit | S/doc | no | README's "run tests" instruction bypasses the mandated central test entry point |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p2-19-infra` / `worktrees/p2-19-infra` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
| Upstream PR / squash SHA | - |
| Post-merge re-decide | - |
| Notes | - |

### P2-U01 - Decide or close the 8 statically undecidable cards

- Session task: #1649. No executor brief: each card needs evidence (a measurement, a published artifact, or a crate-wide absence proof) obtained in CI, or an explicit closing decision by the owner.

| Card | Crate | What is missing |
|---|---|---|
| [RV-BENCH-UTILS-F53F775F](P2.md#rv-bench-utils-f53f775f) | shamir-bench-utils | No boundary-margin analysis or CI target witness establishes a deterministic-fixture flake. |
| [RV-SERVER-D5F412E2](P2.md#rv-server-d5f412e2) | shamir-server | Crate-wide absence proof is missing; shared awaited mutex use is observed, not a proven deadlock. |
| [RV-SERVER-CEE352CA](P2.md#rv-server-cee352ca) | shamir-server | Indexed matching and CV-first removal exist; claimed concurrent asymptotic bound is unproved. |
| [RV-CLIENT-NODE-C5E06B98](P2.md#rv-client-node-c5e06b98) | shamir-client-node | Published artifacts are missing evidence; local non-Windows binary loading is positively supported. |
| [RV-ENGINE-DD871E71](P2.md#rv-engine-dd871e71) | shamir-engine | Selected meaningful tests cannot establish the universal coverage claim. |
| [RV-ENGINE-84C0E18F](P2.md#rv-engine-84c0e18f) | shamir-engine | Named publication and collision sites improved; blanket benignity remains unproved. |
| [RV-ENGINE-1220BF81](P2.md#rv-engine-1220bf81) | shamir-engine | Explicit compiled-size and pattern caps exist; complexity and timing assurance remains incomplete. |
| [RV-QUERY-BUILDER-MACROS-9BCA14B1](P2.md#rv-query-builder-macros-9bca14b1) | shamir-query-builder-macros | Uncapped recursion exists, but accepted depth and rustc-abort manifestation are missing. |

Tracking: status `[ ]`; evidence runs and decisions are recorded in the decision log below.

## Owner questions (owner-decision cards)

One line per card; the first clause of the recipe states what has to be decided. Ask by wave, record the answer in the decision log and in the brief.

| Task | Card | Risk | Decide | Decision |
|---|---|---|---|---|
| P2-01 | RV-DB-C2DCAC96 | low | Promote the builder to a production dependency and migrate facade construction with genuine exceptions documented. | DEFER |
| P2-01 | RV-DB-0541AF4D | low | Define a response-byte budget and enforce it during transfer and bounded file reading. | IMPLEMENT |
| P2-01 | RV-DB-140F9E99 | low | Remove unused direct argon2/TLS dependencies and stale legacy-network rationale. | IMPLEMENT |
| P2-01 | RV-DB-0065D2A2 | nit | Define informational versus integrity/version semantics before adding consumers or changing persisted metadata. | IMPLEMENT |
| P2-01 | RV-DB-7C59E01B | nit | Choose bounded striped locking or an explicit retained-key budget without unsafe live-entry eviction. | SKIP |
| P2-02 | RV-TX-4A1FDAAE | low | Choose shared in-memory table-name storage while preserving serialized string values. | SKIP |
| P2-02 | RV-TX-56651C4A | low | Add ordered journal batch persistence without crossing gap or barrier boundaries. | IMPLEMENT |
| P2-02 | RV-TX-782352E1 | low | Reject reserved empty payloads before version allocation while preserving an explicit deletion path. | IMPLEMENT |
| P2-02 | RV-TX-ED8562B0 | low | Reject or normalize zero seeds consistently in both allocators and pin external-zero semantics. | IMPLEMENT |
| P2-03 | RV-CLIENT-NODE-C42AA40A | high | Define a lossless integer policy shared with TS DTOs and test independent int64 fixtures, bigint inputs and integer markers. | IMPLEMENT |
| P2-03 | RV-CLIENT-NODE-144A5920 | medium | Integrate supported builder and batch DTO exports plus an execution adapter and replace handwritten wire examples. | IMPLEMENT |
| P2-03 | RV-CLIENT-NODE-31D3BBE5 | nit | Document the owner-approved Rust and npm version relationship rather than silently changing release versions. | SKIP |
| P2-04 | RV-ENGINE-3425FDE6 | low | Ratify and document trusted embedding APIs versus authorization-enforced batch execution. | IMPLEMENT |
| P2-05 | RV-INDEX-4DC29EFB | medium | Stream bounded snapshot chunks and move sidecar ownership while preserving generation coherence. | DEFER |
| P2-05 | RV-INDEX-9E0EC9A5 | medium | Validate persisted parameters fallibly and add versioned sidecar integrity coverage with corruption fixtures. | IMPLEMENT |
| P2-05 | RV-INDEX-8533E947 | low | Validate single-component basenames and introduce versioned metadata integrity with a legacy-read policy. | IMPLEMENT |
| P2-06 | RV-INDEX-2BA3B945 | medium | Preserve typed storage errors and sources through index and engine boundaries. | IMPLEMENT |
| P2-06 | RV-INDEX-53CEF405 | medium | Choose collision-resolving token identity with an explicit persisted FTS rebuild/version policy. | ESCALATE |
| P2-06 | RV-INDEX-9E472725 | medium | Choose collision-resolving canonical-value identity and an index migration/rebuild policy. | ESCALATE |
| P2-06 | RV-INDEX-0695AADA | low | Define explicit token/work budgets and propagate typed over-budget errors without silent truncation. | ESCALATE |
| P2-06 | RV-INDEX-605EEC5D | low | Define portable persisted-hash identity, golden keys and explicit upgrade or rebuild rules. | ESCALATE |
| P2-06 | RV-INDEX-FBB72544 | low | Canonicalize recursive Map hashing and provide persisted-index rebuild handling and order-parity tests. | ESCALATE |
| P2-07 | RV-INDEX-95108B6D | medium | Reserve retained bytes atomically and account for admission, replacement, removal and invalidation. | IMPLEMENT |
| P2-07 | RV-INDEX-3854F3CC | low | Choose multi-match selection semantics and maintain a lifecycle-synchronized reverse lookup. | IMPLEMENT |
| P2-08 | RV-STORAGE-38F208C4 | low | Approve the actual probabilistic uniqueness contract or implement cross-handle collision-safe insertion. | IMPLEMENT |
| P2-08 | RV-STORAGE-9478B37E | low | Choose explicit partial-copy semantics or a rename-owned staging and recovery protocol. | IMPLEMENT |
| P2-08 | RV-STORAGE-0A9E3A41 | low | Return worker-start failures through DbResult and approve source-preserving error representations. | IMPLEMENT |
| P2-08 | RV-STORAGE-CC5C79BA | low | Apply one approved backend-compatible name validator before ordinary and atomic keyspace operations. | IMPLEMENT |
| P2-08 | RV-STORAGE-1A5A9374 | low | Document per-worker ordering and explicitly decide whether cross-route submission ordering is required. | IMPLEMENT |
| P2-08 | RV-STORAGE-88C4F2CC | nit | Approve a cohesive API-polish scope before changing aliases, bounds, copy semantics or public errors. | SKIP |
| P2-08 | RV-STORAGE-FB7C0794 | nit | Approve a key-size limit and enforce it with a bounded deserialization visitor before growth. | SKIP |
| P2-09 | RV-CLIENT-8997B366 | low | Protect resume ticket ownership, serialized payloads and framing scratch on all exits while retaining wire bytes. | IMPLEMENT |
| P2-09 | RV-CLIENT-0157DABC | nit | Document RequestIdMismatch as reserved and not emitted, or remove it under an explicit API compatibility decision. | IMPLEMENT |
| P2-10 | RV-CONNECT-2B5AF3F2 | low | Bind immutable session identity before HMAC caching and use that identity for password-change proof binding. | IMPLEMENT |
| P2-10 | RV-CONNECT-387EAA05 | low | Introduce fallible entropy and clock APIs and propagate typed environmental errors through callers. | IMPLEMENT |
| P2-10 | RV-CONNECT-7D41FB60 | low | Resolve the four component cards and close this aggregate without duplicating implementation work. | SKIP |
| P2-10 | RV-CONNECT-3380EB1C | low | Use zeroizing ownership for server and resume secrets while preserving immutable key/cache consistency. | IMPLEMENT |
| P2-10 | RV-CONNECT-E0A24E86 | low | Centralize the supported ticket version and reject unsupported versions during encryption. | IMPLEMENT |
| P2-10 | RV-CONNECT-F34827AC | low | Make ticket serialization reject ciphertext lengths above u16::MAX and test boundary round trips. | IMPLEMENT |
| P2-10 | RV-CONNECT-3BAF631D | nit | Reject an all-zero client nonce before replacing pending challenge state and test preservation of the previous challenge. | IMPLEMENT |
| P2-10 | RV-CONNECT-E4631CD9 | nit | Return the common typed KDF error instead of allocated discarded String diagnostics. | SKIP |
| P2-10 | RV-CONNECT-C7F16BFB | nit | Remove the unused direct unicode-normalization dependency while retaining PRECIS normalization. | IMPLEMENT |
| P2-11 | RV-WAL-A25EDE9D | low | Introduce bounded incremental replay and migrate startup and live gap recovery consumers. | SKIP |
| P2-11 | RV-WAL-DD492F5C | low | Document trusted directory writers, required ACLs and the absence of WAL authenticity guarantees. | IMPLEMENT |
| P2-11 | RV-WAL-0843B96B | nit | Remove the empty public segment_meta namespace and align documentation visibility. | SKIP |
| P2-11 | RV-WAL-3F10E8A1 | nit | Define operator diagnostic versus public-error policy and redact externally surfaced paths. | SKIP |
| P2-11 | RV-WAL-5DFE355F | nit | Gate or restrict mark_poisoned after consumer review, or explicitly support it as an administrative API. | SKIP |
| P2-12 | RV-TYPES-5E4BFC76 | low | Define trusted-type/input constraints and require explicitly bounded decoding for recursive untrusted roots. | IMPLEMENT |
| P2-12 | RV-TYPES-A7B1FCEC | low | Introduce borrowed ancestor traversal and compatible metadata lookup while preserving nearest-first awaited authorization. | IMPLEMENT |
| P2-12 | RV-TYPES-EFA1B8F4 | low | Add negative-literal object arms and CI cases with and without trailing commas while retaining explicit expression escapes. | IMPLEMENT |
| P2-12 | RV-TYPES-FE40CB54 | low | Use safe String zeroization, settle public comparison semantics, and add lifecycle and feature CI coverage. | IMPLEMENT |
| P2-13 | RV-QUERY-TYPES-182C703E | low | Migrate formatting to thiserror and adopt typed validation errors under an explicit API compatibility plan. | SKIP |
| P2-13 | RV-QUERY-TYPES-4F767AA5 | low | Introduce fallible QueryValue conversion, migrate callers and reject unsupported maps without Null substitution. | IMPLEMENT |
| P2-13 | RV-QUERY-TYPES-5F9D5468 | low | Make opaque-versus-nonopaque comparisons false and add a three-value transitivity regression. | IMPLEMENT |
| P2-13 | RV-QUERY-TYPES-F05184D8 | low | Accept existing non-map wire values in the decoder and add scalar/list round-trip tests. | IMPLEMENT |
| P2-13 | RV-QUERY-TYPES-CC015A3C | nit | Decide whether measured cost justifies changing public String ownership or explicitly accept default-name allocations. | SKIP |
| P2-14 | RV-QUERY-BUILDER-FE2003D1 | low | Specify and test large-integer ordering, range and membership across Big and decimal-string storage while preserving RU7. | IMPLEMENT |
| P2-14 | RV-QUERY-BUILDER-9E172C69 | low | Add a consuming validated build path and use it where callers own the batch, retaining borrowed reuse. | IMPLEMENT |
| P2-14 | RV-QUERY-BUILDER-MACROS-02993B77 | low | Resolve the consumer dependency alias for all builder expansion paths and add an alias-only consumer fixture. | ESCALATE |
| P2-14 | RV-QUERY-BUILDER-DE20E96B | nit | Add thiserror and migrate the five error implementations while preserving messages and sources. | IMPLEMENT |
| P2-15 | RV-FUNCLIB-0FCB1771 | medium | Implement exact cross-numeric ordering, reconcile distinct hashing, and add boundary-triple and permutation oracles. | IMPLEMENT |
| P2-16 | RV-WASM-HOST-C17D12C8 | medium | Define a deployment-supported SDK source and configurable runtime resolution with actionable errors. | IMPLEMENT |
| P2-16 | RV-WASM-HOST-4669FDA6 | low | Introduce checked version-aware metadata decoding with explicit malformed-field errors and legacy defaults. | IMPLEMENT |
| P2-16 | RV-WASM-HOST-98551A44 | low | Define an engine memory ceiling and reconcile accepted function limits with pooling capacity. | IMPLEMENT |
| P2-16 | RV-WASM-HOST-EB1654FA | low | Drain both pipes continuously while retaining capped diagnostics with explicit truncation markers. | IMPLEMENT |
| P2-16 | RV-WASM-HOST-FE02BB1D | low | Distinguish blocking-task panic from cancellation and preserve a diagnostic panic cause. | IMPLEMENT |
| P2-16 | RV-WASM-HOST-FF7C2D6E | nit | Complete the remaining path and metadata components through cards 796ECAE4 and 4669FDA6. | SKIP |
| P2-16 | RV-WASM-HOST-EBD32CA5 | nit | Remove the unused direct serde dependency. | IMPLEMENT |
| P2-17 | RV-SDK-77261152 | low | Choose signature-only purity documentation or host-enforced scalar capability isolation. | IMPLEMENT |
| P2-17 | RV-SDK-MACROS-44A9CC7A | low | Define host-enforced scalar capability policy and replace the lexical purity guarantee with accurate documentation and tests. | IMPLEMENT |
| P2-17 | RV-SDK-E3875787 | low | Restrict packed result ABI to wasm32 and use native-width interfaces for native testing. | IMPLEMENT |
| P2-17 | RV-SDK-MACROS-C8189C06 | low | Resolve dependency aliases and reject nonempty unsupported attribute tokens with consumer CI fixtures. | IMPLEMENT |
| P2-17 | RV-SDK-723F5FD2 | nit | Explicitly define the below-2-GiB ABI range and align memory documentation and boundary fixtures. | IMPLEMENT |
| P2-18 | RV-SERVER-E8E1D11B | low | Define persisted leader-pin enrollment, rotation, mismatch, and reconnect policy and wire it into replication. | ESCALATE |
| P2-18 | RV-SERVER-A65118CC | low | Choose mapped-IPv4 loopback policy and align or contract-test both validation paths. | IMPLEMENT |
| P2-18 | RV-TRANSPORT-IPC-FA481516 | low | Record accepted conditional exposure and offer an owner-only socket parent as structural mitigation. | SKIP |
| P2-18 | RV-TRANSPORT-WS-F0A54173 | low | Validate origin patterns and serialized origins while preserving explicit one-level wildcard semantics. | IMPLEMENT |
| P2-18 | RV-SERVER-97C240D7 | nit | Choose protected-file/stdin credential input and deprecate or explicitly document plaintext argv input. | IMPLEMENT |
| P2-18 | RV-TRANSPORT-TCP-4D95A5F8 | nit | Add capped writer companions while retaining existing default-cap wrappers. | IMPLEMENT |
| P2-18 | RV-TRANSPORT-TCP-9C706A16 | nit | Narrow Tokio library features and declare test-only runtime capabilities separately. | IMPLEMENT |
| P2-18 | RV-TRANSPORT-WS-460A02D6 | nit | Remove the unused hex, serde, serde_bytes and rmp-serde dev-dependencies. | IMPLEMENT |
| P2-18 | RV-TRANSPORT-WS-5D3BB859 | nit | Return a fixed browser origin-rejection body while preserving typed diagnostics internally. | IMPLEMENT |
| P2-18 | RV-TRANSPORT-WS-B1243564 | nit | Classify present non-text browser origins as NotAllowed rather than Missing. | IMPLEMENT |
| P2-19 | RV-NUMA-6FDBD35C | low | Introduce shared cached topology for constructors while retaining an explicit fresh-probe path. | IMPLEMENT |
| P2-19 | RV-BENCH-UTILS-92355828 | low | Enforce exclusive range endpoints, define invalid-bound handling and add adjacent-float tests with revision bump. | IMPLEMENT |

## Decision log

| Date | Task / card | Decision | Answer | By |
|---|---|---|---|---|
| 2026-10-29 | P2-19 / RV-BENCH-UTILS-92355828 | IMPLEMENT | For finite lo < hi, preserve next_range's signature, interpolate in f64, round to f32, and constrain to [lo, hi) using hi's representable predecessor. Add try_next_range with explicit invalid-bounds errors; define the legacy wrapper to return NaN for nonfinite, equal, or reversed bounds without consuming RNG state. Consume one step per valid draw. Combine with the exact 24-bit next_f32 fix and bump GENERATOR_REVISION once. CI must cover predecessor seeds, adjacent signed floats, subnormals, opposite finite extremes, and invalid bounds. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-09 / RV-CLIENT-0157DABC | IMPLEMENT | Keep ClientError::RequestIdMismatch with its existing fields and display text, documenting it as reserved and not currently emitted. Do not emit it for legitimate late responses or redesign multiplexing in this documentation card; keep request-ID exhaustion and malformed-envelope remediation separate. No new behavioral test is needed. Evidence: crates/shamir-client/src/error.rs:95-97; crates/shamir-client/src/client/transport.rs:623-632. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-09 / RV-CLIENT-8997B366 | IMPLEMENT | Preserve ResumeOptions.ticket: Vec<u8> for existing struct-literal callers and add a secure-options alternative owning Zeroizing<Vec<u8>> with a resume entry point sharing one protected handshake implementation. In the legacy async entry point, transfer ticket ownership into Zeroizing before the first internal await; explicitly document its raw-options exposure before polling and recommend secure options for retained or cancellable options. Protect wire copies, serialization scratch including partial-error paths, and received rotated-ticket buffers on success, failure and cancellation. Preser… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-03 / RV-CLIENT-NODE-144A5920 | IMPLEMENT | Choose the documentation-only builder path permitted by the brief: replace handwritten wire examples with existing TS builder .build() output passed to Node execute, document execute(object) as the low-level FFI submission boundary, and explicitly state that Batch.execute(nodeClient, db) is unsupported. Preserve the existing object API. Do not add a new SDK dependency, duplicate batch DTOs, or add an executeWithTouch alias that lacks smart-write behavior; defer DTO re-exports and full adapter integration. CI must compile-check and exercise the documented builder-to-execute example together wi… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-03 / RV-CLIENT-NODE-31D3BBE5 | SKIP | Record the Cargo/npm metadata discrepancy as accepted for this sprint, without changing versions or asserting an unrecorded owner-approved release policy. The generated loader compares optional npm platform-package versions, not Cargo's version, and tries local native files first, so the discrepancy alone does not establish loader failure. Defer version alignment or independence policy to release preparation. Evidence: crates/shamir-client-node/Cargo.toml:3; crates/shamir-client-node/package.json:3; crates/shamir-client-node/native/index.js:189-198. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-03 / RV-CLIENT-NODE-C42AA40A | IMPLEMENT | Implement lossless object-level integer conversion using the existing MessagePack dependency: decode int64/uint64 losslessly, normalize safe decoded integers to Number and retain unsafe integers as BigInt; encode in-range signed/unsigned 64-bit BigInt and promote safe wide integer Numbers so they retain integer wire markers. Reject out-of-range BigInt rather than wrapping it. Preserve fractional Numbers, strings, binary contents and raw repl bytes, and use the TS DTO number-or-bigint contract. CI must use independent fixtures for signed ±(2^53+1), signed/unsigned boundaries, genuine BigInt in… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-10 / RV-CONNECT-2B5AF3F2 | IMPLEMENT | Remove the HMAC cache and derive from the current session ID on each call, preserving public construction/admission signatures and store stamping. Retain the explicit-SID password-change API, reject disagreement with session.session_id, and build the proof message from the session-owned ID while preserving single-use challenge consumption on failure. CI must prove pre-admission key access cannot poison the admitted key and mismatched-SID proofs fail. Defer immutable-identity API redesign. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-10 / RV-CONNECT-3380EB1C | IMPLEMENT | Preserve ServerSecrets public array fields, struct literals, Clone and redacted Debug while deriving zeroization-on-drop with the existing dependency. Wrap ResumeConfig private raw keys in zeroizing ownership, preserving constructor/accessor signatures and immutable cipher-cache binding. CI must cover safely inspectable zeroization behavior, redaction, clones and current/previous-key round trips after cache warming. Explicitly exclude caller copies and expanded AES schedules from the wiping guarantee. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-10 / RV-CONNECT-387EAA05 | IMPLEMENT | Add fallible try_random_bytes, try_random_array and checked UnixNanos try_now/system-time conversion APIs with a dedicated typed environmental error. Migrate production auth, ticket, bootstrap, password-change and dispatch paths, adding fallible entry points where needed and retaining documented legacy wrappers for source compatibility. Never substitute zero entropy or fabricated time; preserve generic peer-visible failures. CI must inject entropy failure and pre-epoch/overflow times and prove typed failure, cleanup and no partial publication. Record retained legacy-wrapper panics as a limita… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-10 / RV-CONNECT-3BAF631D | IMPLEMENT | Return Result<ChangePwChallengeView> from start_change_password_challenge and reject an all-zero client nonce before entropy acquisition or pending-state replacement. Propagate entropy failure through the same result and migrate downstream callers once. Preserve successful challenge fields and verification single-use semantics. CI must prove zero-nonce and entropy failures preserve the previous challenge and valid issuance replaces it; wire formats remain unchanged. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-10 / RV-CONNECT-7D41FB60 | SKIP | Close as duplicate aggregate tracking, not independent implementation. Record component dispositions for C8866EB1, E4631CD9, 387EAA05 and 850A9AFD, including the KDF diagnostic deferral and retained legacy-wrapper panic limitation. Do not claim all components are repaired. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-10 / RV-CONNECT-C7F16BFB | IMPLEMENT | Remove only shamir-connect's unused direct unicode-normalization dependency. Retain precis-profiles, existing dependency versions and UsernameCaseMapped identity semantics; do not introduce replacement normalization. Keep existing PRECIS identity regressions registered for CI. No new behavioral test is required for this manifest-only removal. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-10 / RV-CONNECT-E0A24E86 | IMPLEMENT | Centralize supported ticket version 2 across issuers, encryption, decryption and resume validation. Reject unsupported plaintext versions before serialization or entropy acquisition. Preserve all v2 envelope bytes, MessagePack fields, AAD and rotation behavior. CI must prove both encryption entry points reject version 3 and existing v2 tickets remain decryptable. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-10 / RV-CONNECT-E4631CD9 | SKIP | Accept the existing String diagnostic/API debt for this sprint. Handshake protocol-limit validation precedes the outer safety guard, while direct-helper tests explicitly require memory/time diagnostics. Replacing the public error type loses an existing diagnostic contract without demonstrated current handshake benefit; a parallel API is unnecessary scope. Preserve both limit sets. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-10 / RV-CONNECT-F34827AC | IMPLEMENT | Change TicketWire::to_bytes to the common Result<Vec<u8>>, validate ciphertext length with u16::try_from before allocation, and migrate callers to handle errors. Reject oversized encryption output too. Preserve the u16 wire format and representable encodings. CI must prove round trips at lengths 0 and 65535, rejection at 65536 and oversized issuance failure. Coordinate caller migration with entropy fallibility; the approved break is source-level only. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-01 / RV-DB-0065D2A2 | IMPLEMENT | Document existing wasm_hash as a noncryptographic diagnostic fingerprint and function version=1 as legacy informational metadata, neither an integrity guarantee nor a replacement-generation counter. Preserve field names, representations, existing writes and legacy catalogue compatibility; add no integrity consumer, migration or version increment. This is documentation-only remediation requiring no new behavioral test and must remain independent of invocation-metadata reuse. Evidence: crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs:195-217,228-244; crates/shamir-db/src/shami… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-01 / RV-DB-0541AF4D | IMPLEMENT | Keep CurlNetGateway::new(allowlist), giving it a documented 16 MiB response-body limit, and add a constructor accepting an explicit positive byte limit. Enforce the budget during receipt even without Content-Length, rather than relying solely on curl max-filesize support or post-transfer file metadata. Bound any subsequent file read, reject oversize and failed transfers through the existing transport-error variant, never return truncated success, and kill/reap the child on abort. Preserve response shapes, SSRF checks and the adjacent DNS-pinning fix. CI must prove exact-limit success, limit-p… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-01 / RV-DB-140F9E99 | IMPLEMENT | Remove unused direct argon2, rustls, tokio-rustls and rcgen dependencies and the stale legacy-network rationale from shamir-db only. Preserve dependencies and feature selections in actual crypto-owning crates; do not claim transitive crypto removal. No behavioral test is required for declaration cleanup. CI must check all facade targets, affected dependents and feature combinations for accidental feature-unification reliance. Evidence: crates/shamir-db/Cargo.toml:64,70-77. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-01 / RV-DB-7C59E01B | SKIP | Accept indefinite admin/schema lock-key retention for this sprint. Concrete lock maps are publicly exposed, so replacing them with stripes is not a surgical source-compatible change; eviction can split synchronization for a live key, while a retained-key budget adds new operation failures. Record the tradeoff without claiming negligible memory or that these are the only unbounded structures. Defer bounded locking to an API-encapsulation change and preserve all current lock-order and RMW guarantees. Evidence: crates/shamir-db/src/shamir_db/shamir_db/core.rs:661,669,702; crates/shamir-db/src/sh… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-01 / RV-DB-C2DCAC96 | DEFER | DEFER to a separate follow-up task after P2-01 is accepted: migrating 30+ facade operations to the builder is an L-size change for a low-risk convention card and would dominate the authorization fix. Record the reviewer's approach for that follow-up (promote shamir-query-builder to a production dependency of shamir-db; migrate through supported builders; keep batch semantics, transaction boundaries, authorization ordering and durability calls; document genuine exceptions). Do NOT change this card in P2-01. | rush reviewer recommended IMPLEMENT; orchestrator deferred |
| 2026-10-29 | P2-04 / RV-ENGINE-3425FDE6 | IMPLEMENT | Document raw engine/table APIs and custom AccessGate implementations as trusted native-embedding surfaces, cross-linking authorization-enforced execution through the real facade gate and Authorized token. Add explicit trust warnings to raw handle entry points. Preserve existing APIs and test-util gating; do not duplicate DAC or characterize the already-tokenized normal executor as an observed server bypass. Documentation only; no new behavioral test required. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-15 / RV-FUNCLIB-0FCB1771 | IMPLEMENT | Implement exact mathematical cross-numeric comparison using existing BigInt support: finite floats are exact binary rationals, decimals are mantissa/scale, and integers remain exact. Preserve type ranks, recursive container order, equal signed zeros, and NaN-last/all-NaNs-equal behavior. Align distinct hashing with exact comparator equivalence while preserving first-occurrence output order. Do not alter engine filter/index equality. CI must cover boundary triples, permutation-invariant distinct counts, decimal/float boundaries, huge finite integers versus infinity, and recursive equality/hash… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-06 / RV-INDEX-0695AADA | ESCALATE | Owner must choose finite default input-byte, emitted-token-count, and cumulative normalized-token-byte limits, and whether configuration is deployment-wide or index-specific. Recommend configurable finite budgets with typed pre-allocation rejection and no truncation. Defaults change document/query admission policy. Preserve deletion/cleanup of previously accepted oversized records, tokenize old updates once, and propagate query failures rather than swallowing them in planner fallback. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-06 / RV-INDEX-2BA3B945 | IMPLEMENT | Preserve existing string variants and add a boxed source-bearing DbError variant with centralized conversions. Carry actual storage failures through it, propagate metadata Store errors directly, and recover the original DbError at engine boundaries. Include engine adapters in scope; preserve wire encoding and existing constructors, while updating exhaustive matches for the additive enum variant. CI must prove an injected Io error retains its variant, ErrorKind, and source through index-to-engine propagation. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-07 / RV-INDEX-3854F3CC | IMPLEMENT | Preserve multiple indexes per field/kind and any-Ready-match selection. Add an Fx-hashed reverse map from recognized kind and first field path to candidate IDs, maintained on successful insert/remove and rebuilt through recovery registration. Resolve only the candidate bucket and revalidate identity, membership and authoritative Ready state in by_id. Rename leaves membership unchanged; state transitions remain authoritative in by_id. Document O(k) candidate work, preserve reader-gate acquisition and lease lifetime, and test multiple matches, lifecycle transitions, failed registration, rename,… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-05 / RV-INDEX-4DC29EFB | DEFER | DEFER to a dedicated design task: streaming snapshot dump/load is a high-risk redesign of snapshot persistence (staging generations, durability before manifest publication, incremental checksums) for a medium memory-cost card with no demonstrated incident. Do NOT change this card in P2-05; keep 9E0EC9A5, 8533E947 and FDC0161A, and avoid changes that would make a later streaming design harder. | rush reviewer recommended IMPLEMENT; orchestrator deferred |
| 2026-10-29 | P2-06 / RV-INDEX-53CEF405 | ESCALATE | ESCALATE (orchestrator): the reviewer recommended a coordinated persisted-identity redesign with a rebuild migration; because it changes the persisted format of unique, regular, FTS and functional postings, it needs owner/@oh sign-off before any code. Reviewer proposal: Use length-framed normalized token bytes as authoritative FTS identity; hashes are in-memory accelerators only. Add a text-token query path and migrate the engine planner, retaining the old hash API only as documented legacy hash-union semantics. Add an index2 identity-version gate that durably marks rebuilding, drops old post… | rush reviewer recommended IMPLEMENT; orchestrator escalated (persisted index format change) |
| 2026-10-29 | P2-06 / RV-INDEX-605EEC5D | ESCALATE | ESCALATE (orchestrator): the reviewer recommended a coordinated persisted-identity redesign with a rebuild migration; because it changes the persisted format of unique, regular, FTS and functional postings, it needs owner/@oh sign-off before any code. Reviewer proposal: Specify persisted token/value identities as explicit portable byte encodings independent of Hasher implementations, native widths, and dependency versions; retain Fx hashing only for in-memory acceleration. Coordinate legacy and separate index2 identity versions with the collision and functional Map fixes. Missing markers mean… | rush reviewer recommended IMPLEMENT; orchestrator escalated (persisted index format change) |
| 2026-10-29 | P2-05 / RV-INDEX-8533E947 | IMPLEMENT | Validate used basenames as nonempty portable single filename components before file creation, rejecting traversal, separators, absolute/drive/UNC paths, NULs and alternate-stream syntax; allow unused empty qbasename. Add one snapshot-local versioned integrity companion covering exact manifest and sidecar bytes using existing CRC, without changing their layouts or shared MetaEnvelope. Publish it atomically with every manifest update. Missing means legacy validation; invalid present records fail closed. CI covers historical v1/current v2, metadata corruption and both path branches. CRC is not a… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-07 / RV-INDEX-95108B6D | IMPLEMENT | Add a 16 MiB clone-shared per-manager accounted cache budget alongside the existing 512-entry policy. Charge resident keys, RecordId slices and fixed entry/Arc metadata; exclude and document allocator/hash-table overhead and caller-held result Arcs. Atomically reserve before publication, roll back failures and release exactly once on replacement, eviction, all removals and DROP retain. Use bounded arbitrary eviction without nested shard guards; bypass caching when oversized or unable to reserve. Preserve generation/drain semantics and returned results. CI must cover concurrent budget enforcem… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-05 / RV-INDEX-9E0EC9A5 | IMPLEMENT | Add fallible persisted quantizer reconstruction while retaining the existing public to_quantizer signature. Validate positive matching dimensions, parameter lengths, finite parameters, nonnegative scales including zero, vector/code widths and persisted build parameters before construction. Return Corrupt rather than assert and contain parser panics on both graph branches. Share integrity coverage with 8533E947. CI includes malformed legacy and correctly checksummed malformed new metadata, independently proving semantic validation; coordinate existing test relocation. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-06 / RV-INDEX-9E472725 | ESCALATE | ESCALATE (orchestrator): the reviewer recommended a coordinated persisted-identity redesign with a rebuild migration; because it changes the persisted format of unique, regular, FTS and functional postings, it needs owner/@oh sign-off before any code. Reviewer proposal: Replace authoritative dual hashes with framed canonical-value tuple identity across regular/unique postings, probes, and transaction reservation/release keys. Preserve current numeric normalization, missing-field behavior, and NaN exclusion; recursively canonicalize unordered containers. Bump the legacy format and rebuild unde… | rush reviewer recommended IMPLEMENT; orchestrator escalated (persisted index format change) |
| 2026-10-29 | P2-06 / RV-INDEX-FBB72544 | ESCALATE | ESCALATE (orchestrator): the reviewer recommended a coordinated persisted-identity redesign with a rebuild migration; because it changes the persisted format of unique, regular, FTS and functional postings, it needs owner/@oh sign-off before any code. Reviewer proposal: Canonicalize Map entries by sorted canonical key identity before recursively encoding/hashing values; do not use XOR as the replacement. Preserve numeric normalization and List ordering. Include functional postings in the coordinated index2 identity-version migration, with drop plus real record backfill rather than its current… | rush reviewer recommended IMPLEMENT; orchestrator escalated (persisted index format change) |
| 2026-10-29 | P2-19 / RV-NUMA-6FDBD35C | IMPLEMENT | Add process-wide OnceLock-backed detect_cached() returning a shared Arc, retaining detect() as fresh probing. Switch default index constructors, deserialization, and clone paths to cached topology and include shamir-index consumers in scope. Cache successful topology or fallback for process lifetime; topology changes require fresh detection/injection or restart, not mutation of existing replicas. Coordinate once-only fallback diagnostics. Injected-probe CI must prove one initialization, shared Arc identity, cached fallback, and independent fresh calls. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-14 / RV-QUERY-BUILDER-9E172C69 | IMPLEMENT | Add Batch::try_into_request(self) -> Result<BatchRequest, BuildError>, sharing validation with try_build and moving all request fields after successful validation. Preserve existing borrowed build, try_build, and encoding APIs and their behavior. After P2-17, add SDK execute_owned(Batch) using the consuming path while retaining execute(&Batch), existing error mappings, and response handling; authorize this SDK integration outside the brief's current crate scope. Reuse the reference collector updated by 8BEB53C0. CI regressions must prove byte-identical valid requests, identical invalid-batch… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-14 / RV-QUERY-BUILDER-DE20E96B | IMPLEMENT | Authorize adding the existing workspace dependency thiserror 2.x to shamir-query-builder and migrate exactly BuildError, BuilderError, QueryBuildError, CreateIndexBuildError, and ResponseError to derives. Preserve public variants, fields, existing derives, exact Display messages, and source behavior, including ResponseError::Deserialize exposing its WireError source; add no conversion impls. Add CI compatibility coverage for every variant's message and source chain. Record this as convention cleanup, not an error-handling defect, and leave the additional manual WireError implementation outsid… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-14 / RV-QUERY-BUILDER-FE2003D1 | IMPLEMENT | Choose documentation and contract coverage only. Preserve RU7: lit_u64 continues emitting Int through i64::MAX and canonical decimal String above it. Document that its equality guarantee does not imply representation-independent ordering, ranges, or membership: String/String ordering remains lexical, Big/String ordering remains numeric, and Between/In/InSet retain their current scalar-extraction limits. Add CI contract coverage across tree Big, decimal-string, and raw-uint64 lens storage, using 9999999999999999999 versus 10000000000000000000, inclusive range boundaries, and literal/dynamic me… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-14 / RV-QUERY-BUILDER-MACROS-02993B77 | ESCALATE | Owner must approve the new third-party dependency proc-macro-crate in shamir-query-builder-macros; recommend approval rather than a custom Cargo-manifest resolver. After approval, resolve the consumer builder crate root once per expansion and thread it through every filter and query lowering path, including nested filters and write-document lowering. Preserve canonical-name consumers, existing syntax, source spans, and builder-only expansion. Add an alias-only consumer fixture exercising filter! and all six q! forms without direct DTO dependencies. Until approval, leave implementation blocked… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-13 / RV-QUERY-TYPES-182C703E | SKIP | Record accepted convention/API debt: retain manual error formatting and public String-returning validators in this sprint. No incorrect classification behavior is established, thiserror is not a dependency of shamir-query-types, and dependency/API churn is not justified solely by convention. Keep the fallible conversion fix independent of this cleanup. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-13 / RV-QUERY-TYPES-4F767AA5 | IMPLEMENT | Add FilterValue::try_from_query_value returning a structured conversion error, reusing direct literal conversion and the authoritative H5 decoder. Preserve valid markers and existing numeric projections, including RU7 decimal strings. Migrate repository production conversions and propagate errors, including unsupported maps nested in lists. Preserve From<QueryValue> only as an explicitly documented legacy lossy compatibility adapter; do not claim its fallback is eliminated or add conflicting TryFrom<QueryValue>. CI must prove errors rather than Null/panics for unsupported and conflicting maps… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-13 / RV-QUERY-TYPES-5F9D5468 | IMPLEMENT | Keep bytewise equality for two IdBytes rows, return false when exactly one operand is IdBytes, and preserve existing Direct/Inserted field-value equality. Leave as_value and serialization unchanged. Add a three-value regression with two distinct opaque byte sequences and Direct(Null), checking symmetry, opaque equality/inequality and preserved Direct/Inserted equality; keep the scalar-accessor optimization independent. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-13 / RV-QUERY-TYPES-CC015A3C | SKIP | Record accepted allocation cost: retain public owned String repository fields and main defaults. The allocations exist, but the brief establishes no measured bottleneck to justify ownership/API changes. Continue the independent planner allocation reductions without changing these DTO contracts. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-13 / RV-QUERY-TYPES-F05184D8 | IMPLEMENT | Deserialize fields through QueryValue::deserialize and return InsertedRecord with id None, accepting supported non-map values as well as maps. Preserve serializer bytes, sorted map emission, decoded _id fields and the existing non-map-with-id envelope; do not unwrap envelopes. CI must prove id-less scalar, null, binary and list round-trips plus unchanged map/_id behavior. Coordinate removal of the obsolete visitor-local import with import cleanup. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-17 / RV-SDK-723F5FD2 | IMPLEMENT | Document the existing interoperable ABI as nonnegative signed-i32 pointer/length fields with SDK exchange buffers in the lower 2 GiB and bounds-checked ranges. Preserve signatures, negative-address rejection and existing memory configuration limits. Explicitly distinguish the final packed-result reader's unsigned extraction rather than claiming all paths already reject high-bit pointers. Add CI boundary fixtures for negative/high-bit signed fields, valid ranges and out-of-bounds ranges without multi-gigabyte allocation. Coordinate host documentation/fixtures after P2-16 and align SDK response… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-17 / RV-SDK-77261152 | IMPLEMENT | Choose signature-only scalar documentation: scalar receives one Params argument and no injected Ctx, but is not a purity or security boundary. Preserve Ctx construction and current host authorization/capabilities; add no scalar-specific import denial. Add a consumer compile fixture constructing Ctx inside a valid scalar without executing imports, retain invalid-signature rejection, and coordinate documentation/tests with RV-SDK-MACROS-44A9CC7A. Do not characterize construction as unauthorized host access. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-17 / RV-SDK-E3875787 | IMPLEMENT | Restrict packed pointer-result production and generated raw-pointer guest entrypoints to wasm32, preserving WASM export names, signatures and packed layout. Preserve native SDK/macro compilation and use typed encode/decode helpers and native-width ownership interfaces for native tests; never pack native allocations into guest pointer fields. CI must prove native consumers compile without executable guest pointer exports and WASM consumers retain the exact ABI. Coordinate target gating with response-range validation and retain native-width ownership tests. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-17 / RV-SDK-MACROS-44A9CC7A | IMPLEMENT | Retain type_contains_ctx only as a best-effort signature diagnostic and remove semantic-purity claims. Document that generated concrete Params forwarding and Rust type checking determine signature compatibility, not lexical token matching. CI consumer fixtures must accept a Params alias and body-local Ctx construction, reject an SDK Ctx argument alias through type checking, and preserve the direct-Ctx diagnostic. Use the signature-only policy from RV-SDK-77261152; add neither semantic alias resolution nor capability enforcement. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-17 / RV-SDK-MACROS-C8189C06 | IMPLEMENT | Support optional crate = path on all four SDK attributes, defaulting to ::shamir_sdk, and substitute the parsed path throughout generated SDK references. Reject unknown options, duplicate crate options and malformed arguments while preserving no-argument behavior. Use existing syn/quote dependencies, not a new resolver dependency or Cargo-manifest discovery. CI must compile canonical and renamed-only consumers for every attribute and reject unsupported arguments; coordinate expansion edits with native ABI gating. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-18 / RV-SERVER-97C240D7 | IMPLEMENT | Add --bootstrap-password-stdin, mutually exclusive with argv password and skip-bootstrap. Read one bounded UTF-8 credential line, strip only LF or CRLF, reject empty/oversized input without echo, and use zeroizing storage. Preserve argv and random-token behavior; discourage argv in help with explicit process-list/history exposure. CI must prove whitespace and line-ending semantics, EOF handling, invalid/oversized rejection, argument conflicts, and unchanged existing modes. Evidence: crates/shamir-server/src/main.rs:48–56,411–425. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-18 / RV-SERVER-A65118CC | IMPLEMENT | Align server validation with the existing transport policy: accept IPv4-mapped IPv6 only when its mapped IPv4 is loopback in 127/8. Keep separate helpers, update diagnostics, and contract-test both paths for native/mapped loopback, mapped non-loopback, unspecified, and non-mapped IPv6-compatible addresses. Do not export a new shared API. Evidence: crates/shamir-transport-tcp/src/listener.rs:51–55; crates/shamir-server/src/boot/config.rs:939–942,991–995. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-18 / RV-SERVER-E8E1D11B | ESCALATE | Owner must choose whether existing unpinned replication subscriptions retain explicit compatibility-mode TOFU or require enrollment before their next connection. Recommend persisted per-upstream out-of-band pins, enforcement on every fresh connection, fatal mismatch without automatic relearning, and explicit operator-approved rotation. Preserve wire bytes and coordinate P2-09/P2-10. Password-derived and identity authentication already exist; missing continuity is not unauthenticated replication. After policy selection, CI must cover enrollment, restart/reconnect continuity, mismatch, and appr… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-08 / RV-STORAGE-0A9E3A41 | IMPLEMENT | Fix worker startup only: make WriteWorker::spawn fallible and propagate startup failure through insert and nonempty transact using existing DbError::Io. Preserve lazy, single-worker initialization under concurrent first use; a private OnceLock<Result<WriteWorker, std::io::Error>> may cache startup failure, with propagated owned I/O errors preserving kind and source. Inject startup failure in CI tests and prove both methods, including subsequent attempts, return errors without panic or writes. Defer general source-chain redesign; preserve existing public error variants, conversions and wire co… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-08 / RV-STORAGE-1A5A9374 | IMPLEMENT | Clarify ordering documentation only: one worker executes jobs FIFO in actual channel-admission order, not concurrent invocation order. There is no universal submission order across handles, worker jobs, blocking routes or repository batches; callers needing sequencing must await preceding operations. Correct transact's every-point-write ordering claim. Preserve routes, visibility and durability behavior, and do not imply atomic reader visibility. No behavioral regression test is required. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-08 / RV-STORAGE-38F208C4 | IMPLEMENT | Approve documented probabilistic uniqueness, not collision-safe insertion. Replace uniform-128-bit and provably-pointless-probe claims with the actual relative-timestamp plus 64-bit Xoshiro-tail layout and explicitly accept collision/overwrite risk; qualify probability estimates by equal timestamp prefixes and the random-tail model. Preserve generation, encoding and insertion behavior; do not add a check-then-insert probe. Documentation-only: no behavioral regression test is required. Coordinate any record_id.rs documentation edit with P2-12. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-08 / RV-STORAGE-88C4F2CC | SKIP | Defer interface-polish debt: the private stream alias is externally expressible, and changing Bytes bounds, copy signatures or public error payloads adds compatibility cost without a demonstrated runtime benefit. Preserve those APIs. Apply copy semantics and worker-start decisions under their dedicated cards; retain separately scoped alias-reuse and documentation cleanup without duplicating them here. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-08 / RV-STORAGE-9478B37E | IMPLEMENT | Choose explicit partial-copy semantics: copy_store incrementally merges/upserts into the destination, preserves unrelated keys, provides no snapshot or rollback guarantee, and may leave partial results on error or cancellation. Do not delete caller-owned destinations. Extend contract tests to prove completed batches, pre-existing unrelated keys and matching-key overwrites survive a later failure. Record rename retry contamination as unremediated and revise P2-08's recovery acceptance criterion; a rename-owned recovery protocol requires a separately scoped engine/database change. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-08 / RV-STORAGE-CC5C79BA | IMPLEMENT | Add one Fjall-local store-name validator accepting nonempty names of at most 255 UTF-8 bytes, without character filtering or normalization. Validate ordinary get/delete, atomic staging and every commit name before keyspace resolution; return DbError::Validation. Preserve valid existing names and other backends' contracts. CI tests cover empty, 255/256-byte and multibyte names, accepted punctuation, and mixed valid/invalid batches with no keyspace creation or writes. Confirm this compatibility boundary against locked Fjall 3.1.10; locally inspected 3.1.6 source is not evidence of the locked im… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-08 / RV-STORAGE-FB7C0794 | SKIP | Accept current allocation exposure pending a demonstrated ingress path and compatibility-backed size policy. KeyBytes constructors and serialization accept arbitrary lengths; a Deserialize-only cap would reject previously round-trippable keys and cannot universally prevent allocation inside the underlying decoder. No remote exploit path is established by this review. Preserve encoding and accepted lengths; bounded diagnostics remain an independent change, not justification for a universal key-size cap. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-18 / RV-TRANSPORT-IPC-FA481516 | SKIP | Record accepted conditional publication-before-chmod exposure and recommend an effective-user-owned 0700 socket parent. Preserve the bind contract, process-global umask, and publication mechanism. Existing parent ownership/non-writability checks do not require owner-only traversal or eliminate the interval; chmod does not revoke established connections. Evidence: crates/shamir-transport-ipc/src/unix.rs:66–75,75–111; docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:57–59. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-18 / RV-TRANSPORT-TCP-4D95A5F8 | IMPLEMENT | Add explicit-cap companions for all three data writers; retain existing default-cap wrappers and unchanged close frames. Enforce caller cap and u32 wire-length capacity before allocation or writing, preserving default-wrapper error precedence and zeroizing/pooled storage. Do not introduce negotiation or broad caller migration. Coordinate buffer reuse. CI must prove boundaries, zero-write rejection, pre-reserved validation, and byte-identical wrappers. Evidence: crates/shamir-transport-tcp/src/framing.rs:183–201,225–244,278–306. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-18 / RV-TRANSPORT-TCP-9C706A16 | IMPLEMENT | Replace production Tokio full with net and io-util; declare macros, rt, rt-multi-thread, sync, and time as dev-only capabilities for existing tests and benchmark. Require isolated library and all-target CI checks rather than relying on workspace feature unification. Claim no measured size or compile-time gain. Evidence: crates/shamir-transport-tcp/Cargo.toml:14; crates/shamir-transport-tcp/src/listener.rs:18; crates/shamir-transport-tcp/src/framing.rs:12; crates/shamir-transport-tcp/src/tests/framing_tests.rs:65,158. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-18 / RV-TRANSPORT-WS-460A02D6 | IMPLEMENT | Remove unused hex, serde, serde_bytes, and rmp-serde dev-dependencies and their empty section. Crate-wide Rust search finds no consumers; keep new origin tests dependency-free. Existing isolated all-target CI checks suffice without a new behavior test for removal itself. Evidence: crates/shamir-transport-ws/Cargo.toml:31–35. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-18 / RV-TRANSPORT-WS-5D3BB859 | IMPLEMENT | Return fixed HTTP 403 body origin not allowed while preserving internal OriginRejected::NotAllowed(String) diagnostics and missing-origin HTTP 400 behavior. CI must assert the exact fixed body, absence of a distinctive supplied origin, unchanged status, and retained typed rejection. Implement with B1243564; classify as response hardening, not a demonstrated rendering exploit. Evidence: crates/shamir-transport-ws/src/server.rs:253–264; crates/shamir-transport-ws/src/browser.rs:103. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-18 / RV-TRANSPORT-WS-B1243564 | IMPLEMENT | Distinguish absent Origin from present non-text HeaderValue: absence remains Missing/400; conversion failure becomes existing NotAllowed with a fixed invalid-header diagnostic and HTTP 403. Never reflect raw bytes; reuse 5D3BB859's fixed body. CI must send obs-text and assert NotAllowed/403 alongside absent-header Missing/400, with both upgrades rejected. Evidence: crates/shamir-transport-ws/src/server.rs:244–257; crates/shamir-transport-ws/src/browser.rs:99. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-18 / RV-TRANSPORT-WS-F0A54173 | IMPLEMENT | Add fallible BrowserOriginPolicy::try_allow for server configuration and launch; retain allow as a compatible wrapper whose malformed entries never match. Validate HTTP(S) serialized tuple origins and optional ports; reject userinfo, paths, queries, fragments, whitespace, lists, null, and literal wildcard hosts. Allow exactly one whole leftmost DNS wildcard label, matching scheme and port, without broad normalization. Use existing dependencies and coordinate policy sharing. CI must cover valid DNS/IP/IPv6 origins, ports, exact/one-level matching, apex/deeper rejection, invalid configuration,… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-02 / RV-TX-4A1FDAAE | SKIP | Record accepted allocation debt. Keep the public RecordChange.table: String field and serialized string values unchanged: Arc<str> would break Rust source compatibility, and no measured benefit justifies a parallel event representation in this sprint. Journal batching is independent. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-02 / RV-TX-56651C4A | IMPLEMENT | Add an object-safe event-only put_many with an ordered default returning one result per input. Batch consecutive Event messages within count/byte limits, flushing before every control message and serialization failure; preserve FIFO order, per-event gaps and successful-write watermark accounting. Add the production StoreChangelog batch override and preserve clean-horizon durability boundaries. Extend ownership to the engine journal adapter and coordinate with P2-04. CI proves storage batching, partial failures and all control boundaries. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-02 / RV-TX-782352E1 | IMPLEMENT | Reject reserved empty payloads on all ordinary versioned-set variants using an existing invalid-input error. Prevalidate entire batches before allocating versions or publishing cells; keep empty batches as no-ops and explicit delete/recovery tombstones unchanged. Preserve persisted encoding. CI proves invalid single and mixed-batch writes consume no versions and change no history/overlay state, while explicit deletion remains functional. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-02 / RV-TX-ED8562B0 | IMPLEMENT | Normalize constructor seeds to max(seed, 1) in RepoWalManager and RepoTxGate without signature changes. Preserve positive seeds, monotonic recovery floors, persisted entries and implicit/nontransactional external tx_id zero. CI proves both zero-seeded allocators first issue one, positive seeds remain exact and event projection retains intentional external zero semantics. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-12 / RV-TYPES-5E4BFC76 | IMPLEMENT | Document from_bytes<T> as providing no recursion protection: permit statically nonrecursive roots, or require a type-specific decoder enforcing depth during deserialization for recursive untrusted roots. Distinguish depth from byte/allocation limits. Keep the generic signature and bincode encoding unchanged; do not add post-decode depth walks or claim a demonstrated recursive production exploit. Preserve the nonrecursive interner chunk format while coordinating hydration validation. Documentation only; no new behavioral test required. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-12 / RV-TYPES-A7B1FCEC | IMPLEMENT | Add borrowed resource-path views and nearest-first ancestor iteration, retaining owned ResourcePath, parent, and ancestors as compatibility wrappers. Adapt metadata resolution and ACL memo probes to borrowed paths; allocate ownership only for cache insertion or denial output. Support folder slices and function-name prefixes. Include shamir-db in scope and rebase on P2-01. Preserve awaited order, first denial, fail-closed errors, admin bypass, and diagnostic escaping; CI must compare all path variants against the owned traversal contract. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-12 / RV-TYPES-EFA1B8F4 | IMPLEMENT | Add unary-minus numeric-literal object-value arms before generic token-tree arms, forwarding to the existing negative-literal conversion. Preserve arbitrary expressions as explicit @expr escapes and retain existing container syntax. CI must compile downstream object uses with negative integers/floats in first, middle, final, and nested positions, with and without trailing commas, and assert resulting values. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-12 / RV-TYPES-FE40CB54 | IMPLEMENT | Use safe String::zeroize() in crypto-gated Drop. Retain ordinary PartialEq/Eq and document that equality is not constant-time and must not implement authentication. Preserve feature gating, redacted Debug, serde bytes, cloning, and into_inner ownership transfer; add no dependency or comparison API. CI must cover crypto on/off, empty and Unicode strings, ownership transfer, and wiping through a valid live-buffer seam without reading freed memory. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-11 / RV-WAL-0843B96B | SKIP | Record accepted public-namespace debt. Keep segment_meta publicly importable: removing an empty public module still breaks downstream imports without runtime benefit. Do not widen private helpers for documentation. Reconsider only during explicitly compatibility-breaking API cleanup, independently of sidecar correctness repairs. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-11 / RV-WAL-3F10E8A1 | SKIP | Record accepted operator-diagnostic path exposure at the embedded library boundary. Preserve WAL path context and typed I/O causes; no specific remote WAL disclosure route has been established, and ticketed append display errors are already path-free. Sanitize only at a demonstrated external transport boundary rather than weakening WAL diagnostics or the P1 typed-I/O policy. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-11 / RV-WAL-5DFE355F | SKIP | Record accepted legacy public test-hook surface, not a supported administrative contract. Keep mark_poisoned unchanged: restricting it breaks public callers, repository-only consumer review cannot exclude external use and misuse requires an in-process segment holder. Avoid API churn for this nit while fixing actual append/rollback concurrency behavior. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-11 / RV-WAL-A25EDE9D | SKIP | Record accepted backlog-dependent decoded-memory debt and schedule a cross-crate bounded-recovery project. A cursor alone cannot preserve startup commit-version sorting, interner-before-finalization ordering, full-entry cold journal certification and live window reseeding with bounded memory. Those changes exceed this WAL-only brief. Continue CC3131A9 incremental frame parsing, but do not claim end-to-end bounded recovery. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-11 / RV-WAL-DD492F5C | IMPLEMENT | Document that data and sibling WAL directories, sidecars, parent-directory replacement and restored copies are writable only by the database identity and trusted operators/tooling. Specify restrictive Unix permissions and Windows ACLs. Explain that CRC detects corruption but does not authenticate writers and that TLS/volume encryption do not provide WAL authenticity. Update WAL crate docs and the security data-protection guide; authorize that documentation scope extension. No format/runtime/dependency changes or behavioral tests are required. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-16 / RV-WASM-HOST-4669FDA6 | IMPLEMENT | Add checked FunctionMeta::try_from_record, retaining from_record as a legacy compatibility API but migrating both production catalogue/security callers. Write separate metadata_version=1; absent version selects legacy decoding with defaults only for absent fields. Reject malformed present fields, non-string grants, non-map records, and unsupported versions. Decode before recovery registration and return contextual initialization errors; malformed metadata in effective_fn_actor retains the caller without setuid escalation. Preserve artifact revision version. Coordinate DB scope with P2-01. CI… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-16 / RV-WASM-HOST-98551A44 | IMPLEMENT | Add engine options for an explicit pooling memory ceiling, defaulting to the current 64 MiB, and preserve existing constructors as wrappers. Track actual allocator mode and effective ceiling; reject above-ceiling limits in both WAT and binary constructors with requested/available bytes and remediation. Do not apply a pool-derived rejection to on-demand engines or pooling fallback. CI must prove early rejection, growth above 64 MiB with a larger configured pool, and unchanged on-demand behavior. Land before P2-17. Evidence: crates/shamir-wasm-host/src/wasm/wasm_engine.rs:101–121,204; crates/sh… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-16 / RV-WASM-HOST-C17D12C8 | IMPLEMENT | Add an explicit SDK-directory compile option and SHAMIR_WASM_SDK_DIR fallback, with explicit configuration taking precedence and invalid explicit settings producing actionable errors. Preserve existing entry points and the checkout-relative development fallback. Support a trusted version-matched SDK source bundle including sibling path dependencies; do not introduce registry downloads. Safely escape manifest paths and coordinate native command arguments with 796ECAE4. CI must prove precedence, invalid-bundle errors, and compilation without the original checkout. Evidence: crates/shamir-wasm-h… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-16 / RV-WASM-HOST-EB1654FA | IMPLEMENT | Add a bounded-capture supervisor companion in shamir-proc-tree and use it for guest Cargo, preserving the existing supervise API and other callers' retention semantics. Retain at most 64 KiB per pipe plus a fixed truncation marker, continuously draining excess output to EOF. Preserve process-tree termination, bounded drain/reap, and teardown diagnostics. Expand the brief scope to shamir-proc-tree. CI must cover oversized simultaneous pipes, truncation, normal exit, timeout, and inherited-pipe descendants. Evidence: crates/shamir-proc-tree/src/supervise.rs:68–77,137–190; crates/shamir-wasm-hos… | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-16 / RV-WASM-HOST-EBD32CA5 | IMPLEMENT | Remove only the unused direct serde dependency. Crate-wide Rust search finds prose mentions, not consumers. Preserve SDK-generated manifest behavior and transitive dependencies; make no measured build-savings claim. Existing isolated all-target CI checks suffice without a new behavioral test. Evidence: crates/shamir-wasm-host/Cargo.toml:15; crates/shamir-wasm-host/src/compile.rs:61,574. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-16 / RV-WASM-HOST-FE02BB1D | IMPLEMENT | Map actual task cancellation to existing Cancelled and task panic to existing Compute, preserving Argon2id context and string panic payloads with a stable non-string fallback. Do not add an error variant or resume unwinding. CI must inject string/non-string panics and cancellation and verify distinct classification while retaining ordinary KDF behavior. Evidence: crates/shamir-wasm-host/src/builtin/argon2id.rs:78–80; crates/shamir-wasm-host/src/runtime/error.rs:32–34,54–56. | rush reviewer (read-only), accepted by orchestrator |
| 2026-10-29 | P2-16 / RV-WASM-HOST-FF7C2D6E | SKIP | Record as superseded by component cards, not accepted unresolved correctness debt. Weak ticker ownership and the shared gateway matcher already exist. Complete native path arguments under 796ECAE4 and checked metadata under 4669FDA6; authorize no duplicate implementation or regression suite. Evidence: crates/shamir-wasm-host/src/wasm/wasm_engine.rs:164–172; crates/shamir-wasm-host/src/gateway/net_gateway.rs:20; crates/shamir-wasm-host/src/compile.rs:701–703; crates/shamir-wasm-host/src/builtin/meta.rs:124–141. | rush reviewer (read-only), accepted by orchestrator |

## Execution log

Append one line per event (launch, review verdict, CI run, merge, cleanup).

| Date | Task | Event | Detail |
|---|---|---|---|
| 2026-10-29 | sprint | board created | 19 tasks + P2-U01 from the revalidation at master `7f04e096`; gate #1593 closed |

## Landing procedure (upstream)

Used for each wave once the fork CI run on the integration branch is green:

1. Disable the upstream `CI` workflow for the duration of the window.
2. Push the same verified SHA to the upstream repository and open the PR (squash merge only; linear history).
3. Post the attested commit status `merge gate (required)` = success on that SHA, referencing the green fork run.
4. Relax the `master-delivery` ruleset by dropping the `integration_id` of the required check, squash-merge the PR, then ALWAYS restore the ruleset and verify it equals the saved original.
5. Re-enable the upstream `CI` workflow, delete the integration branch and the task worktrees and branches.

If the pre-push hook refuses a push to the fork, do not retry and never bypass it: push to the upstream repository instead.
