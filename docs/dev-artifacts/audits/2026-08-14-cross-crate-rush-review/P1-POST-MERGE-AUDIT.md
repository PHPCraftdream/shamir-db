# P1 post-merge audit report

## Scope and method

On 2026-10-29, 13 rush-reviewer sessions (A1–A13) statically reviewed the squash diffs and current source for the 55 merged PRs in the #311–#378 scope, against the stage cards and acceptance in [P1-FIX-PLAN.md](P1-FIX-PLAN.md) and [P1-FIX-TASKS.md](P1-FIX-TASKS.md). The review was read-only; no builds, tests or benchmarks were run. `OK` means static sign-off, `OK_WITH_NITS` means qualified sign-off, and `FAIL` means the reviewer withheld independent sign-off. Test sensitivity is inferred from code, not an executed mutation. Later fixes and the authoritative orchestrator triage below do not retroactively change reviewer verdicts.

## Summary

| Measure | PRs |
|---|---:|
| OK | 8 |
| OK_WITH_NITS | 35 |
| FAIL | 12 |
| Total | 55 |
| FAILs triaged as real | 11 |
| FAILs already closed by a subsequent scoped fix | 1 (H8) |

Real includes H5, delivered in stages (RU6 and G10 in #396, the `FilterValue` decoder and single-pass dispatch in #404, request-side unknown-key rejection partially in batch 2). The dispositions are the orchestrator's audit-time triage, not an independent verification of PR merge state or CI.

## Verdict table

| PR | Card | Verdict | Reason | Disposition |
|---|---|---|---|---|
| #311 | F4 | OK_WITH_NITS | Sealed replay fails loud; append uses the brief's documented-precondition alternative. | nits recorded |
| #312 | F7 | OK_WITH_NITS | MVCC durability and GC addressed; rebuild comment overstates cleanup. | nits recorded |
| #313 | G9 | OK_WITH_NITS | Decode caps and numeric projection addressed; guest stack safety unverified. | nits recorded |
| #314 | F1 | OK_WITH_NITS | Drain and transaction fencing addressed; dirty-entry cap oracle is limited. | nits recorded |
| #315 | G2 | OK_WITH_NITS | Audit chain corrections delivered; durable-writer lock still covers materialization. | nits recorded |
| #316 | F5 | OK | Canonical names, watermark and recovery coverage reviewed. | signed off |
| #317 | H4 | OK_WITH_NITS | Node buffer and close fixes; mutation oracle is probabilistic. | nits recorded |
| #318 | G6 | OK_WITH_NITS | TLS verification and framing fixes; proxy oracle and layout have limitations. | nits recorded |
| #319 | G1a | OK_WITH_NITS | Budget handoff and follower fixes; some tests exercise helpers only. | nits recorded |
| #320 | F11 | OK_WITH_NITS | Catalogue integrity and durability reviewed; local import remains. | nits recorded |
| #321 | F2 | FAIL | Fjall journal write errors remain lost. | Fixed: owner-approved lock-only bump and guard test, #404 (`a10cb72c`). |
| #322 | F9 | FAIL | Unfitted SQ8 opt-in is not persisted. | Fixed: `IndexDescriptor::options` carrier (@oh), #404 (`a10cb72c`). |
| #323 | H5 | FAIL | Unknown-field rejection and decode-work requirements incomplete; RU6 test gap. | Fixed in stages: RU6 #396; `FilterValue` decoder and single-pass dispatch #404 (`a10cb72c`); producer/DTO wire guard and request-side unknown-key rejection (partial) follow in batch 2. |
| #324 | F6 | FAIL | Several WAL I/O boundaries lose `ErrorKind`. | Fixed incl. sync-snapshot: #404 (`a10cb72c`). |
| #325 | G3 | OK_WITH_NITS | Session fixes reviewed; rotation and privacy oracles have gaps. | nits recorded |
| #326 | G4 | FAIL | Gateway IP range policy does not meet acceptance. | Fixed: three-class `classify_ip` policy (@oh), #404 (`a10cb72c`). |
| #328 | H1 | OK_WITH_NITS | Demux fixes reviewed; some oracles are structural or probabilistic. | nits recorded |
| #329 | F8 | OK_WITH_NITS | Bounded lookups and shared layout reviewed; handoff test is sequential. | nits recorded |
| #330 | H11 | FAIL | Argon2 offload incomplete across call sites. | Fixed under amended @oh acceptance (`block_in_place`, no nested pool task), #404 (`a10cb72c`). |
| #332 | F3 | OK_WITH_NITS | Cursor paging and replacement addressed; replacement stress oracle is nondeterministic. | nits recorded |
| #334 | G10 | FAIL | Root `ResultEncoding` re-export removed. | Fixed: #396 (`eda2115d`). |
| #335 | F12 | OK | DDL serialization and fail-closed FK guards reviewed. | signed off |
| #337 | G7 | OK_WITH_NITS | WebSocket bounds and heartbeat reviewed; scheduler and layout nits. | nits recorded |
| #338 | G5a | FAIL | Host-import trap matrix incomplete; setter tests probabilistic. | Fixed (tests only): #404 (`a10cb72c`). |
| #339 | H2 | OK_WITH_NITS | Empty squash; H2 implementation landed in #364. | nits recorded |
| #341 | H8 | FAIL | Its squash left builder-owned error and macro work incomplete. | Already closed by H8b (#356); no second fix. |
| #342 | G1b | OK_WITH_NITS | Shutdown tracking reviewed; integration oracle does not prove flush ordering. | nits recorded |
| #343 | G5b | OK | Async allocator and registry rename reviewed within approved scope. | signed off |
| #344 | H12 | OK_WITH_NITS | Canonical contracts addressed; hash-vector and error-type nits. | nits recorded |
| #345 | RG1 | FAIL | Atomic ordering gate misses associated-function calls. | Fix: #394 (merged). |
| #346 | H6 | OK_WITH_NITS | Versioned HMAC and gating reviewed; local imports remain. | nits recorded |
| #347 | G5d | FAIL | Startup cleanup can wait unboundedly and ignores kill errors. | Fixed: #404 (`a10cb72c`). |
| #348 | F14a | OK_WITH_NITS | Malformed FK map detection reviewed; on-update oracle and docs incomplete. | nits recorded |
| #349 | F14 | FAIL | Unknown fields fold before Regex/Like validation. | Fixed: #404 (`a10cb72c`). |
| #350 | F13 | OK_WITH_NITS | Error and authz work delivered; shim and some cleanup are partial. | nits recorded |
| #352 | G5c | OK | Guest global quota accounting reviewed within approved contract. | signed off |
| #353 | F15 | OK_WITH_NITS | NUMA bounds reviewed; host pinning test may skip. | nits recorded |
| #354 | I2 | OK | Fallible capacities and duration boundaries reviewed; RU8 predates this PR. | signed off |
| #355 | H7 | OK | Planner budget and pagination fixes reviewed. | signed off |
| #356 | H8b | OK_WITH_NITS | Completes H8; some tests and error implementation have nits. | nits recorded |
| #357 | F13b | OK_WITH_NITS | ACL memo and lookup improvements; repository lookup still scans. | nits recorded |
| #358 | I1 | OK_WITH_NITS | Measurement ownership and fixtures reviewed; unwind test and docs have gaps. | nits recorded |
| #359 | F10b | OK_WITH_NITS | Collector and borrowed planning reviewed; whole lookup is not O(k). | nits recorded |
| #360 | G8 | OK_WITH_NITS | IPC ownership reviewed; cross-principal denial not exercised. | nits recorded |
| #361 | H13 | OK_WITH_NITS | Interner and record-view fixes reviewed; deferred finding belongs to #370. | nits recorded |
| #362 | F14b | OK | Lazy staged probes reviewed, with explicit fallback. | signed off |
| #363 | H9 | OK_WITH_NITS | SDK validation reviewed; pagination deferred under RU10. | nits recorded |
| #364 | H3 | OK_WITH_NITS | Auth and scratch fixes reviewed; identity recovery completed by #375. | nits recorded |
| #366 | H14 | OK_WITH_NITS | Fail-closed metadata and codecs reviewed; documentation and test layout nits. | nits recorded |
| #367 | H10 | OK_WITH_NITS | Macro hardening reviewed; error transport completed by #378. | nits recorded |
| #368 | F14c | OK_WITH_NITS | Unwrap guard enforcement reviewed; fallback oracle and test names have gaps. | nits recorded |
| #370 | H13b | OK | Fail-closed conversion and interactive overlay resolution reviewed. | signed off |
| #371 | F14d | OK_WITH_NITS | Loom model and production-order pins reviewed; wrapper not directly tested. | nits recorded |
| #375 | #1530 S1–S6 / H3 | OK_WITH_NITS | Identity recovery follow-up reviewed; negative error assertions are broad. | nits recorded |
| #378 | #1531 / H9–H10 | OK_WITH_NITS | Guest error transport reviewed; compiled-guest e2e may skip. | nits recorded |

## FAIL arbitration

### F2 — #321

`RV-STORAGE-EC3A171B`: the reviewed shim documents the pinned Fjall journal-write error loss, and transaction execution delegates to batch commit (`crates/shamir-storage/src/stores/storage_fjall.rs:193`, `crates/shamir-storage/src/stores/storage_fjall.rs:209`). Triage: fixed, owner-approved lock-only fjall/lsm-tree 3.1.6 → 3.1.10 bump plus a Cargo.lock guard test, #404 (`a10cb72c`).

### F6 — #324

Append, fsync and replay stringify I/O errors, losing their kind (`crates/shamir-wal/src/segment/wal_segment.rs:338`, `crates/shamir-wal/src/segment/wal_segment.rs:398`, `crates/shamir-wal/src/segment/wal_segment.rs:659`). Triage: fixed, typed `io::ErrorKind` and the downcastable source are kept on the append, fsync, replay, repair, sidecar and sync-snapshot paths, #404 (`a10cb72c`). Retained-byte measurement and truncation unlink still stringify (outside the ticket cause path).

### F9 — #322

`RV-INDEX-E69B5ACD`: `VectorConfig.quantization` is skipped by serde, so an unfitted SQ8 opt-in is lost on descriptor persistence (`crates/shamir-index/src/model/kind.rs:173`). Triage: fixed, `IndexDescriptor::options` carries the SQ8 intent (@oh), #404 (`a10cb72c`).

### F14 — #349

The reviewed direct compiler folds unknown fields before constructing Regex/Like patterns (`crates/shamir-engine-query/src/query/filter/evaluation/planning/compile.rs:76`). The reviewer inferred that invalid patterns under NOT can therefore be accepted; batch preflight does not repair the direct contract. Triage: fixed, #404 (`a10cb72c`).

### G4 — #326

The gateway predicate adds exact unspecified IPv4 but did not establish the broader range policy (`crates/shamir-wasm-host/src/gateway/net_gateway.rs:494`). The inferred CGNAT example was not a demonstrated exploitation. Triage: fixed, @oh's three-class `classify_ip` policy with resolved and pinned exact hostnames, #404 (`a10cb72c`). Separate follow-up (pre-existing, not in scope): the curl gateway emits one `--resolve` entry per address, which curl may not accumulate.

### G5a — #338

`RV-WASM-HOST-91385BE0`: reviewed tests cover only part of the requested host-import misuse matrix; setter barriers do not force the remove/insert gap (`crates/shamir-wasm-host/src/tests/wasm/wasm_tests.rs:365`; `crates/shamir-wasm-host/src/tests/runtime/context_tests.rs:69`). Triage: fixed (tests only), #404 (`a10cb72c`). The registry setter test is overwhelmingly but not mathematically deterministic; the context/globals stress tests count only demonstrably overlapped reads.

### G5d — #347

Unix waiter-start failure ignores kill results and waits without a bound; the reviewer also identified a waiter-error reap gap (`crates/shamir-proc-tree/src/process_tree_unix.rs:29`, `crates/shamir-proc-tree/src/process_tree_unix.rs:62`). Windows startup failure has a similar unchecked-kill/unbounded-wait path (`crates/shamir-proc-tree/src/process_tree_windows.rs:23`). Triage: fixed, #404 (`a10cb72c`).

### G10 — #334

The squash removed the root `ResultEncoding` export while the enum remained public at its nested location (`crates/shamir-query-types/src/lib.rs:49`; `crates/shamir-query-types/src/ops/batch/req/result_encoding.rs:14`). The downstream source break is inferred. Triage: fixed, #396 (`eda2115d`).

### H5 — #323

The reviewer found permissive unknown fields, untagged `FilterValue` trials and a MessagePack re-encode/typed-decode dispatch trampoline (`crates/shamir-query-types/src/ops/batch/ops/batch_op.rs:382`; `crates/shamir-query-types/src/read/core/read_query.rs:12`; `crates/shamir-query-types/src/read/filter/filter_value.rs:9`). The RU6 test did not exercise the DROP response envelope through the handler (`crates/shamir-server/src/engine/db_handler/tests/misc/record_id_wire_roundtrip_tests.rs:62`). Triage: RU6 and G10 in #396; deterministic `FilterValue` decoding and single-pass `BatchOp` dispatch (typed u64 above `i64::MAX` now decodes) in #404 (`a10cb72c`); the producer/DTO wire fixture guard and request-side unknown-key rejection follow in batch 2 with known remaining gaps: the Filter family (shared with persisted schema defaults), unit variants of internally tagged enums (serde ignores `deny_unknown_fields` there) and the generic error text of untagged access references.

### H8 — #341

The original squash exposed codec error payloads and left part of macro accumulation unfinished; #356 supplies builder-owned errors and completes the loop (`crates/shamir-query-builder/src/wire/error.rs:3`; `crates/shamir-query-builder-macros/src/query_parse_lower.rs:164`). Triage: already closed by H8b; no second fix. The original reviewer FAIL remains in the table.

### H11 — #330

`RV-FUNCLIB-8B0C96FB`: the review found conditional multi-thread offload and an explicit current-thread inline path (`crates/shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs:38`; `crates/shamir-engine-query/src/query/filter/evaluation/semantics/tests/resolve_offload_tests.rs:53`). Triage: fixed under @oh's amended acceptance, #404 (`a10cb72c`): argon2 waits off the runtime workers on multi-thread runtimes (`block_in_place`, no nested blocking-pool task), inline with warn-once and counter on current-thread runtimes.

### RG1 — #345

The scanner checks method-call atomics but its function-call visitor recognizes only fences, leaving associated-function/UFCS atomics outside the gate (`tools/rust-guard/src/orderings_cmd/scan.rs:436`; `tools/rust-guard/src/orderings_cmd/scan.rs:461`). The missed weakened-load outcome was inferred, not executed. Triage: #394, merged.

## Unscheduled nits

These are reviewer qualifications for `OK_WITH_NITS` PRs, recorded for owner decision and **not scheduled**. References are the reviewers' source locations at review time; test limitations are not claims of test execution.

### shamir-storage

- #314: `max_entries` test does not prove dirty-entry cardinality is capped (`crates/shamir-storage/src/tests/stores/storage_membuffer_p1_tests/rv_storage_regression_tests.rs:98`).
- #332: replacement stress barrier does not force the old remove/reinsert gap (`crates/shamir-storage/src/tests/stores/storage_in_memory_tests.rs:156`).

### shamir-wal

- #311: append validates magic/version rather than every decodable payload; sign-off follows the brief's documented-precondition alternative (`crates/shamir-wal/src/entry/wal_entry_v2.rs:301`; `docs/dev-artifacts/prompts/remediation/183-p1-wal-sealed-replay.md:39`).

### shamir-tx

- #312: rebuild comment claims partial entries are cleared, but reviewed code resets readiness and inserts (`crates/shamir-tx/src/mvcc_store/store.rs:376`).
- #329: handoff oracle is sequential, not a competing opener (`crates/shamir-tx/src/tests/gate_tests/repo_tx_gate_tests.rs:737`); shared-layout import remains mid-file (`crates/shamir-tx/src/gate/predicate_set.rs:163`).

### shamir-index

- #359: bounded heap does not bound full postings/scores working set (`crates/shamir-index/src/backends/fts_ranked_backend.rs:376`); grandfathered credentials may still be rewritten in cleartext, so this is not blanket F10 closure (`crates/shamir-index/src/model/meta/persistence.rs:93`). Multiple public types remain together (`crates/shamir-index/src/fts/bm25.rs:9`).

### shamir-db

- #320: relocated test import remains function-local (`crates/shamir-db/src/shamir_db/tests/access/admin_access_validation_tests.rs:759`).
- #350: compatibility shim remains and demo-removal nit was not established (`crates/shamir-db/src/api/types.rs:6`); public error type is co-located (`crates/shamir-db/src/shamir_db/gateway/ports.rs:33`).
- #357: filtered repository lookup still scans rather than being keyed (`crates/shamir-db/src/shamir_db/system_store/catalogue.rs:324`).
- #378: compiled-guest e2e can return without assertions if toolchain unavailable (`crates/shamir-db/tests/functions/guest_error_kind_e2e.rs:24`).

### shamir-engine and related crates

- #348: on-update extraction lacks a direct new regression and both extraction docstrings overstate decoder-error conditions (`crates/shamir-engine/src/query/batch/fk/fk_on_update/scan.rs:179`; `crates/shamir-engine/src/query/batch/fk/fk_actions/helpers.rs:208`).
- #368: defensive fallback lacks a dedicated behavioural regression (`crates/shamir-engine/src/query/batch/runner/query_runner/runner.rs:214`); behavioural test names omit card suffixes (`crates/shamir-engine/src/table/tests/admission/outcome_tests.rs:6`).
- #371: Loom oracle exercises `begin_drain`/`is_drained`, not the async wrapper (`crates/shamir-engine/src/table/support/guards/tests/loom_model.rs:98`).

### shamir-numa

- #353: host pinning test can skip when no eligible node exists and retains a node-zero name (`crates/shamir-numa/tests/linux_topology.rs:17`).

### shamir-server

- #319: budget and version oracles exercise helpers rather than production call sites (`crates/shamir-server/src/engine/db_handler/tests/budget/response_budget_scope_tests.rs:11`; `crates/shamir-server/src/tests/engine/connection/query_version_tests.rs:3`).
- #342: stalled-writer test does not independently establish listener cancellation or repository-flush ordering (`crates/shamir-server/src/tests/engine/connection/handshake_e2e_tests.rs:630`).
- #337: heartbeat settling assumes a fixed number of scheduler yields (`crates/shamir-server/src/tests/engine/connection/ws/heartbeat_tests.rs:21`); local imports remain (`crates/shamir-server/src/tests/engine/connection/handshake_e2e_tests.rs:184`).

### shamir-connect

- #315: durable gate still serializes materialization and cloning (`crates/shamir-connect/src/server/infra/audit_chain.rs:506`).
- #325: resume privacy and rotation tests do not deterministically distinguish the old implementations (`crates/shamir-connect/src/server/tests/session_flow/resume_regression_tests.rs:117`; `crates/shamir-connect/src/server/tests/session_flow/rotation_tests.rs:16`); the claimed exhaustive test is a list, not an exhaustive match (`crates/shamir-connect/src/common/tests/types_tests/error_tests.rs:1`).

### transports

- #318: framing tests use a wall-clock timeout and source-string cancellation oracle (`crates/shamir-transport-tcp/src/tests/framing_tests.rs:120`); proxy e2e does not independently prove CertificateVerify or pin persistence (`crates/shamir-server/tests/e2e/transport/rv_transport_tcp_1257db79_tls_proxy_e2e.rs:174`); TLS file has multiple public types (`crates/shamir-transport-tcp/src/tls.rs:28`).
- #337: `HeartbeatState` adds another public type to the framing file (`crates/shamir-transport-ws/src/framing.rs:308`).
- #360: Windows DACL coverage does not execute denial under another principal (`crates/shamir-transport-ipc/src/tests/windows_tests.rs:54`); local import remains (`crates/shamir-transport-ipc/src/endpoint.rs:7`). Logical names were deliberately not implemented under RU3 despite the title (`docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:138`).

### shamir-client and shamir-client-node

- #328: borrowed-envelope and lock checks are source-shape tests; select-bias regression is probabilistic (`crates/shamir-client/src/tests/client/lifecycle_tests.rs:632`; `crates/shamir-client/src/tests/client/demux_tests.rs:36`). Push routing retains a standard mutex (`crates/shamir-client/src/client/transport.rs:206`); `Protocol(String)` remains a typed-error exception (`crates/shamir-client/src/error.rs:34`).
- #339: empty squash; credit H2 implementation and tests to #364 (`crates/shamir-client/src/interner_cache/cache.rs:169`).
- #364: merge-time scratch assertion did not prove wiping; #375 adds a capacity-byte oracle (`crates/shamir-client/src/tests/auth/resume_and_scratch_tests.rs:151`, `crates/shamir-client/src/tests/auth/resume_and_scratch_tests.rs:186`); resume coverage uses timed retries (`crates/shamir-client/src/tests/auth/resume_and_scratch_tests.rs:120`).
- #375: fake-server negative cases accept any handshake error rather than the specific cryptographic rejection (`crates/shamir-client/src/tests/auth/auth_ok_hints_tests.rs:398`).
- #317: buffer mutation test is race-stress, not deterministic; hand-built wire operation lacks an exception explanation (`tests/e2e/tests/01-connect.test.js:50`; `tests/e2e/tests/01-connect.test.js:17`).

### shamir-query-types and shamir-query-builder

- #313: guest stack safety remains unverified despite the explicit nesting limit (`crates/shamir-sdk/src/api/value.rs:22`).
- #346: function-local imports remain in HMAC implementation (`crates/shamir-query-types/src/security/hmac.rs:181`).
- #356: error-variant tests do not independently pin owned payload conversion (`crates/shamir-query-builder/src/wire/tests/wire_tests.rs:14`); handwritten error and single-line map literal remain (`crates/shamir-query-builder/src/wire/error.rs:8`; `crates/shamir-query-builder/src/wire/response/tests/response_tests.rs:171`). Macro interpolation test is source-text coupled (`crates/shamir-query-builder-macros/src/tests/lowering_tests.rs:44`).

### shamir-sdk and shamir-sdk-macros

- #363: pagination is explicitly deferred under RU10, not closed (`crates/shamir-sdk/src/api/db.rs:112`); native import mocks do not prove WASM transport (`crates/shamir-sdk/src/tests/value/h9/host_import_tests.rs:18`); malformed host values retain User classification (`crates/shamir-sdk/src/api/db.rs:99`).
- #367: expansion oracles are structural rather than compiled edge-signature execution (`crates/shamir-sdk-macros/src/tests/return_type_tests.rs:128`); #378, not #367, completes typed host error transport (`crates/shamir-sdk/src/runtime/__rt.rs:226`).
- #378: macro expansion checks are structural, not compiled-guest behavioural tests (`crates/shamir-sdk-macros/src/tests/raise_path_tests.rs:7`).

### shamir-funclib

- #344: expected hashes are computed from literal bytes rather than frozen digest literals (`crates/shamir-funclib/src/functions/value/canonical/tests/canonical_vectors_tests.rs:15`); `CanonicalError` is handwritten instead of the specified `thiserror` (`crates/shamir-funclib/src/functions/value/canonical/core.rs:48`).

### shamir-types

- #361: import-hygiene test checks mapping, not imports (`crates/shamir-types/src/core/interner/tests/publication_cards.rs:114`); duplicate detection contradicts the retained allocation-free documentation (`crates/shamir-types/src/codecs/interned/validate_keys.rs:7`). #370 delivers the deferred finding.
- #366: examples use function-local imports (`crates/shamir-types/src/codecs/README.md:53`); ID tests extend an inline wrapper (`crates/shamir-types/src/types/tests/record_id_tests.rs:1`). New-ID timestamp clamping does not reclassify historical arbitrary bytes (`crates/shamir-types/src/types/ids/record_id.rs:17`).

### shamir-bench-utils

- #358: unwind release has no direct panic oracle in the reviewed tests (`crates/shamir-bench-utils/src/peak_mem/tests/measurement_tests.rs:32`); generator documentation still calls the consumer a criterion bench (`crates/shamir-bench-utils/src/vector_data.rs:3`).

## Process notes

Static review cannot establish runtime behaviour, real scheduling, resource usage, guest stack safety, or cross-platform behaviour; individual tests may also skip or have nondeterministic oracles. References identify reviewer evidence at audit time and may move as later fixes land. No CI results were independently verified in the reviewer reports. GitHub CI is the authority for builds, tests and cross-platform execution; this report is not a green-CI attestation.

## Remediation record

The fix stages were reviewed individually by a read-only reviewer (several needed one to four rounds; first-pass reviewer FAILs were mostly wrong test oracles, a dropped serde callback, a missing trailing-element check and a nested blocking-pool deadlock, all fixed before landing). They landed as #394, #396 and the integration batch #404 (`a10cb72c`), verified by one full CI run on the exact head in the fork (all jobs green); batch 2 (producer/DTO wire guard and request-side unknown-key rejection) follows. Open follow-ups, none scheduled: the Filter family and internally tagged unit variants for unknown-key rejection, untagged access-reference error text, the curl multi-address `--resolve` emission, and the nit lists above.
