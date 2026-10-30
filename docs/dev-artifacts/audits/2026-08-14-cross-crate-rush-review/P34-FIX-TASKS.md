# P34 fix plan - execution board

Companion of [P3-REVALIDATION-2026-10-30.md](P3-REVALIDATION-2026-10-30.md) and [P4-REVALIDATION-2026-10-30.md](P4-REVALIDATION-2026-10-30.md) (verified verdicts, evidence and recipes at master `07bc804c`), [P3.md](P3.md) / [P4.md](P4.md) (the original cards) and [P2-SPRINT-REPORT-2026-10-30.md](P2-SPRINT-REPORT-2026-10-30.md) (the P2 carry-overs). Every one of the 100 active P3/P4 cards (P3 63, P4 37; all risk low or nit) belongs to exactly one task below; 14 P2 cards that stayed open and one new P2 finding ride along in the stage of their crate. Status: `[ ]` todo, `[~]` executor running, `[r]` in review, `[c]` in CI, `[x]` merged, `[-]` dropped after re-check.

## Managing the plan

- This file is the source of truth. Session tasks are labelled `[P34-NN]` and are only pointers; the briefs in `docs/dev-artifacts/prompts/remediation/p34/` carry the per-card detail and the decisions. Update the row and the tracking block of a task in the same step that changes its state.
- Standing order of the owner (2026-10-30): solve the tasks; do not ask questions that have a technical answer. Owner-decision cards were given to the design decider (@oh) on 2026-10-30: 47 cards, of which 19 IMPLEMENT, 25 SKIP (accepted as they are) and 3 ESCALATE. Only the three escalations go to the owner (below).
- After a task is merged: re-decide its cards with one read-only reviewer pass, update the re-validation tables, tick the box, remove its worktree and branch.

## Rules for every task

- Own worktree and branch (`worktrees/p34-NN-<slug>`, branch `fix/p34-NN-<slug>`); the brief is committed before the executor is launched.
- Compilation, builds and tests run only on GitHub CI (in the fork rmyva/shamir-db, then landed upstream). Locally only `cargo fmt -p <crate> -- --check`, `cargo clippy -p <crate> --all-targets -- -D warnings`, `cargo check` of dependents when a public signature changed, and `python -I scripts/ci/check_layout.py`. Every brief repeats this verbatim, including the ban on git mutation by the executor.
- Every behavioral fix ships with a regression test that fails on the old code (named after the card id, red first in CI). Documentation-only cards need no test.
- No new dependency, no `#[allow]`, no visibility widening beyond the card, layout limits (7 entries per folder, 1000 lines per file), `mod.rs` re-exports only, imports at the top, tests in `tests/` dirs, multi-line JSON literals in tests.
- A card found already fixed or wrong is reported with evidence, not forced. A card with Decision SKIP is not changed.
- Concurrency cap: at most 5 rush sessions or decision agents at the same time, counting executors, reviewers and decision readers.
- CI for a wave: drafts run the fast tier; the single full-matrix run per wave is requested with the `[ci full]` marker on the head commit of the integration PR (see CLAUDE.md, CI tiers).

## Pipeline per task

1. Brief is final; owner answers recorded (or knowingly skipped). Record it in the tracking block.
2. Executor (rush, own worktree) with the brief; no builds or tests.
3. Orchestrator reads the diff: scope, layout, rules, tests that fail on the old code, no silent behavior change. Local gates are re-run on the touched crates.
4. Independent read-only reviewer on the branch (verdict first; only a concrete reachable defect or compile error blocks). Follow-up rounds until PASS or PASS_WITH_NOTES.
5. Merge the accepted branches of the wave into one integration branch, push it to the fork, open a draft PR and run the full matrix once (marker commit).
6. A failing test or flake is read from the CI log, fixed in code and re-pushed (never reproduced locally).
7. Land upstream with the manual-status procedure, clean up worktrees and branches, tick the box, run the post-merge re-decide.

## Waves and order

| Wave | Tasks | Why |
|---|---|---|
| 1 | P34-01, P34-02, P34-03, P34-04, P34-08, P34-09 | Hub and leaf crates, pairwise disjoint files |
| 2 | P34-05, P34-06, P34-07 | Depend on wave 1 (shared files): client after connect, db/engine after wal/storage/types, index after storage |
| 3 | P34-10, P34-11, P34-12 | Depend on waves 1-2: query after db/engine, sdk/wasm after db/engine, index identity v2 after index |

Order inside a wave is by free slot (cap 5). P34-12 starts with a read-only design and an owner question before any code.

## Shared files between tasks (conflict matrix)

Derived from the evidence paths of the cards at the verification commit; an executor may touch more files, so treat this as a floor.

| Tasks | Shared files (cards) |
|---|---|
| P34-06 / P34-08 | `crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs` (P34-06: A7B1FCEC; P34-08: 71AFA12F) |
| P34-07 / P34-12 | `crates/shamir-index/src/backends/fts_ranked_backend.rs` (P34-07: F39434C2; P34-12: 53CEF405) |
| P34-07 / P34-12 | `crates/shamir-index/src/fts/tokenizer.rs` (P34-07: 0695AADA; P34-12: 53CEF405, 605EEC5D) |
| P34-07 / P34-12 | `crates/shamir-index/src/model/meta/persistence.rs` (P34-07: CF86432E; P34-12: 605EEC5D) |

Cross-crate edits (a card whose fix lives in another stage's crate) are listed in the Scope section of each brief; the AFTER column below orders those stages.

## Board

| Box | ID | Slug | Wave | After | Session | Risk | Cards | P2 tails | Effort | OD | Decisions (I/S/E) | Branch | PR / fork run | Reviewer | Landed |
|---|---|---|---:|---|---|---|---:|---:|---:|---:|---|---|---|---|---|
| [ ] | P34-01 | wal | 1 | - | #1716 | low:4 nit:5 | 9 | 0 | 18 | 6 | 4/2/0 | - | - | - | - |
| [ ] | P34-02 | tx | 1 | - | #1717 | low:6 nit:2 | 8 | 0 | 20 | 3 | 1/2/0 | - | - | - | - |
| [ ] | P34-03 | storage | 1 | - | #1718 | low:3 nit:4 | 7 | 0 | 21 | 5 | 1/3/1 | - | - | - | - |
| [ ] | P34-04 | connect-server | 1 | - | #1719 | low:5 nit:5 | 10 | 0 | 24 | 7 | 4/3/0 | - | - | - | - |
| [ ] | P34-05 | client-net | 2 | P34-04 | #1720 | low:9 nit:4 | 13 | 0 | 23 | 5 | 1/3/1 | - | - | - | - |
| [ ] | P34-06 | db-engine | 2 | P34-01, P34-03, P34-08 | #1721 | low:8 nit:3 | 11 | 2 | 30 | 5 | 3/2/0 | - | - | - | - |
| [ ] | P34-07 | index | 2 | P34-03 | #1722 | low:2 nit:2 | 4 | 3 | 19 | 2 | 1/0/1 | - | - | - | - |
| [ ] | P34-08 | types | 1 | - | #1723 | low:3 nit:3 | 6 | 4 | 16 | 2 | 1/1/0 | - | - | - | - |
| [ ] | P34-09 | funclib-infra | 1 | - | #1724 | low:4 nit:4 | 8 | 0 | 9 | 0 | 0/0/0 | - | - | - | - |
| [ ] | P34-10 | query | 3 | P34-06 | #1725 | low:9 nit:4 | 13 | 3 | 39 | 7 | 1/6/0 | - | - | - | - |
| [ ] | P34-11 | sdk-wasm | 3 | P34-06 | #1726 | low:6 nit:3 | 9 | 0 | 18 | 3 | 2/1/0 | - | - | - | - |
| [ ] | P34-12 | index-identity-v2 | 3 | P34-07 | #1727 | low:2 | 2 | 2 | 16 | 2 | 0/2/0 | - | - | - | - |

## Owner questions

Only what the design decider could not decide on technical merit. Until an answer is recorded here the card is skipped and stays open.

1. **RV-STORAGE-9478B37E** (P34-03). Rename or create into a table name whose old physical stores were left behind by DROP TABLE / RENAME appears to merge stale rows. (A) delete the leftover destination stores before copy and create (recommended; deletes orphaned data, backend delete coherence must be proven); (B) refuse rename/create when the destination stores are not empty; (C) incarnation-qualified store names (persisted-format migration). Until answered: only the red CI test.
2. **RV-CLIENT-B80C0D09** (P34-05). std::sync::Mutex on the client push-routing state fits none of the three sanctioned exceptions of CLAUDE.md. (A) add a fourth exception "per-connection client demux state" (recommended); (B) rewrite on scc (medium, risks the close-vs-subscribe invariants). Until answered: no change.
3. **RV-INDEX-CF86432E** (P34-07). Legacy inline External vector API keys are rewritten in cleartext on every index-metadata save and nothing consumes them. (A) scrub on load/save with a warning, irreversible (recommended); (B) keep until secret references exist; (C) build secret references now (no consumer). Until answered: keep (B).

Also open for the owner, found by P34-12 design: any incompatible persisted-format change of index identity v2.

## Decisions made by the design decider (2026-10-30)

| Card | Decision | Compat | One line |
|---|---|---|---|
| [L3-RV-CLIENT-35A189A9](P3.md#l3-rv-client-35a189a9) | IMPLEMENT | compatible | In crates/shamir-client/src/client/api.rs (roundtrip REQ_BUF closure), stop calling RequestEnvelopeRef::to_msgpack and stop mapping its failure to ClientError::Protoco... |
| [L3-RV-CLIENT-8997B366](P3.md#l3-rv-client-8997b366) | SKIP | n/a | Documented limitation, consistent with the P2-09 decision to preserve ResumeOptions.ticket: Vec<u8> for struct-literal callers. SecureResumeOptions/resume_secure and t... |
| [L3-RV-CLIENT-B80C0D09](P3.md#l3-rv-client-b80c0d09) | ESCALATE | n/a | Owner must decide the CLAUDE.md sanctioned-mutex policy for SubsInner (crates/shamir-client/src/client/transport.rs:251-260, locked per push frame at :592-595). The pe... |
| [L3-RV-CONNECT-2A423D73](P3.md#l3-rv-connect-2a423d73) | SKIP | n/a | The premise of the remaining facet is wrong. The handler Err(String) path is already collapsed to the fixed code application_error (crates/shamir-connect/src/server/in... |
| [L3-RV-CONNECT-387EAA05](P3.md#l3-rv-connect-387eaa05) | IMPLEMENT | additive | Second tranche of the P2-10 decision; keep its rules: never substitute zero entropy or fabricated time, fail closed, and do no partial publication. (1) shamir-connect:... |
| [L3-RV-DB-C2DCAC96](P3.md#l3-rv-db-c2dcac96) | IMPLEMENT | compatible | Move shamir-query-builder from [dev-dependencies] to [dependencies] of shamir-db (crates/shamir-db/Cargo.toml:99). It is a workspace crate, no new third-party crate, n... |
| [L3-RV-ENGINE-2A18B647](P3.md#l3-rv-engine-2a18b647) | IMPLEMENT | compatible | Option 1, approve the infallible adapter contract and close the validator gap. (a) Document compile_filter and compile_filter_with_context (shamir-engine-query/src/que... |
| [L3-RV-INDEX-0695AADA](P3.md#l3-rv-index-0695aada) | IMPLEMENT | compatible | Continuing the P2 decision for RV-INDEX-0695AADA. Add a crate-private streaming visitor in src/fts/ngram_budget.rs (re-exported or wrapped on TokenizerEnum as pub(crat... |
| [L3-RV-INDEX-605EEC5D](P3.md#l3-rv-index-605eec5d) | SKIP | n/a | Accepted risk, deferred to the scheduled 'index identity v2' follow-up (P2 decisions for 53CEF405/9E472725/605EEC5D/FBB72544). Golden vectors (fts/tokenizer_tests.rs:5... |
| [L3-RV-INDEX-CF86432E](P3.md#l3-rv-index-cf86432e) | ESCALATE | compatible | The owner must decide what happens to legacy inline External vector credentials. Today they are grandfathered and rewritten in cleartext on every index-metadata save (... |
| [L3-RV-INDEX-FBB72544](P3.md#l3-rv-index-fbb72544) | SKIP | n/a | Deferred to index identity v2 (P2 decision for RV-INDEX-FBB72544). Map hashing in functional_backend.rs:182-189 depends on insertion order. Its bytes are pinned by rv_... |
| [L3-RV-QUERY-BUILDER-2A648617](P3.md#l3-rv-query-builder-2a648617) | SKIP | n/a | Accepted, documented cost. Batch::switch already documents O(K^2) lowering (crates/shamir-query-builder/src/batch/batch_refs.rs:273-275, as P2-14 required). A linear f... |
| [L3-RV-QUERY-BUILDER-MACROS-2C459E8E](P3.md#l3-rv-query-builder-macros-2c459e8e) | IMPLEMENT | additive | Add two inherent constructors to CallOp in its owner module crates/shamir-query-types/src/ops/call/call_op.rs, following the TableRef::new/with_repo pattern: CallOp::n... |
| [L3-RV-QUERY-BUILDER-MACROS-9BCA14B1](P3.md#l3-rv-query-builder-macros-9bca14b1) | SKIP | n/a | Documented limitation, accepted. The input is compile-time, author-controlled DSL code. syn::Expr parsing and Drop are themselves recursive with no depth cap (filter_l... |
| [L3-RV-QUERY-TYPES-182C703E](P3.md#l3-rv-query-types-182c703e) | SKIP | n/a | Accepted convention/API debt, same as the P2-13 SKIP. Manual Display/Error (batch_error.rs:300, :459) and the public String-returning validators (retention.rs:41, filt... |
| [L3-RV-QUERY-TYPES-4F767AA5](P3.md#l3-rv-query-types-4f767aa5) | SKIP | n/a | Documented limitation, consistent with P2-13, which explicitly kept From<QueryValue> for FilterValue as a documented legacy lossy adapter (filter_value.rs:198-224; che... |
| [L3-RV-SDK-723F5FD2-SEMANTIC-FACET](P3.md#l3-rv-sdk-723f5fd2-semantic-facet) | SKIP | n/a | Accepted, documented limitation, continuing the P2-17 decision, which has landed. Pointer and length fields are nonnegative signed i32, exchange buffers are range-chec... |
| [L3-RV-SDK-B5ACD67D-FACET](P3.md#l3-rv-sdk-b5acd67d-facet) | IMPLEMENT | compatible | Duplicate top-level fields fail closed: HttpResponse::from_value (crates/shamir-sdk/src/api/http.rs:235-283) returns a codec error when status, headers or body appears... |
| [L3-RV-SERVER-E8E1D11B](P3.md#l3-rv-server-e8e1d11b) | IMPLEMENT | compatible | Docs-only, accepting restart re-enrollment. Add an 'Upstream identity pinning' entry to docs/guide-docs/KNOWN_LIMITATIONS.md section 5 (Replication) and to the operati... |
| [L3-RV-STORAGE-0A9E3A41-SEMANTIC-FACET](P3.md#l3-rv-storage-0a9e3a41-semantic-facet) | SKIP | n/a | Accepted, deliberately deferred diagnostic debt. P2 already made Fjall worker startup fallible and kind/source-preserving (storage_fjall.rs:148-156,541-557), which was... |
| [L3-RV-STORAGE-9478B37E](P3.md#l3-rv-storage-9478b37e) | ESCALATE | breaking | The owner must choose how name-keyed orphaned table stores are handled. Rename copies into the final __data__/__info__/__history__<to> stores (catalog.rs:329-342). DRO... |
| [L3-RV-STORAGE-CCF2090E](P3.md#l3-rv-storage-ccf2090e) | IMPLEMENT | compatible | Pin the v1 persisted bytes of the buffer config and state the schema-change policy. Add golden tests to crates/shamir-engine/src/table/tests/table_core/config/buffer_c... |
| [L3-RV-STORAGE-FB7C0794-SEMANTIC-FACET](P3.md#l3-rv-storage-fb7c0794-semantic-facet) | SKIP | n/a | Accepted allocation exposure, unchanged since P2. KeyBytes::deserialize still materializes a ByteBuf (key_bytes/core.rs:242-247), but no production remote ingress deco... |
| [L3-RV-TRANSPORT-IPC-FA481516](P3.md#l3-rv-transport-ipc-fa481516) | SKIP | n/a | Accepted risk, consistent with the P2-18 SKIP and already documented in docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:61-66, with the recommendation of... |
| [L3-RV-TX-3FC2FA1D-SEMANTIC-FACET](P3.md#l3-rv-tx-3fc2fa1d-semantic-facet) | IMPLEMENT | compatible | Add pub(crate) async fn try_lookup_ts(&self, version) -> DbResult<Option<u64>> in crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs. It returns Ok(None) for DbEr... |
| [L3-RV-TX-4A1FDAAE](P3.md#l3-rv-tx-4a1fdaae) | SKIP | n/a | Accepted allocation debt, same as the P2-02 SKIP. Keep RecordChange.table: String (events.rs:17) and the event wire bytes. The per-record table.clone() (events.rs:196,... |
| [L3-RV-TX-FF6AB063](P3.md#l3-rv-tx-ff6ab063) | SKIP | n/a | Accepted convention debt; the core of the card is fixed. ChangelogStore now uses the typed thiserror ChangelogError (changefeed/feed/error.rs:3-25), so the dependency ... |
| [L3-RV-TYPES-5E4BFC76](P3.md#l3-rv-types-5e4bfc76) | SKIP | n/a | Trusted-root constraint ratified; documented limitation (the P2-12 rustdoc has landed at codecs/basic/bincode.rs:30-33). The only production roots are non-recursive va... |
| [L3-RV-TYPES-FE40CB54](P3.md#l3-rv-types-fe40cb54) | IMPLEMENT | compatible | The comparison contract stays as P2-12 chose it: ordinary PartialEq/Eq, documented as not constant-time, with no new comparison API or dependency. Add only a Drop-disp... |
| [L3-RV-WAL-48749F97](P3.md#l3-rv-wal-48749f97) | IMPLEMENT | compatible | Documentation only. On WalSegment::open (wal_segment.rs:195-204), WalSegment::max_committed (around wal_segment.rs:427) and the max_committed field doc (wal_segment.rs... |
| [L3-RV-WAL-A25EDE9D](P3.md#l3-rv-wal-a25ede9d) | SKIP | n/a | Accepted backlog-dependent decoded-memory debt, unchanged since P2. SegmentSet::replay gathers every segment into one Vec (segment_set.rs:562-577), and its consumers (... |
| [L3-RV-WAL-C55C2D47-FACET](P3.md#l3-rv-wal-c55c2d47-facet) | IMPLEMENT | compatible | Documentation only; closes the card as a documented limitation. On WalGroupCommit::append and append_many (wal_group_commit.rs:180-191, 291-312) and on From<WalAppendE... |
| [L3-RV-WASM-HOST-4669FDA6](P3.md#l3-rv-wasm-host-4669fda6) | IMPLEMENT | compatible | Mark FunctionMeta::from_record (crates/shamir-wasm-host/src/builtin/meta.rs:114-149) #[deprecated(note = "lossy legacy decoder; use FunctionMeta::try_from_record")]. I... |
| [L4-ENGINE-STYLE-DELIVERY-PROVENANCE](P4.md#l4-engine-style-delivery-provenance) | SKIP | n/a | Accepted historical exception. 6c286cc9 (#1173-#1205 remediation campaign) mixed style and substantive edits and is deliberately absent from .git-blame-ignore-revs:1-1... |
| [L4-RV-CLIENT-NODE-31D3BBE5](P4.md#l4-rv-client-node-31d3bbe5) | SKIP | n/a | Accepted and deferred to release preparation, same as the P2-03 SKIP. Cargo 0.1.0-alpha.1 (publish=false) vs npm 0.1.0 does not affect loading: native/index.js:211-214... |
| [L4-RV-CONNECT-7D41FB60](P4.md#l4-rv-connect-7d41fb60) | SKIP | n/a | Aggregate tracker, closed by reference, consistent with the P2-10 SKIP. Component dispositions: C8866EB1 fixed (auth_message.rs:5-6 now names test-vectors/). 850A9AFD ... |
| [L4-RV-CONNECT-E4631CD9](P4.md#l4-rv-connect-e4631cd9) | SKIP | n/a | Accepted API debt, re-affirming the P2-10 SKIP. validate_client_kdf_safe (kdf_params.rs:77-91) has a single production caller, which discards the message and maps it t... |
| [L4-RV-DB-0065D2A2](P4.md#l4-rv-db-0065d2a2) | SKIP | n/a | Already resolved by the P2 decision for RV-DB-0065D2A2, which is in master: wasm_hash is documented as a noncryptographic diagnostic fingerprint and function version=1... |
| [L4-RV-DB-7C59E01B](P4.md#l4-rv-db-7c59e01b) | IMPLEMENT | compatible | Docs-only, continuing the P2 SKIP of RV-DB-7C59E01B. Rewrite the doc comments of ShamirDb::admin_user_locks, group_member_locks and repo_create_locks: the fields (sham... |
| [L4-RV-QUERY-BUILDER-21560B4A](P4.md#l4-rv-query-builder-21560b4a) | SKIP | n/a | Accepted policy: users who import both val and select write qualified val::func/select::func (and val::expr/select::expr) or use explicit import aliases, as documented... |
| [L4-RV-QUERY-TYPES-CC015A3C](P4.md#l4-rv-query-types-cc015a3c) | SKIP | n/a | Accepted allocation cost, same as the P2-13 SKIP. Keep the public owned String repository fields and the "main" defaults (table_ref.rs:21-26, the default_repo helpers)... |
| [L4-RV-SERVER-97C240D7](P4.md#l4-rv-server-97c240d7) | IMPLEMENT | compatible | Keep --bootstrap-password (argv) for compatibility with existing scripts and the e2e harness. When it is used, emit one tracing::warn! at boot, without the value, sayi... |
| [L4-RV-SERVER-A65118CC](P4.md#l4-rv-server-a65118cc) | IMPLEMENT | compatible | Make shamir-transport-tcp the single owner of the plain-profile loopback predicate. In crates/shamir-server/src/boot/config.rs:889, replace !is_loopback_ip(addr.ip()) ... |
| [L4-RV-STORAGE-88C4F2CC](P4.md#l4-rv-storage-88c4f2cc) | SKIP | n/a | Deferred interface polish. The alias, Bytes bounds, copy signature, KeyExists wording and engine-domain errors (types.rs:13-14,476,490-495,631; storage_in_memory.rs:20... |
| [L4-RV-WAL-0843B96B](P4.md#l4-rv-wal-0843b96b) | IMPLEMENT | compatible | Change 'pub use entry::segment_meta;' to 'pub(crate) use entry::segment_meta;' in crates/shamir-wal/src/lib.rs:70. Keep every crate::segment_meta::... path inside the ... |
| [L4-RV-WAL-3F10E8A1](P4.md#l4-rv-wal-3f10e8a1) | SKIP | n/a | Accepted operator-diagnostic path exposure at the embedded library boundary, as decided in P2. Typed I/O errors keep the path plus an io::Error source (wal_segment.rs:... |
| [L4-RV-WAL-5DFE355F](P4.md#l4-rv-wal-5dfe355f) | IMPLEMENT | compatible | Restrict WalSegment::mark_poisoned (wal_segment.rs:493-498) to #[cfg(test)] pub(crate) and change its doc to a crate-test fault hook. Production code keeps poisoning t... |

Two decisions overturn a P2 SKIP: `RV-WAL-0843B96B` (`segment_meta` becomes crate-private) and `RV-WAL-5DFE355F` (`mark_poisoned` becomes a crate-private test hook), because `shamir-wal` has `publish = false` and no other workspace crate uses either item; `cargo check` of the dependents is the proof. `RV-TX-3FC2FA1D` is the only accepted behavior change: `history_of` / `history_of_many` return the storage error instead of `ts_millis = None`.

## Tasks

### P34-01 - shamir-wal: replay bounds, reopen watermark, sink causes, public surface, docs

- Session task: #1716. Brief: `docs/dev-artifacts/prompts/remediation/p34/01-wal.md`. Crates: `shamir-wal`.
- Wave 1; after: none. Cards 9 (low 4, nit 5), effort 18, owner-decision cards 6.

**Goal.** Stop the append path from persisting frames that later hard-fail replay, make the public surface honest (segment_meta and mark_poisoned crate-private, retired-API narratives removed from the module headers), document the reopen watermark and the legacy sink-cause loss, and hoist the remaining test imports. Bounded replay (A25EDE9D) and absolute paths in errors (3F10E8A1) are accepted and not changed.

**Acceptance.**

- A supported-header, invalid-body append returns a typed error and writes nothing; a following replay of the segment is unaffected (regression named after 4470BEF3).
- segment_meta and WalSegment::mark_poisoned are crate-private (mark_poisoned test-only); every dependent still passes `cargo check`; documentation no longer presents them as public.
- Documentation-only cards (48749F97, C55C2D47, 79167DD7) change comments only. C55C2D47 must not add the underlying error text to the returned error: that would put WAL file paths into client-facing errors.

- A25EDE9D names shamir-engine drain_step.rs as evidence but its decision is SKIP: no engine edit in this stage.
- Keep the P0 cancellation/fail-stop ticket semantics and the P1 corruption and typed-I/O policies.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Decision | Title |
|---|---|---|---|---|---|
| [L3-RV-WAL-A25EDE9D](P3.md#l3-rv-wal-a25ede9d) | low | L/code | yes | SKIP | replay materializes the entire WAL as decoded entries in one Vec |
| [L3-RV-WAL-4470BEF3](P3.md#l3-rv-wal-4470bef3) | low | M/code | no | - | Append path accepts payloads that produce well-formed frames which then hard-fail replay |
| [L3-RV-WAL-48749F97](P3.md#l3-rv-wal-48749f97) | low | M/- | yes | IMPLEMENT | Reopen resets the active commit maximum rather than restoring the documented cross-segment watermark |
| [L3-RV-WAL-C55C2D47-FACET](P3.md#l3-rv-wal-c55c2d47-facet) | low | M/code | yes | IMPLEMENT | Group-commit callers lose the underlying sink cause |
| [L3-RV-WAL-79167DD7](P3.md#l3-rv-wal-79167dd7) | nit | M/doc | no | - | Retired F5c KV-marker wire protocol still exported as public API, with docs describing it as live |
| [L4-RV-WAL-0843B96B](P4.md#l4-rv-wal-0843b96b) | nit | M/code | yes | IMPLEMENT | pub mod segment_meta exports nothing public |
| [L4-RV-WAL-3F10E8A1](P4.md#l4-rv-wal-3f10e8a1) | nit | M/decision | yes | SKIP | Error strings embed absolute filesystem paths |
| [L4-RV-WAL-5DFE355F](P4.md#l4-rv-wal-5dfe355f) | nit | S/code | yes | IMPLEMENT | WalSegment::mark_poisoned is an un-gated public test hook |
| [L4-RV-WAL-C66039FC](P4.md#l4-rv-wal-c66039fc) | nit | S/code | no | - | Mid-function use statements in tests (imports-at-top rule) |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p34-01-wal` / `worktrees/p34-01-wal` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |

### P34-02 - shamir-tx: GC starvation, publication floor, typed errors, history diagnostics, docs

- Session task: #1717. Brief: `docs/dev-artifacts/prompts/remediation/p34/02-tx.md`. Crates: `shamir-tx`.
- Wave 1; after: none. Cards 8 (low 6, nit 2), effort 20, owner-decision cards 3.

**Goal.** Make the legacy plain-store publisher monotonic, stop history_of from reporting a storage failure as an unknown age (GC keeps its conservative lookup), correct the VERSION_SEP codec-domain tests and prose, refresh stale scaffold documentation, and handle the A10 barrier versus GC starvation design-first.

**Acceptance.**

- publish_committed can no longer lower last_committed_version (regression named after 58E47F9E).
- history_of / history_of_many return the storage error instead of ts_millis = None; GC keeps versions of unknown age (regression named after 3FC2FA1D).
- The VERSION_SEP property generators admit separator bytes and the explanatory prose is true (1480F306, 1480F306-FACET).
- 7BB0D16C: either a written epoch-handoff design with safety and progress tests, or a DISPUTED / DEFERRED report. The barrier is not changed without a proof.

- Keep gap/barrier semantics of timestamp batching.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Decision | Title |
|---|---|---|---|---|---|
| [L3-RV-TX-4A1FDAAE](P3.md#l3-rv-tx-4a1fdaae) | low | L/code | yes | SKIP | project_event: per-record heap clone of the table-name String |
| [L3-RV-TX-7BB0D16C](P3.md#l3-rv-tx-7bb0d16c) | low | L/decision | no | - | A10 in-flight barrier can starve GC under sustained snapshot-open churn |
| [L3-RV-TX-FF6AB063](P3.md#l3-rv-tx-ff6ab063) | low | L/code | yes | SKIP | thiserror declared but never used — String errors including a public trait |
| [L3-RV-TX-1480F306](P3.md#l3-rv-tx-1480f306) | low | M/test | no | - | VERSION_SEP domain/coverage qualification; vacuum integrity remains separate |
| [L3-RV-TX-3FC2FA1D-SEMANTIC-FACET](P3.md#l3-rv-tx-3fc2fa1d-semantic-facet) | low | M/code | yes | IMPLEMENT | lookup_ts swallows history.get errors as unknown age with no log |
| [L3-RV-TX-58E47F9E](P3.md#l3-rv-tx-58e47f9e) | low | S/code | no | - | publish_committed plain store can regress last_committed_version; its safety contract is unsound |
| [L4-RV-TX-10BB4C96](P4.md#l4-rv-tx-10bb4c96) | nit | M/doc | no | - | TX editorial vocabulary, removed links and stale scaffold comments |
| [L4-RV-TX-1480F306-FACET](P4.md#l4-rv-tx-1480f306-facet) | nit | S/test | no | - | VERSION_SEP documentation and restricted-generator explanation |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p34-02-tx` / `worktrees/p34-02-tx` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |

### P34-03 - shamir-storage: rename copy recovery, decoder budgets, error sources, polish

- Session task: #1718. Brief: `docs/dev-artifacts/prompts/remediation/p34/03-storage.md`. Crates: `shamir-storage`.
- Wave 1; after: none. Cards 7 (low 3, nit 4), effort 21, owner-decision cards 5.

**Goal.** Pin the persisted buffer-config bytes with golden tests and state the schema-change policy, flatten the remaining test wrapper, add CI-measured allocation evidence before touching the batched-path clones, and, after the owner answer, make rename/create into a previously used table name safe against stale stores.

**Acceptance.**

- The v1 MetaEnvelope bytes of the buffer config are pinned by golden tests and the schema-change policy is written next to them (CCF2090E).
- 9478B37E: a red CI test shows whether rename-swap, drop-then-create and retry after an injected copy failure merge stale rows. The fix is implemented only when the Decisions section contains the owner answer.
- Flattening the audit_2_3 wrapper (3D6F974D) keeps leaf test names or the report states the discovery-name change.
- B6B3CBB6 changes code only where a CI allocation measurement shows a benefit.

- CCF2090E edits crates/shamir-engine/src/table/tests/table_core/config/buffer_config_tests.rs and reads crates/shamir-index/src/model/meta/meta_envelope.rs; touch only those files outside shamir-storage.
- 9478B37E touches shamir-engine repo/instance/repo_instance/catalog.rs only after the owner answer.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Decision | Title |
|---|---|---|---|---|---|
| [L3-RV-STORAGE-9478B37E](P3.md#l3-rv-storage-9478b37e) | low | L/code | yes | ESCALATE | Repo::copy_store default impl leaves a partially-populated destination on failure |
| [L3-RV-STORAGE-FB7C0794-SEMANTIC-FACET](P3.md#l3-rv-storage-fb7c0794-semantic-facet) | low | L/decision | yes | SKIP | Nit: KeyBytes::Deserialize allocates an unbounded blob before any size check |
| [L3-RV-STORAGE-CCF2090E](P3.md#l3-rv-storage-ccf2090e) | low | M/test | yes | IMPLEMENT | Persisted MemBufferConfig wire format has no versioning guardrails despite a stable wire-format claim |
| [L3-RV-STORAGE-0A9E3A41-SEMANTIC-FACET](P3.md#l3-rv-storage-0a9e3a41-semantic-facet) | nit | L/decision | yes | SKIP | Error-source chains flattened; thread-spawn failure panics instead of DbResult |
| [L4-RV-STORAGE-88C4F2CC](P4.md#l4-rv-storage-88c4f2cc) | nit | L/decision | yes | SKIP | Storage alias/bounds/copy/error-wording polish; external implementations already work |
| [L4-RV-STORAGE-B6B3CBB6](P4.md#l4-rv-storage-b6b3cbb6) | nit | M/code | no | - | Minor allocations/clones on batched paths |
| [L4-RV-STORAGE-3D6F974D](P4.md#l4-rv-storage-3d6f974d) | nit | S/test | no | - | storage_membuffer_tests.rs packs three unrelated fixture topics into nested inline mods instead of topic files |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p34-03-storage` / `worktrees/p34-03-storage` |
| Owner answers recorded | see Owner questions |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |

### P34-04 - shamir-connect + shamir-server: wire-visible errors, fallible entropy/clock, KDF error, replication pins, bootstrap password

- Session task: #1719. Brief: `docs/dev-artifacts/prompts/remediation/p34/04-connect-server.md`. Crates: `shamir-connect`, `shamir-server`.
- Wave 1; after: none. Cards 10 (low 5, nit 5), effort 24, owner-decision cards 7.

**Goal.** Finish the fallible entropy/clock work (Ed25519 generation, Argon2 admission, session touch, identity-rotation update, remaining UnixNanos::now callers), keep ticket-version acceptance coherent with the authoritative rotation snapshot, close the remaining error-path test gaps, give the plain-profile loopback predicate a single owner, warn on the argv bootstrap password, document the replication-upstream pinning limit, and stop the long-poll replication source from serialising pull targets.

**Acceptance.**

- Entropy and clock failure injection proves typed failure, cleanup and no partial publication on every migrated path (387EAA05).
- ServerIdentityState rotation publishes ticket-acceptance versions from the authoritative snapshot only (F8A5E821).
- Shared handshake byte fixtures exercise the real production DTOs of server and client (8999F6B7); the abrupt-termination durable-counter replay oracle and the old-derivation buffer-wipe case exist (F43EB41E).
- Boot warns once, without the value, when --bootstrap-password is used (97C240D7); the plain-profile loopback predicate has one owner and the accepted addresses are unchanged (A65118CC); the upstream-pinning limit is documented (E8E1D11B).
- Two pull targets of one WireReplSource: the second is not delayed by the first long poll (regression for the P2-U01 finding).

- 387EAA05 is the second tranche of the P2-10 decision; keep its rules (never substitute zero entropy or fabricated time, fail closed, no partial publication).
- Downstream shamir-server/shamir-client migrations for any API that becomes fallible are part of this stage; run `cargo check` of dependents.
- A65118CC reads shamir-transport-tcp listener.rs (ListenerProfile) and does not change its behaviour.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Decision | Title |
|---|---|---|---|---|---|
| [L3-RV-CONNECT-387EAA05](P3.md#l3-rv-connect-387eaa05) | low | L/code | yes | IMPLEMENT | Library expect panics for OS RNG failure and pre-epoch clock |
| [L3-RV-SERVER-E8E1D11B](P3.md#l3-rv-server-e8e1d11b) | low | L/decision | yes | IMPLEMENT | Replication client uses trust-on-first-use with no leader-key pinning |
| [L3-RV-CONNECT-2A423D73](P3.md#l3-rv-connect-2a423d73) | low | M/- | yes | SKIP | RequestHandler::handle Err(String) flows verbatim onto the wire |
| [L3-RV-CONNECT-F43EB41E](P3.md#l3-rv-connect-f43eb41e) | low | M/test | no | - | Consolidated error-path test gaps |
| [L3-RV-CONNECT-F8A5E821](P3.md#l3-rv-connect-f8a5e821) | low | M/code | no | - | ServerIdentityState::rotate is a non-atomic check-then-store on ArcSwap |
| [L4-RV-CONNECT-7D41FB60](P4.md#l4-rv-connect-7d41fb60) | nit | M/code | yes | SKIP | Auth maintenance bundle: stale references and inert suppressors |
| [L4-RV-CONNECT-E4631CD9](P4.md#l4-rv-connect-e4631cd9) | nit | M/code | yes | SKIP | validate_client_kdf_safe returns String; its diagnostic is discarded |
| [L4-RV-SERVER-8999F6B7](P4.md#l4-rv-server-8999f6b7) | nit | M/test | no | - | Positional handshake DTOs independently mirrored with shamir-client |
| [L4-RV-SERVER-97C240D7](P4.md#l4-rv-server-97c240d7) | nit | M/decision | yes | IMPLEMENT | `bootstrap_password` accepted as a CLI argument |
| [L4-RV-SERVER-A65118CC](P4.md#l4-rv-server-a65118cc) | nit | M/code | yes | IMPLEMENT | Duplicated normative loopback predicate with shamir-transport-tcp |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p34-04-connect-server` / `worktrees/p34-04-connect-server` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |

### P34-05 - shamir-client (+node binding) + transports: typed sources, resume ticket lifetime, framing reuse, tls tests, docs

- Session task: #1720. Brief: `docs/dev-artifacts/prompts/remediation/p34/05-client-net.md`. Crates: `shamir-client`, `shamir-client-node`, `shamir-transport-tcp`, `shamir-transport-ipc`.
- Wave 2; after: P34-04. Cards 13 (low 9, nit 4), effort 23, owner-decision cards 5.

**Goal.** Preserve typed sources for envelope encoding failures, drop the redundant interner sync where it is provably unnecessary, reuse request framing storage on the client hot path, add the missing TLS exporter and write_frame_prereserved contract tests, qualify the exporter RFC wording, and replace guessed performance claims with CI measurements.

**Acceptance.**

- Envelope encoding errors keep their source chain (35A189A9).
- write_frame_prereserved reports a mismatched prefix as a malformed-buffer error before the size cap (884DDBF5); a real uncompleted rustls connection exporter case asserts its specific cause (A38C5274).
- Framing reuse allocates no second frame buffer on the roundtrip path and keeps write serialisation, terminal cancellation and zeroization of secret scratch (79910BA5), with an allocation oracle that fails on the old code.
- 66E9A5FC and 31C5E8A6: the measurement harness or oracle exists and its CI result is recorded; numbers are not guessed.
- B80C0D09 is skipped until the owner answers.

**Coordination.** P34-04 changes shamir-connect APIs and handshake fixtures that the client consumes (35A189A9 edits connect/common/messages/envelope.rs; 8999F6B7 edits client wire tests).

- 35A189A9 edits shamir-connect/src/common/messages/envelope.rs (a P34-04 crate): keep that edit minimal and run `cargo check -p shamir-connect`.
- 79910BA5 changes the client hot path and shamir-transport-tcp framing together; keep zeroization and terminal cancellation.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Decision | Title |
|---|---|---|---|---|---|
| [L3-RV-CLIENT-35A189A9](P3.md#l3-rv-client-35a189a9) | low | M/code | yes | IMPLEMENT | Stringly-typed `Handshake`/`Tls`/`Transport`/`Protocol` variants erase typed sources |
| [L3-RV-CLIENT-8997B366](P3.md#l3-rv-client-8997b366) | low | M/code | yes | SKIP | `ResumeOptions::ticket` and its wire copy are not `Zeroizing` |
| [L3-RV-CLIENT-B80C0D09](P3.md#l3-rv-client-b80c0d09) | low | M/decision | yes | ESCALATE | `std::sync::Mutex` on the demux hot paths (`PendingMap`, `SubscriptionMap`, `EarlyBuffer`) without a sancti... |
| [L3-RV-TRANSPORT-IPC-FA481516](P3.md#l3-rv-transport-ipc-fa481516) | low | M/decision | yes | SKIP | chmod-after-bind race window (documented, spec-prescribed — report as accepted risk with a structural option) |
| [L3-RV-TRANSPORT-TCP-31C5E8A6](P3.md#l3-rv-transport-tcp-31c5e8a6) | low | M/test | no | - | Two `read_exact` calls per frame: extra read round-trip on unbuffered plain-TCP streams |
| [L3-RV-TRANSPORT-TCP-79910BA5](P3.md#l3-rv-transport-tcp-79910ba5) | low | M/code | no | - | Allocating `read_frame`/`write_frame` remain the ergonomic defaults and are still on a production per-reque... |
| [L3-RV-TRANSPORT-TCP-884DDBF5](P3.md#l3-rv-transport-tcp-884ddbf5) | low | M/- | no | - | `write_frame_prereserved` reports every contract violation as `TooLarge` |
| [L3-RV-TRANSPORT-TCP-A38C5274](P3.md#l3-rv-transport-tcp-a38c5274) | low | M/test | no | - | `tls.rs` has zero unit tests; all error branches unexercised (happy-path-only e2e coverage) |
| [L3-RV-TRANSPORT-TCP-17E11DF1](P3.md#l3-rv-transport-tcp-17e11df1) | low | S/doc | no | - | Project-specific exporter is incorrectly described as strict RFC 9266 compliance |
| [L4-RV-CLIENT-1C219006](P4.md#l4-rv-client-1c219006) | nit | M/- | no | - | Ambient interner sync runs on every `execute` regardless of server version or cache state |
| [L4-RV-CLIENT-66E9A5FC](P4.md#l4-rv-client-66e9a5fc) | nit | M/test | no | - | Perf claims are unmeasured: no benches, no allocation-behaviour tests |
| [L3-RV-TRANSPORT-TCP-93246CAF](P3.md#l3-rv-transport-tcp-93246caf) | nit | S/test | no | - | Test name promises TLS-bind-on-unspecified coverage the body deliberately doesn't deliver |
| [L4-RV-CLIENT-NODE-31D3BBE5](P4.md#l4-rv-client-node-31d3bbe5) | nit | S/decision | yes | SKIP | Version drift: Cargo `0.1.0-alpha.1` vs npm/generated-loader `0.1.0` |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p34-05-client-net` / `worktrees/p34-05-client-net` |
| Owner answers recorded | see Owner questions |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |

### P34-06 - shamir-db + shamir-engine(+query, validator): catalogue lookups, boot diagnostics, builder migration, pattern contract, FK membership, access-gate allocations

- Session task: #1721. Brief: `docs/dev-artifacts/prompts/remediation/p34/06-db-engine.md`. Crates: `shamir-db`, `shamir-engine`, `shamir-engine-query`, `shamir-engine-validator`.
- Wave 2; after: P34-01, P34-03, P34-08. Cards 11 (low 8, nit 3) + 2 P2 carry-overs, effort 30, owner-decision cards 5.

**Goal.** Replace full-catalogue scans by keyed lookups where the corruption semantics allow, make boot reconstruction failures visible, move the facade onto the query builder, close the validator gap behind the infallible pattern adapters, share FK parent membership per batch, drop per-evaluation formatting from the condition cache, and remove the remaining allocations on the access-gate path.

**Acceptance.**

- Every facade request built by the builder equals the old hand-written request field for field (one equality test per call site); then shamir-query-builder is a production dependency of shamir-db (C2DCAC96).
- Boot logs one warning per skipped repository with identity, backend and reason (A7A29B02-FACET); a missing group name does not scan the catalogue (5977A782); keyed repository lookup keeps the corruption errors (23A03103).
- Invalid regex/like patterns are rejected at validation time (2A18B647); FK parent membership is reused per batch and still sees transaction-staged parents (3C71057D); condition-cache hits no longer format the AST (62C6F2B1).
- Cost-discriminating regressions (counter or seam) for 39178D2D and 3EB43AC7 and a deep-chain allocation oracle for A7B1FCEC.

**Coordination.** The hub crates go first: their cards cite engine and db files (wal A25EDE9D drain step, storage 9478B37E catalog.rs, types A7B1FCEC access gate), so db/engine edits land on top of them.

- A7B1FCEC (a shamir-types card) is implemented here because its edits are in shamir-db access/authorize.rs and access/meta.rs; the borrowed accessor it needs in shamir-types is a small additive edit.
- RV-DB-C2DCAC96: set every request field explicitly; the builder defaults (return_all, id) differ from the hand-written requests. SystemStore/replication construction sites are out of scope and only get a one-line exception comment.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Decision | Title |
|---|---|---|---|---|---|
| [L3-RV-ENGINE-2A18B647](P3.md#l3-rv-engine-2a18b647) | low | L/decision | yes | IMPLEMENT | Regex/Like patterns have no size cap and invalid patterns silently compile to False |
| [L4-RV-ENGINE-3C71057D](P4.md#l4-rv-engine-3c71057d) | low | L/code | no | - | FK exists_in_table fallback: full parent-table scan per child record |
| [L3-RV-DB-23A03103](P3.md#l3-rv-db-23a03103) | low | M/code | no | - | `SystemStore::load_repository_record` full-catalogue scan + linear filter for a keyed lookup |
| [L3-RV-DB-C2DCAC96](P3.md#l3-rv-db-c2dcac96) | low | M/code | yes | IMPLEMENT | Builder-only query-construction rule bypassed across the facade (30+ hand-assembled wire ops, no exception ... |
| [L3-RV-ENGINE-62C6F2B1](P3.md#l3-rv-engine-62c6f2b1) | low | M/code | no | - | Content-key cache remediation introduces per-evaluation formatting allocation and AST traversal |
| [L3-RV-TYPES-A7B1FCEC](P3.md#l3-rv-types-a7b1fcec) | low | M/code | no | - | Authorization helpers allocate fresh Strings + Vecs per check on the access-gate path |
| [L4-RV-DB-5977A782](P4.md#l4-rv-db-5977a782) | low | M/code | no | - | `resolve_group_id(GroupRef::Name)` is a full `load_groups()` scan per call |
| [L3-RV-DB-A7A29B02-FACET](P3.md#l3-rv-db-a7a29b02-facet) | low | S/code | no | - | Boot catalogue skip diagnostics, not a claim that recovery always succeeds |
| [L4-RV-DB-0065D2A2](P4.md#l4-rv-db-0065d2a2) | nit | M/decision | yes | SKIP | Catalogue `wasm_hash` and `version` fields are dead, and the hash is not integrity-grade |
| [L4-RV-DB-7C59E01B](P4.md#l4-rv-db-7c59e01b) | nit | M/decision | yes | IMPLEMENT | Nit: intentionally-leaked per-key lock maps are the only unbounded-growth sites — documented, but key count... |
| [L4-ENGINE-STYLE-DELIVERY-PROVENANCE](P4.md#l4-engine-style-delivery-provenance) | nit | S/decision | yes | SKIP | Style-delivery provenance remains partial after the structural code edits |
| [RV-DB-39178D2D](P2.md#rv-db-39178d2d) | medium | L/code | no | P2 carry-over | `InternerTouch` computes the epoch via a full interner traversal per touch |
| [RV-DB-3EB43AC7](P2.md#rv-db-3eb43ac7) | low | M/code | no | P2 carry-over | Boot path pairs repos with their tables via an O(repos × tables) nested scan |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p34-06-db-engine` / `worktrees/p34-06-db-engine` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |

### P34-07 - shamir-index: n-gram budget, vector snapshot, backend secrets, doc/import cleanup

- Session task: #1722. Brief: `docs/dev-artifacts/prompts/remediation/p34/07-index.md`. Crates: `shamir-index`.
- Wave 2; after: P34-03. Cards 4 (low 2, nit 2) + 3 P2 carry-overs, effort 19, owner-decision cards 2.

**Goal.** Bound n-gram tokenization with a streaming visitor and complete legacy cleanup, handle legacy backend credentials per the owner answer, fix the stale lifecycle documentation and local imports, and (bench-gated) stream the vector snapshot while keeping the repaired generation/prune ordering.

**Acceptance.**

- The n-gram visitor never builds a per-value Vec of all grams and old-value cleanup is complete without truncation (0695AADA).
- CF86432E is skipped until the owner answers.
- 4DC29EFB: before/after memory numbers from CI; the repaired generation/prune ordering is preserved.
- 40BCFF22 and F39434C2 follow their P2 recipes; the lifecycle documentation matches the shipped IndexState enum (20415F00); remaining local imports are hoisted (32C98E9F).

**Coordination.** CCF2090E (P34-03) reads index meta_envelope.rs and pins its bytes; index edits come after the pin exists.

- CCF2090E in P34-03 pins the buffer-config bytes through meta_envelope.rs; do not change that envelope here.
- tokenizer.rs and persistence.rs are shared with P34-12, which starts after this stage.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Decision | Title |
|---|---|---|---|---|---|
| [L3-RV-INDEX-0695AADA](P3.md#l3-rv-index-0695aada) | low | L/code | yes | IMPLEMENT | `NgramTokenizer` output is unbounded — indexing-time memory/write amplification from one long token |
| [L3-RV-INDEX-CF86432E](P3.md#l3-rv-index-cf86432e) | low | L/decision | yes | ESCALATE | External vector-backend API key persisted in cleartext inside the index-metadata blob |
| [L4-RV-INDEX-32C98E9F](P4.md#l4-rv-index-32c98e9f) | nit | M/code | no | - | `use` statements inside function/block bodies across six production files |
| [L4-RV-INDEX-20415F00](P4.md#l4-rv-index-20415f00) | nit | S/- | no | - | Stale lifecycle doc contradicts the shipped `IndexState` wire enum; minor `IndexRecordKey` API warts |
| [RV-INDEX-4DC29EFB](P2.md#rv-index-4dc29efb) | medium | L/code | yes | P2 carry-over | Vector snapshot dump/load fully materializes the graph and sidecar in RAM |
| [RV-INDEX-40BCFF22](P2.md#rv-index-40bcff22) | medium | M/code | no | P2 carry-over | Vector snapshot v1 back-compat is claimed but has no working decode path, and the only test is vacuous |
| [RV-INDEX-F39434C2](P2.md#rv-index-f39434c2) | low | M/code | no | P2 carry-over | Silent degradation fallbacks with no logging |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p34-07-index` / `worktrees/p34-07-index` |
| Owner answers recorded | see Owner questions |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |

### P34-08 - shamir-types + shamir-collections: secret equality contract, decode caps, allocation, doc and import cleanup

- Session task: #1723. Brief: `docs/dev-artifacts/prompts/remediation/p34/08-types.md`. Crates: `shamir-types`, `shamir-collections`.
- Wave 1; after: none. Cards 6 (low 3, nit 3) + 4 P2 carry-overs, effort 16, owner-decision cards 2.

**Goal.** Add the Drop-dispatch regression for SecretString, keep the ratified trusted-root constraint of the generic bincode helper as a documented limitation, close the allocation and 32-bit gaps carried over from P2 with real oracles, and clean up documentation and imports.

**Acceptance.**

- The SecretString regression exercises Drop dispatch, not only the wipe helper; its feature-gated test is compiled with its feature (FE40CB54).
- Direct username-bridge determinism and i64-range assertions exist (65DDE5B8).
- Allocation-count oracles that fail on the old code exist for 71AFA12F, E82C6BE1 and 313627EB; the 32-bit-only oracle of F070E432 is documented as such.
- Remaining test-body imports are hoisted (178ED8CB); documentation cards change comments only.

- RV-TYPES-A7B1FCEC is implemented in P34-06 (its edits are in shamir-db).

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Decision | Title |
|---|---|---|---|---|---|
| [L3-RV-TYPES-5E4BFC76](P3.md#l3-rv-types-5e4bfc76) | low | M/decision | yes | SKIP | Generic `bincode::from_bytes` helper lacks the depth caps its sibling decoders have |
| [L3-RV-TYPES-FE40CB54](P3.md#l3-rv-types-fe40cb54) | low | M/code | yes | IMPLEMENT | `SecretString`: constant-time-comparison footgun plus avoidable `unsafe` |
| [L3-RV-TYPES-65DDE5B8](P3.md#l3-rv-types-65dde5b8) | low | S/test | no | - | Untested public surface with documented contracts: `Interner::generation`, `ResourceMeta::owner_field`, `pr... |
| [L4-RV-TYPES-178ED8CB](P4.md#l4-rv-types-178ed8cb) | nit | M/code | no | - | Mid-function imports scattered through test files |
| [L4-RV-COLLECTIONS-201D84D3-FACET](P4.md#l4-rv-collections-201d84d3-facet) | nit | S/doc | no | - | Public rustdoc discoverability and local lint-comment presentation |
| [L4-RV-TYPES-F8CF2706](P4.md#l4-rv-types-f8cf2706) | nit | S/doc | no | - | API-polish bundle (five items; two folded into other findings) |
| [RV-TYPES-71AFA12F](P2.md#rv-types-71afa12f) | low | M/code | no | P2 carry-over | Log-forging surface: raw resource names rendered into trace/denial lines |
| [RV-TYPES-E82C6BE1](P2.md#rv-types-e82c6be1) | low | M/code | no | P2 carry-over | `merge_storage_bytes` allocates intermediate Vecs per NEW set_map entry before copying into the output buffer |
| [RV-TYPES-F070E432](P2.md#rv-types-f070e432) | low | M/code | no | P2 carry-over | `pos + len` unchecked additions in the tree decoder's `read_str`/`read_bin` |
| [RV-TYPES-313627EB](P2.md#rv-types-313627eb) | nit | S/code | no | P2 carry-over | `QueryValue::set_path` builds the error-message path prefix during successful traversals too |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p34-08-types` / `worktrees/p34-08-types` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |

### P34-09 - shamir-funclib + shamir-numa + shamir-bench-utils: error taxonomy, doc claims, test coverage

- Session task: #1724. Brief: `docs/dev-artifacts/prompts/remediation/p34/09-funclib-infra.md`. Crates: `shamir-funclib`, `shamir-numa`, `shamir-bench-utils`.
- Wave 1; after: none. Cards 8 (low 4, nit 4), effort 9, owner-decision cards 0.

**Goal.** Catalogue the resolver-generated scalar error codes, correct the remaining lock-free and canonical-exclusion documentation claims, repair the NUMA coverage claims and mirror-window prose, and fix the bench-utils documentation and the statistical-versus-exact test assertion.

**Acceptance.**

- Catalogue assertions cover the resolver-generated scalar_task_failed and not_indexable codes with their spellings preserved (24C30E50).
- Documentation states the actual shared bucket read-lock contract (C8BC39BC) and the string-key-only exclusion (775ADABC).
- NUMA replica load_node/store_node out-of-range coverage uses a real seam instead of mock clamping (553CB81D).
- bench-utils: zero-noise per-row construction-identity coverage without weakening tolerances (F53F775F); GENERATOR_REVISION is bumped only if generator output changes.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Decision | Title |
|---|---|---|---|---|---|
| [L3-RV-FUNCLIB-24C30E50](P3.md#l3-rv-funclib-24c30e50) | low | M/code | no | - | `ScalarError` string-code taxonomy is unpoliced: no code catalog, drifting synonyms, zero context |
| [L3-RV-FUNCLIB-775ADABC](P3.md#l3-rv-funclib-775adabc) | low | S/doc | no | - | Generic canonical APIs do not exclude an interned _prev_hash field despite their unconditional reserved-fie... |
| [L3-RV-FUNCLIB-C8BC39BC](P3.md#l3-rv-funclib-c8bc39bc) | low | S/doc | no | - | `UserScalarLayer::get` uses `get_sync` (exclusive bucket lock) while its docs claim "read-only hash probes ... |
| [L3-RV-NUMA-553CB81D](P3.md#l3-rv-numa-553cb81d) | low | S/- | no | - | Test-coverage claims vs reality: `detect()` untested off-Linux, `LinuxTopology` error paths untested, READM... |
| [L4-RV-BENCH-UTILS-4E7A9919](P4.md#l4-rv-bench-utils-4e7a9919) | nit | S/- | no | - | Reproducibility key described inconsistently: "(k, σ, seed) triple" vs. five values |
| [L4-RV-BENCH-UTILS-67097ED5-FACET](P4.md#l4-rv-bench-utils-67097ed5-facet) | nit | S/doc | no | - | Stale Criterion wording alongside the real Harness consumers |
| [L4-RV-BENCH-UTILS-F53F775F](P4.md#l4-rv-bench-utils-f53f775f) | nit | S/test | no | - | `round_robin_balances_clusters` asserts a statistical property as an exact equality |
| [L4-RV-NUMA-6E1761A2-FACET](P4.md#l4-rv-numa-6e1761a2-facet) | nit | S/doc | no | - | Unsupported numerical bound in mirror-window documentation |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p34-09-funclib-infra` / `worktrees/p34-09-funclib-infra` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |

### P34-10 - shamir-query-types + shamir-query-builder (+macros): conversion contract, receipt metadata, switch lowering, macro grammar, structure

- Session task: #1725. Brief: `docs/dev-artifacts/prompts/remediation/p34/10-query.md`. Crates: `shamir-query-types`, `shamir-query-builder`, `shamir-query-builder-macros`.
- Wave 3; after: P34-06. Cards 13 (low 9, nit 4) + 3 P2 carry-overs, effort 39, owner-decision cards 7.

**Goal.** Reuse nested receipt metadata, add the CallOp constructor so q!(call ...) goes through the builder layer, normalise raw identifiers in select-function classification, take borrowed Batch encoding off deep clones, finish the DDL owner-file separation, fix the remaining documentation, and close the P2 carry-overs (pagination equality, allocation-free Big comparison, diagnostic fixtures).

**Acceptance.**

- Nested receipt-policy preflight no longer recollects descendant repositories on admin-write paths (0A0D9092).
- q!(call ...) builds its operation through CallOp::new / CallOp::with_repo; the expansion is unchanged for callers (2C459E8E).
- A raw `r#count` select function classifies like `count` (002C0148); borrowed Batch encoding does not deep-clone the request (9E172C69).
- The DDL list builders have their own owner files and the exports are preserved (7A0F3500); pagination equality and Big comparison carry-overs have tests that fail on the old code (3D922433, D687DBA3).

**Coordination.** 0A0D9092 edits engine receipt policy and batch_execute, which P34-06 also edits.

- 2C459E8E adds CallOp::new / with_repo in shamir-query-types (additive, source compatible).
- 0A0D9092 edits shamir-engine receipt policy and batch_execute: touch only those files in shamir-engine.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Decision | Title |
|---|---|---|---|---|---|
| [L3-RV-QUERY-BUILDER-2A648617](P3.md#l3-rv-query-builder-2a648617) | low | L/decision | yes | SKIP | `Batch::switch` guard construction is O(K^2) deep clones and O(K^2) emitted filter nodes |
| [L3-RV-QUERY-TYPES-0A0D9092](P3.md#l3-rv-query-types-0a0d9092) | low | L/code | no | - | Three separate full-tree recursive walks per request: `is_write`, `distinct_repos`, `collect_required_access` |
| [L3-RV-QUERY-TYPES-182C703E](P3.md#l3-rv-query-types-182c703e) | low | L/code | yes | SKIP | thiserror convention deviation: hand-rolled Display/Error impls and `Result<(), String>` public APIs |
| [L3-RV-QUERY-TYPES-4F767AA5](P3.md#l3-rv-query-types-4f767aa5) | low | L/code | yes | SKIP | `From<QueryValue> for FilterValue` silently substitutes `Null` in release builds |
| [L3-RV-QUERY-BUILDER-9E172C69](P3.md#l3-rv-query-builder-9e172c69) | low | M/code | no | - | `Batch::build()` deep-clones the entire request; this sits on the SDK/client send path |
| [L3-RV-QUERY-BUILDER-MACROS-002C0148](P3.md#l3-rv-query-builder-macros-002c0148) | low | M/code | no | - | Raw identifiers are silently lowered to database names containing the Rust r# escape prefix |
| [L3-RV-QUERY-BUILDER-MACROS-2C459E8E](P3.md#l3-rv-query-builder-macros-2c459e8e) | low | M/code | yes | IMPLEMENT | `q!(call ...)` bypasses the builder layer entirely and pins `repo: "main"` with no repo-qualified syntax |
| [L3-RV-QUERY-BUILDER-MACROS-9BCA14B1](P3.md#l3-rv-query-builder-macros-9bca14b1) | low | M/test | yes | SKIP | Unbounded recursion in expression lowering; pathological nesting aborts rustc instead of erroring |
| [L3-RV-QUERY-BUILDER-MACROS-30DA1AC8](P3.md#l3-rv-query-builder-macros-30da1ac8) | low | S/doc | no | - | Clause keywords are silently reserved inside `q!` where/having, contradicting the documented "full `filter!... |
| [L4-RV-QUERY-TYPES-CC015A3C](P4.md#l4-rv-query-types-cc015a3c) | nit | L/decision | yes | SKIP | Per-construction `"main"` String allocations |
| [L3-RV-QUERY-BUILDER-7A0F3500](P3.md#l3-rv-query-builder-7a0f3500) | nit | M/code | no | - | `ddl/` applies one-file-one-export inconsistently — family files bundle many unrelated public builders |
| [L4-RV-QUERY-BUILDER-0E30EB25](P4.md#l4-rv-query-builder-0e30eb25) | nit | S/doc | no | - | Stale doc references |
| [L4-RV-QUERY-BUILDER-21560B4A](P4.md#l4-rv-query-builder-21560b4a) | nit | S/decision | yes | SKIP | `val::func`/`val::expr` collide by name with `select::func`/`select::expr` for glob-import users |
| [RV-QUERY-BUILDER-MACROS-13A0D25A](P2.md#rv-query-builder-macros-13a0d25a) | low | M/code | no | P2 carry-over | Field-path and alias spans are discarded; downstream errors point at the macro call site |
| [RV-QUERY-TYPES-3D922433](P2.md#rv-query-types-3d922433) | low | M/code | no | P2 carry-over | `Pagination::eq` (`After`) — two msgpack encodes per equality comparison |
| [RV-QUERY-TYPES-D687DBA3](P2.md#rv-query-types-d687dba3) | nit | M/code | no | P2 carry-over | `FieldPath` type alias defined in `filter/mod.rs` |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p34-10-query` / `worktrees/p34-10-query` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |

### P34-11 - shamir-sdk (+macros) + shamir-wasm-host: ABI limit, error categories, oracles, docs

- Session task: #1726. Brief: `docs/dev-artifacts/prompts/remediation/p34/11-sdk-wasm.md`. Crates: `shamir-sdk`, `shamir-sdk-macros`, `shamir-wasm-host`.
- Wave 3; after: P34-06. Cards 9 (low 6, nit 3), effort 18, owner-decision cards 3.

**Goal.** Classify unexpected DB response shapes and codec causes consistently in the SDK, make HTTP response decoding fail closed on duplicate fields, deprecate the lossy catalogue decoder in favour of the checked one, cover host-import trap paths, and fix the remaining documentation. The signed-i32 ABI limit stays an accepted, documented limitation.

**Acceptance.**

- Unexpected DB response shapes classify as Codec and codec sources are preserved (6D2497CA-FACET).
- A duplicate top-level status/headers/body field in an HTTP response map is a codec error, not first-match (B5ACD67D-FACET).
- FunctionMeta::from_record is #[deprecated] with a doc that states the lossy fallbacks; consumers use try_from_record (4669FDA6).
- Host-import trap/error paths, including malformed or non-map nested-call Params, have in-crate tests (91385BE0); remaining cards change comments only.

**Coordination.** 4669FDA6 migrates its consumer in shamir-db catalog/artifact_kind.rs and B5ACD67D edits shamir-db function tests; both are P34-06 territory.

- 4669FDA6 also migrates the consumer in shamir-db catalog/artifact_kind.rs; B5ACD67D edits shamir-db/tests/functions/lifecycle/http_egress.rs.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Decision | Title |
|---|---|---|---|---|---|
| [L3-RV-SDK-723F5FD2-SEMANTIC-FACET](P3.md#l3-rv-sdk-723f5fd2-semantic-facet) | low | L/decision | yes | SKIP | Guest ABI passes pointers as signed `i32`; host rejects addresses ≥ 2 GiB |
| [L3-RV-SDK-6D2497CA-FACET](P3.md#l3-rv-sdk-6d2497ca-facet) | low | M/code | no | - | SDK protocol-error diagnostic category is flattened |
| [L3-RV-SDK-B5ACD67D-FACET](P3.md#l3-rv-sdk-b5acd67d-facet) | low | M/test | yes | IMPLEMENT | Malformed HTTP-envelope and success-decoder oracle gaps |
| [L3-RV-SDK-MACROS-853FC350-FACET](P3.md#l3-rv-sdk-macros-853fc350-facet) | low | M/code | no | - | Lexical return-name diagnostics versus concrete SDK typing |
| [L3-RV-WASM-HOST-4669FDA6](P3.md#l3-rv-wasm-host-4669fda6) | low | M/code | yes | IMPLEMENT | Catalogue record decoding has silent fallbacks and no format versioning |
| [L3-RV-WASM-HOST-91385BE0](P3.md#l3-rv-wasm-host-91385be0) | low | M/- | no | - | No in-crate test coverage for any host-import trap/error path (db/http/batch/global imports, depth limit, O... |
| [L4-RV-WASM-HOST-C43E9092](P4.md#l4-rv-wasm-host-c43e9092) | nit | M/doc | no | - | Internal audit-tracking references leaked into public API docs |
| [L4-RV-SDK-MACROS-FEC0E5E4-FACET](P4.md#l4-rv-sdk-macros-fec0e5e4-facet) | nit | S/doc | no | - | Guest driver comments retain obsolete pure-only/future-import wording |
| [L4-RV-WASM-HOST-FF7C2D6E](P4.md#l4-rv-wasm-host-ff7c2d6e) | nit | S/doc | no | - | WASM maintenance bundle: duplicate prose/glob code and fallback descriptions |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p34-11-sdk-wasm` / `worktrees/p34-11-sdk-wasm` |
| Owner answers recorded | - |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |

### P34-12 - shamir-index: persisted hash identity ("index identity v2"), design first

- Session task: #1727. Brief: `docs/dev-artifacts/prompts/remediation/p34/12-index-identity-v2.md`. Crates: `shamir-index`.
- Wave 3; after: P34-07. Cards 2 (low 2) + 2 P2 carry-overs, effort 16, owner-decision cards 2.

**Goal.** Design, then implement, a persisted hash identity ("index identity v2") that removes the insertion-order dependence of functional Map hashing, the false DuplicateKey risk of composite numeric unique indexes and the FxHasher-stability coupling, with an explicit migration or rebuild policy for existing persisted indexes. Design first: this stage needs a written design and an owner decision on any incompatible persisted-format change before code.

**Acceptance.**

- A written design (read-only reviewer) covering: the identity scheme, versioning of persisted hashes, the migration or rebuild path, the behaviour of old binaries, and the test plan, before any code.
- Owner decision recorded for any incompatible persisted-format change.
- After implementation: reversed-insertion Map lookups hit the same posting, composite numeric unique indexes raise no false DuplicateKey, golden vectors cover the new identity.

**Coordination.** tokenizer.rs and model/meta/persistence.rs are shared with P34-07 (0695AADA, CF86432E).

- 605EEC5D and FBB72544 were SKIPPED in P34 decisions as deferred to this stage; 53CEF405 and 9E472725 are P2 carry-overs.

**Cards** (order: risk, then size):

| Card | Risk | Size/kind | OD | Decision | Title |
|---|---|---|---|---|---|
| [L3-RV-INDEX-605EEC5D](P3.md#l3-rv-index-605eec5d) | low | L/decision | yes | SKIP | Persisted posting keys depend on FxHasher output stability, with no version coupling and a caret-pinned dep... |
| [L3-RV-INDEX-FBB72544](P3.md#l3-rv-index-fbb72544) | low | L/code | yes | SKIP | FunctionalBackend Map hashing is insertion-order dependent (byte-identity floor violation) |
| [RV-INDEX-53CEF405](P2.md#rv-index-53cef405) | medium | L/decision | yes | P2 carry-over | FTS posting keys hash untrusted token text with unkeyed `FxHasher` (`token_hash`) |
| [RV-INDEX-9E472725](P2.md#rv-index-9e472725) | medium | L/decision | yes | P2 carry-over | Regular + unique index keys use two correlated FxHasher streams as "collision resistance"; unique constrain... |

**Tracking** (fill in as the task moves):

| Item | Value |
|---|---|
| Status | [ ] todo |
| Branch / worktree | `fix/p34-12-index-identity-v2` / `worktrees/p34-12-index-identity-v2` |
| Owner answers recorded | see Owner questions |
| Executor session | - |
| Orchestrator diff read | - |
| Reviewer verdict | - |
| Fork PR / CI run | - |
| Integration batch | - |
