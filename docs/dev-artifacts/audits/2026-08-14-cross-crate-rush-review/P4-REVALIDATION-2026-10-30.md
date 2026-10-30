# P4 - revalidation of the active findings against current master

Navigation: [P4](P4.md) - [P3](P3.md) - [P2 revalidation](P2-REVALIDATION-2026-10-29.md) - [P2 sprint report](P2-SPRINT-REPORT-2026-10-30.md)

Revalidated on 2026-10-30 against upstream master `96d694f5` (after the P0/P1 remediation waves, the P2 sprint landed as PR #406 / `aaa22258`, and the CI trigger PR #407). The previous revalidation of [P4](P4.md) was made against `b0e1ddf7` on 2026-10-24. This document records the current state only; no code was changed by this stage.

## Method

- Scope: the 169 active cards of [P4](P4.md) (status `confirmed-open`, `partially-fixed` or `unverified` in its table "Активные / partial / unverified facets"). Cards already `fixed` / `refuted` / `not-applicable` and the higher-risk-only redirects are out of scope. Most P3/P4 cards are facets of a P0/P1/P2 mechanism; the facet itself was decided, not the parent.
- Pass 1: 19 read-only rush reviewer sessions (B01-B19, grouped by crate, 340 P3 + 169 P4 cards across both documents). Each card was re-read in [P4](P4.md), located in the live code by symbol (old line numbers are stale after the layout campaign and the P2 sprint), checked against `git log b0e1ddf7..HEAD` and given one verdict.
- Pass 2 (skeptical, independent): 21 sessions (S01-S21) re-decided EVERY one of the 509 cards of P3 and P4. Because pass 1 closed 81% of the cards as `FIXED`, the usual selection (all `FIXED`/`CHANGED`/`PARTIAL`/`UNVERIFIABLE`, high/medium `STILL_OPEN`, a sample of low/nit `STILL_OPEN`) degenerated into all cards. The reviewer decided its own verdict from the live code BEFORE weighing the first-pass claim. Where the two passes disagree, the pass-2 verdict is used here and the card is listed under "Contested cards".
- Everything here is static analysis. A verdict is evidence from source and history, not a test result; CI remains the authority.

## Result

| Status | Cards |
|---|---|
| confirmed-open | 14 |
| partially-fixed | 22 |
| unverified | 1 |
| fixed | 132 |
| **Total** | 169 |

Still active (`confirmed-open` + `partially-fixed` + `changed` + `unverified`): **37**. Closed since 2026-10-24 (`fixed` + `refuted`): **132**.

Transitions against the status recorded on 2026-10-24:

| Was | Now | Cards |
|---|---|---|
| confirmed-open | fixed | 126 |
| confirmed-open | partially-fixed | 18 |
| confirmed-open | confirmed-open | 14 |
| partially-fixed | fixed | 6 |
| partially-fixed | partially-fixed | 4 |
| unverified | unverified | 1 |

Pass 2 disagreed with pass 1 on **3** of the 169 cards of this document (100% of them were first-pass `FIXED`). Disagreement directions: FIXED -> PARTIAL: 3.

Risk grade of the active cards (after re-grading):

| Risk | Active cards |
|---|---|
| low | 2 |
| nit | 35 |
| **Total** | 37 |

Re-graded among the active cards: 8 (-->nit 1, high->nit 1, low->nit 3, medium->nit 1, nit->low 2).

Fix size and owner-decision flag of the active cards (pass-2 value):

| Size | Cards |
|---|---|
| S | 17 |
| M | 17 |
| L | 3 |
| **Total** | 37 |

Owner-decision flag set on **14** active cards.

## Active cards by crate

Columns: status change, risk (old -> new), fix size, owner decision, and what remains (pass-2 note; the first-pass recipe where one exists).

### shamir-connect (2 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-CONNECT-7D41FB60](P4.md#l4-rv-connect-7d41fb60) | confirmed-open -> partially-fixed | nit | M | yes | Resolve E4631CD9 once and close this aggregate by reference; keep environmental panic debt separate. / Fixture reference and suppressors are corrected; resolve the public String KDF error through E4631CD9, keeping environmental failures separate. |
| [L4-RV-CONNECT-E4631CD9](P4.md#l4-rv-connect-e4631cd9) | confirmed-open -> confirmed-open | nit | M | yes | Replace the public String error with an agreed typed error and migrate callers and tests. / Public String error API and diagnostic discard remain; stricter preceding caps prevent this helper's rejection in the current handshake. |

### shamir-server (3 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-SERVER-8999F6B7](P4.md#l4-rv-server-8999f6b7) | confirmed-open -> partially-fixed | nit | M | no | Consume shared base, hinted and resume byte fixtures in tests of both actual production DTOs. / Exact server fixture still decodes a test mirror; shared base, hinted and resume fixtures should exercise both actual production DTOs. |
| [L4-RV-SERVER-97C240D7](P4.md#l4-rv-server-97c240d7) | confirmed-open -> partially-fixed | nit | M | yes | Deprecate or remove plaintext argv input, or explicitly accept its compatibility and local-disclosure trade-off. / Stdin alternative and warning exist, but plaintext argv remains a live bootstrap input; removal or acceptance is a compatibility-policy decision. |
| [L4-RV-SERVER-A65118CC](P4.md#l4-rv-server-a65118cc) | confirmed-open -> partially-fixed | nit | M | yes | Choose a shared predicate owner and route both validators through it without changing accepted addresses. / Mapped-loopback behavior agrees, but independently maintained normative predicates remain; choose shared ownership without changing accepted addresses. |

### shamir-client (2 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-CLIENT-1C219006](P4.md#l4-rv-client-1c219006) | confirmed-open -> partially-fixed | nit | M | no | First pass missed public execute_with_receipts, which still creates interner cache entries without a legacy-peer version gate. **(contested)** |
| [L4-RV-CLIENT-66E9A5FC](P4.md#l4-rv-client-66e9a5fc) | confirmed-open -> confirmed-open | nit | M | no | Add allocation-count oracles and fixed-iteration client benchmarks with results recorded from CI. / Client allocation-count oracles and implemented benchmark measurements remain absent and explicitly deferred. |

### shamir-client-node (1 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-CLIENT-NODE-31D3BBE5](P4.md#l4-rv-client-node-31d3bbe5) | confirmed-open -> confirmed-open | nit | S | yes | Decide and document version alignment or intentional independent versioning during release preparation. / Version divergence remains unexplained; a release-policy note suffices and Cargo metadata does not drive loader rejection. |

### shamir-storage (3 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-STORAGE-3D6F974D](P4.md#l4-rv-storage-3d6f974d) | partially-fixed -> partially-fixed | nit | S | no | Flatten the remaining audit_2_3 wrapper while preserving test names and registration. / Race topics are split; audit wrapper remains. Flattening preserves leaf names but changes fully qualified discovery names. |
| [L4-RV-STORAGE-88C4F2CC](P4.md#l4-rv-storage-88c4f2cc) | confirmed-open -> confirmed-open | nit | L | yes | Approve a narrowly scoped compatibility-aware interface polish plan or retain current APIs. / Deferred interface and wording polish remains; external implementation is not blocked. |
| [L4-RV-STORAGE-B6B3CBB6](P4.md#l4-rv-storage-b6b3cbb6) | confirmed-open -> confirmed-open | nit | M | no | Measure allocations in CI, then optimize miss-key ownership and bounded reservation only where beneficial. / Miss-key cloning and fixed scan reservations remain; optimize only with CI allocation evidence and bounded reservation. |

### shamir-wal (4 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-WAL-0843B96B](P4.md#l4-rv-wal-0843b96b) | confirmed-open -> confirmed-open | nit | M | yes | Restrict the internal segment_meta namespace and align documentation references with its visibility. / Empty public namespace and private documentation links remain; narrowing a public module needs API approval. |
| [L4-RV-WAL-3F10E8A1](P4.md#l4-rv-wal-3f10e8a1) | confirmed-open -> confirmed-open | nit | M | yes | Define diagnostic disclosure policy and redact public path rendering while retaining protected operator context. / Returned errors render supplied paths unchanged; a remote disclosure route is not established. |
| [L4-RV-WAL-5DFE355F](P4.md#l4-rv-wal-5dfe355f) | confirmed-open -> confirmed-open | nit | S | yes | Restrict mark_poisoned to crate-local tests or an approved test-util feature. / Public ungated poison hook remains callable by a segment holder; restricting it changes public API. |
| [L4-RV-WAL-C66039FC](P4.md#l4-rv-wal-c66039fc) | confirmed-open -> partially-fixed | nit | S | no | Hoist remaining Future, Ordering and Poll imports into the recoverability test header. / Original Duration sites are repaired; ordinary Future, Ordering and Poll body imports still need hoisting. |

### shamir-tx (2 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-TX-10BB4C96](P4.md#l4-rv-tx-10bb4c96) | confirmed-open -> partially-fixed | nit | M | no | Replace remaining main-plus-history and unwired-overlay scaffold narration with current behavior. / Stage and timestamp prose improved; main-plus-history and production-empty-overlay descriptions remain stale. |
| [L4-RV-TX-1480F306-FACET](P4.md#l4-rv-tx-1480f306-facet) | confirmed-open -> partially-fixed | low -> nit | S | no | Correct the test generator rationale and admit separator bytes alongside the L3 codec-domain repair. / Production grammar explanation is repaired; test rationale and generator still exclude valid separator-containing keys. |

### shamir-engine (2 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-ENGINE-3C71057D](P4.md#l4-rv-engine-3c71057d) | partially-fixed -> partially-fixed | nit -> low | L | no | Reuse batch-scoped parent membership while preserving transaction-staged parent visibility. / Per-record FK probes still reach fresh full parent scans when indexes are absent or unavailable; batch membership must preserve staged visibility. |
| [L4-ENGINE-STYLE-DELIVERY-PROVENANCE](P4.md#l4-engine-style-delivery-provenance) | partially-fixed -> partially-fixed | - -> nit | S | yes | Record an accepted historical delivery exception and require isolated style commits prospectively. / Historical mixed delivery remains; accept an explicit exception and enforce isolated style delivery prospectively without hiding substantive commits. |

### shamir-db (3 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-DB-5977A782](P4.md#l4-rv-db-5977a782) | confirmed-open -> partially-fixed | nit -> low | M | no | Make snapshot absence authoritative with generation-safe invalidation on every group mutation. / Positive cache hits avoid scans; missing names still perform full catalogue reads and need generation-safe negative lookup. |
| [L4-RV-DB-0065D2A2](P4.md#l4-rv-db-0065d2a2) | confirmed-open -> confirmed-open | nit | M | yes | Explicitly retain informational fields as supported metadata or deprecate/remove them under a compatibility policy. / Write-only diagnostic hashes and constant function version remain; choose supported metadata or compatible deprecation. |
| [L4-RV-DB-7C59E01B](P4.md#l4-rv-db-7c59e01b) | confirmed-open -> confirmed-open | nit | M | yes | Choose bounded striped locking or an explicit retained-key budget without naive live-entry eviction. / Schema operations insert permanently retained distinct-key locks; use bounded striping or an explicit admission budget. |

### shamir-index (2 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-INDEX-20415F00](P4.md#l4-rv-index-20415f00) | confirmed-open -> partially-fixed | nit | S | no | First pass missed the remaining explicit two-variant enum claim in the lifecycle introduction despite its corrected later section. **(contested)** |
| [L4-RV-INDEX-32C98E9F](P4.md#l4-rv-index-32c98e9f) | confirmed-open -> partially-fixed | nit | M | no | Hoist remaining ordinary HNSW Entry variant imports to file headers without changing names or cfg scope. / Some imports moved, but ordinary production-local Entry imports remain in two HNSW files. |

### shamir-types (2 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-TYPES-178ED8CB](P4.md#l4-rv-types-178ed8cb) | confirmed-open -> confirmed-open | nit | M | no | Hoist remaining non-exempt test imports into their file headers. / Ordinary non-exempt test-body imports remain across multiple files; scoped hoisting is sufficient. |
| [L4-RV-TYPES-F8CF2706](P4.md#l4-rv-types-f8cf2706) | confirmed-open -> partially-fixed | nit | S | no | Correct the helper prose from two implementations to three without changing the API. / Alias and unsealed-helper guidance are corrected; implementation-count prose still says two while listing three. |

### shamir-collections (1 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-COLLECTIONS-201D84D3-FACET](P4.md#l4-rv-collections-201d84d3-facet) | confirmed-open -> partially-fixed | low -> nit | S | no | Explain the sanctioned collection-facade lint allowance immediately beside it. / Public documentation is repaired; the sanctioned lint allowance still lacks adjacent rationale. |

### shamir-query-types (1 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-QUERY-TYPES-CC015A3C](P4.md#l4-rv-query-types-cc015a3c) | confirmed-open -> confirmed-open | nit | L | yes | Retain accepted owned String defaults; require CI measurements before reconsidering ownership. / Construction and bare-string deserialization still allocate main; retain accepted ownership cost unless measurements justify an owner-approved API redesign. |

### shamir-query-builder (2 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-QUERY-BUILDER-0E30EB25](P4.md#l4-rv-query-builder-0e30eb25) | confirmed-open -> partially-fixed | nit | S | no | Replace obsolete panicking IntoBatchOp commentary with current fallible registration and conversion behavior. / Constructor and macro names are repaired; obsolete panicking IntoBatchOp commentary remains. |
| [L4-RV-QUERY-BUILDER-21560B4A](P4.md#l4-rv-query-builder-21560b4a) | confirmed-open -> partially-fixed | nit | S | yes | Explicitly accept qualified use as policy or authorize additive disambiguated aliases without renaming existing exports. / Qualified-use guidance mitigates but does not remove simultaneous-glob ambiguity; prefer explicit policy acceptance. |

### shamir-wasm-host (2 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-WASM-HOST-C43E9092](P4.md#l4-rv-wasm-host-c43e9092) | confirmed-open -> partially-fixed | nit | M | no | Remove internal audit identifiers from both public gateway-error descriptions while retaining substantive explanations. / Old finding-2c references are removed, but internal audit IDs remain in two public error descriptions. |
| [L4-RV-WASM-HOST-FF7C2D6E](P4.md#l4-rv-wasm-host-ff7c2d6e) | partially-fixed -> partially-fixed | nit | S | no | Document the retained lossy metadata adapter and direct persisted-data consumers to checked try_from_record decoding. / Document the retained public adapter as lossy compatibility decoding; production recovery already uses checked decoding. |

### shamir-sdk (1 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-SDK-MACROS-FEC0E5E4-FACET](P4.md#l4-rv-sdk-macros-fec0e5e4-facet) | confirmed-open -> confirmed-open | medium -> nit | S | no | Replace the obsolete pure-only claim with the shared one-poll, no-suspension contract for all wrapper kinds. / The shared driver still claims pure-only support despite all wrapper kinds using its one-poll implementation. |

### shamir-numa (1 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-NUMA-6E1761A2-FACET](P4.md#l4-rv-numa-6e1761a2-facet) | confirmed-open -> confirmed-open | high -> nit | S | no | Replace the numerical mirror-window claim with a scheduling- and contention-dependent interval without a time bound. / Unsupported few-nanoseconds mirror-window prose survives; replace with scheduling- and contention-dependent wording. |

### shamir-bench-utils (3 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L4-RV-BENCH-UTILS-4E7A9919](P4.md#l4-rv-bench-utils-4e7a9919) | confirmed-open -> partially-fixed | nit | S | no | First pass missed the workload comment still calling an unversioned five-field tuple the reproduction key while current contracts require six fields. **(contested)** |
| [L4-RV-BENCH-UTILS-67097ED5-FACET](P4.md#l4-rv-bench-utils-67097ed5-facet) | confirmed-open -> partially-fixed | low -> nit | S | no | Replace the surviving criterion-bench module-header phrase with current Harness terminology. / Harness integration is corrected but the vector-data module header still says criterion bench. |
| [L4-RV-BENCH-UTILS-F53F775F](P4.md#l4-rv-bench-utils-f53f775f) | unverified -> unverified | nit | S | no | Add zero-noise per-row construction-identity coverage and keep noisy geometric assertions separate without arbitrary tolerance weakening. / Nearest-centroid counts are not construction labels, but no separation proof or affected-target failure witness establishes the deterministic fixture flake; ... |

## Closed since 2026-10-24

`fixed` = the mechanism is gone and the evidence points at the live code (or the commit); `refuted` = the premise is false at HEAD with positive counter-evidence. `aaa22258` is the P2 sprint squash commit.

### shamir-connect (18 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-CONNECT-34144ECE](P4.md#l4-rv-connect-34144ece) | fixed | crates/shamir-connect/src/server/state/session.rs:418; crates/shamir-connect/src/server/infra/audit_chain.rs:145; crates/shamir-connect/src/server/ops/admin.rs:299; crates/shamir-connect/src/server/auth/bootstrap.rs:45; crates/... |
| [L4-RV-CONNECT-349809B3](P4.md#l4-rv-connect-349809b3) | fixed | crates/shamir-connect/src/common/user/username.rs:8; crates/shamir-connect/src/common/user/username.rs:38 |
| [L4-RV-CONNECT-3BAF631D](P4.md#l4-rv-connect-3baf631d) | fixed | crates/shamir-connect/src/server/auth/changepw.rs:62; crates/shamir-connect/src/server/tests/session_flow/changepw_challenge_tests.rs:254 |
| [L4-RV-CONNECT-4496810D](P4.md#l4-rv-connect-4496810d) | fixed | crates/shamir-connect/src/server/state/session.rs:112 |
| [L4-RV-CONNECT-66EA32BD](P4.md#l4-rv-connect-66ea32bd) | fixed | crates/shamir-connect/src/common/messages/envelope.rs:95 |
| [L4-RV-CONNECT-68CAD3B6](P4.md#l4-rv-connect-68cad3b6) | fixed | crates/shamir-connect/src/server/limits/lockout.rs:256-259; crates/shamir-connect/src/server/limits/lockout.rs:286-299 |
| [L4-RV-CONNECT-699EBFCF](P4.md#l4-rv-connect-699ebfcf) | fixed | crates/shamir-connect/src/server/ops/admin.rs:15; crates/shamir-connect/src/server/tests/security/post_auth_rate_limit_tests.rs:13-14 |
| [L4-RV-CONNECT-850A9AFD](P4.md#l4-rv-connect-850a9afd) | fixed | crates/shamir-connect/src/server/auth/handshake.rs:238-245; crates/shamir-connect/src/server/auth/handshake.rs:402-419 |
| [L4-RV-CONNECT-8B168854](P4.md#l4-rv-connect-8b168854) | fixed | crates/shamir-connect/src/server/infra/audit_chain.rs:391-432 |
| [L4-RV-CONNECT-90EC2E05](P4.md#l4-rv-connect-90ec2e05) | fixed | crates/shamir-connect/src/server/limits/rate_limit.rs:89-93 |
| [L4-RV-CONNECT-94E37810](P4.md#l4-rv-connect-94e37810) | fixed | crates/shamir-connect/src/common/model/error.rs:129-171; crates/shamir-connect/src/common/tests/types_tests/error_tests.rs:24-70; crates/shamir-connect/src/common/tests/types_tests/mod.rs:1 |
| [L4-RV-CONNECT-A57D515B](P4.md#l4-rv-connect-a57d515b) | fixed | crates/shamir-connect/src/server/limits/argon2_semaphore.rs:10-14; crates/shamir-connect/src/server/limits/argon2_semaphore.rs:31-33; crates/shamir-connect/src/server/limits/argon2_semaphore.rs:82-103 |
| [L4-RV-CONNECT-C75B520F](P4.md#l4-rv-connect-c75b520f) | fixed | crates/shamir-connect/src/common/tests/message_tests/push_envelope_tests.rs:4-14; crates/shamir-connect/src/common/tests/message_tests/mod.rs:2 |
| [L4-RV-CONNECT-C7F16BFB](P4.md#l4-rv-connect-c7f16bfb) | fixed | crates/shamir-connect/Cargo.toml:34-43 |
| [L4-RV-CONNECT-C8866EB1](P4.md#l4-rv-connect-c8866eb1) | fixed | crates/shamir-connect/src/common/messages/auth_message.rs:6; crates/shamir-connect/src/common/messages/envelope.rs:90; crates/shamir-connect/tests/session/integration_session.rs:333 |
| [L4-RV-CONNECT-E6CF11C8](P4.md#l4-rv-connect-e6cf11c8) | fixed | crates/shamir-connect/src/common/messages/auth_message.rs:80-97 |
| [L4-RV-CONNECT-E71CB30A](P4.md#l4-rv-connect-e71cb30a) | fixed | crates/shamir-connect/src/server/auth/changepw.rs:86-108; crates/shamir-server/src/engine/db_handler/handlers/admin.rs:579; crates/shamir-server/src/engine/db_handler/handlers/admin.rs:634 |
| [L4-RV-CONNECT-EAB07B19](P4.md#l4-rv-connect-eab07b19) | fixed | crates/shamir-connect/src/server/infra/durable_counters.rs:60-65 |

### shamir-server (3 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-SERVER-4512C85D](P4.md#l4-rv-server-4512c85d) | fixed | crates/shamir-server/src/sync/connection/request_loop.rs:503-513 |
| [L4-RV-SERVER-72CEAF9E](P4.md#l4-rv-server-72ceaf9e) | fixed | crates/shamir-server/src/engine/db_handler/handlers/admin.rs:18 |
| [L4-RV-SERVER-9C77C713](P4.md#l4-rv-server-9c77c713) | fixed | crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs:468-488 |

### shamir-client (5 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-CLIENT-0157DABC](P4.md#l4-rv-client-0157dabc) | fixed | crates/shamir-client/src/error.rs:95-98; crates/shamir-client/src/client/transport.rs:655-672 |
| [L4-RV-CLIENT-153A118E](P4.md#l4-rv-client-153a118e) | fixed | crates/shamir-client/src/client/transport.rs:126-149 |
| [L4-RV-CLIENT-CFE6B78B](P4.md#l4-rv-client-cfe6b78b) | fixed | crates/shamir-client/src/client/connect.rs:9,701-705 |
| [L4-RV-CLIENT-E39C43D4](P4.md#l4-rv-client-e39c43d4) | fixed | crates/shamir-client/src/tests/receipts/batch_has_refs_tests.rs:12; crates/shamir-client/src/tests/client/demux_tests.rs:7-18; crates/shamir-client/src/tests/wire/wire_version_tests.rs:8-9 |
| [L4-RV-CLIENT-F1276114](P4.md#l4-rv-client-f1276114) | fixed | crates/shamir-client/src/tests/client/timeout_tests.rs:32-50; crates/shamir-client/src/client/transport.rs:160-189 |

### shamir-client-node (1 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-CLIENT-NODE-4B69BB13](P4.md#l4-rv-client-node-4b69bb13) | fixed | crates/shamir-client-node/src/lib.rs:255-258,285-288 |

### shamir-transport-tcp (13 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-TRANSPORT-TCP-05C670C2](P4.md#l4-rv-transport-tcp-05c670c2) | fixed | crates/shamir-transport-tcp/tests/tls13_only.rs:7-21; crates/shamir-transport-tcp/tests/fixtures/handshake.rs:1-6 |
| [L4-RV-TRANSPORT-TCP-16C543F3](P4.md#l4-rv-transport-tcp-16c543f3) | fixed | crates/shamir-transport-tcp/tests/handshake_e2e.rs:33-37; crates/shamir-transport-tcp/tests/echo_e2e.rs:45-49; crates/shamir-transport-tcp/tests/fixtures/handshake.rs:8-73 |
| [L4-RV-TRANSPORT-TCP-4D95A5F8](P4.md#l4-rv-transport-tcp-4d95a5f8) | fixed | crates/shamir-transport-tcp/src/framing.rs:187-196,231-241,285-310 |
| [L4-RV-TRANSPORT-TCP-5CE7BA5B](P4.md#l4-rv-transport-tcp-5ce7ba5b) | fixed | crates/shamir-transport-tcp/tests/handshake_e2e.rs:217-222; crates/shamir-transport-tcp/tests/echo_e2e.rs:313-318 |
| [L4-RV-TRANSPORT-TCP-6062E565](P4.md#l4-rv-transport-tcp-6062e565) | fixed | crates/shamir-transport-tcp/src/tls.rs:128-142,153-176 |
| [L4-RV-TRANSPORT-TCP-6392C63D](P4.md#l4-rv-transport-tcp-6392c63d) | fixed | crates/shamir-transport-tcp/src/tests/mod.rs:1-4; crates/shamir-transport-tcp/src/lib.rs:15-16; crates/shamir-transport-tcp/src/framing.rs:323-325 |
| [L4-RV-TRANSPORT-TCP-7DFC88FF](P4.md#l4-rv-transport-tcp-7dfc88ff) | fixed | crates/shamir-transport-tcp/src/tls.rs:40-55,73,90-96,115 |
| [L4-RV-TRANSPORT-TCP-9C706A16](P4.md#l4-rv-transport-tcp-9c706a16) | fixed | crates/shamir-transport-tcp/Cargo.toml:14,34-35 |
| [L4-RV-TRANSPORT-TCP-A13A3534](P4.md#l4-rv-transport-tcp-a13a3534) | fixed | crates/shamir-transport-tcp/src/listener.rs:30-34,71-84 |
| [L4-RV-TRANSPORT-TCP-B48A0EF2](P4.md#l4-rv-transport-tcp-b48a0ef2) | fixed | crates/shamir-transport-tcp/src/tls.rs:62-65,83-86 |
| [L4-RV-TRANSPORT-TCP-CD312411](P4.md#l4-rv-transport-tcp-cd312411) | fixed | crates/shamir-transport-tcp/src/tls.rs:16-19 |
| [L4-RV-TRANSPORT-TCP-EC4F02F5](P4.md#l4-rv-transport-tcp-ec4f02f5) | fixed | crates/shamir-transport-tcp/src/tls.rs:58,137 |
| [L4-RV-TRANSPORT-TCP-FA40B59D](P4.md#l4-rv-transport-tcp-fa40b59d) | fixed | crates/shamir-transport-tcp/src/listener.rs:97-99 |

### shamir-transport-ws (10 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-TRANSPORT-WS-0B4767DE](P4.md#l4-rv-transport-ws-0b4767de) | fixed | crates/shamir-transport-ws/src/framing.rs:11-18,245-264 |
| [L4-RV-TRANSPORT-WS-3D98F208](P4.md#l4-rv-transport-ws-3d98f208) | fixed | crates/shamir-transport-ws/src/browser.rs:29-31,60-64,184 |
| [L4-RV-TRANSPORT-WS-460A02D6](P4.md#l4-rv-transport-ws-460a02d6) | fixed | crates/shamir-transport-ws/Cargo.toml:25-37 |
| [L4-RV-TRANSPORT-WS-5AF6EE84](P4.md#l4-rv-transport-ws-5af6ee84) | fixed | crates/shamir-transport-ws/src/framing.rs:53-54,239-243; crates/shamir-transport-ws/src/server.rs:61-66 |
| [L4-RV-TRANSPORT-WS-5D3BB859](P4.md#l4-rv-transport-ws-5d3bb859) | fixed | crates/shamir-transport-ws/src/server.rs:147-153,239-251 |
| [L4-RV-TRANSPORT-WS-82800EF6](P4.md#l4-rv-transport-ws-82800ef6) | fixed | crates/shamir-transport-ws/src/server.rs:5-7; crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs:351-363 |
| [L4-RV-TRANSPORT-WS-82B13B1B](P4.md#l4-rv-transport-ws-82b13b1b) | fixed | crates/shamir-transport-ws/src/tls_exporter.rs:27-28; crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs:468 |
| [L4-RV-TRANSPORT-WS-B1243564](P4.md#l4-rv-transport-ws-b1243564) | fixed | crates/shamir-transport-ws/src/server.rs:140-153; crates/shamir-transport-ws/src/server.rs:227-247 |
| [L4-RV-TRANSPORT-WS-E6697C98](P4.md#l4-rv-transport-ws-e6697c98) | fixed | crates/shamir-transport-ws/src/server.rs:61-76; crates/shamir-transport-ws/src/framing.rs:245-258 |
| [L4-RV-TRANSPORT-WS-F6AAB91C](P4.md#l4-rv-transport-ws-f6aab91c) | fixed | crates/shamir-transport-ws/tests/framing_round_trip.rs:3 |

### shamir-transport-ipc (3 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-TRANSPORT-IPC-2072FE7B](P4.md#l4-rv-transport-ipc-2072fe7b) | fixed | crates/shamir-transport-ipc/Cargo.toml:10-20; crates/shamir-transport-ipc/src/windows.rs:87-100 |
| [L4-RV-TRANSPORT-IPC-400D803E](P4.md#l4-rv-transport-ipc-400d803e) | fixed | crates/shamir-transport-ipc/src/unix.rs:150-168; crates/shamir-transport-ipc/src/windows.rs:91-100; crates/shamir-transport-ipc/src/windows.rs:181-200 |
| [L4-RV-TRANSPORT-IPC-9B233842](P4.md#l4-rv-transport-ipc-9b233842) | fixed | crates/shamir-transport-ipc/src/windows.rs:12-14; crates/shamir-transport-ipc/src/windows.rs:173 |

### shamir-storage (5 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-STORAGE-458CFAD8](P4.md#l4-rv-storage-458cfad8) | fixed | crates/shamir-storage/src/stores/storage_fjall.rs:900-907 |
| [L4-RV-STORAGE-6FEF6FDD](P4.md#l4-rv-storage-6fef6fdd) | fixed | crates/shamir-storage/src/tests/types_tests.rs:4; crates/shamir-storage/src/core/types.rs:13-14 |
| [L4-RV-STORAGE-B26793CA](P4.md#l4-rv-storage-b26793ca) | fixed | crates/shamir-storage/src/test_seams/membuffer_clear_race_hook.rs:53-56; crates/shamir-storage/src/tests/stores/storage_membuffer_tests/clear_race_tests/batch_insert_republish_535.rs:1-10,32-54 |
| [L4-RV-STORAGE-B7703B8B](P4.md#l4-rv-storage-b7703b8b) | fixed | crates/shamir-storage/src/stores/storage_cached.rs:979-989; crates/shamir-storage/src/stores/storage_in_memory.rs:366-368; crates/shamir-storage/src/stores/storage_fjall.rs:975-984 |
| [L4-RV-STORAGE-F26D060C](P4.md#l4-rv-storage-f26d060c) | fixed | crates/shamir-storage/src/tests/atomic_batch_tests.rs:86-93; crates/shamir-storage/src/stores/storage_fjall.rs:986-988; crates/shamir-storage/src/key_bytes/tests/serde/serde_byte_identity_tests.rs:36-38 |

### shamir-wal (2 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-WAL-0CAFAF79](P4.md#l4-rv-wal-0cafaf79) | fixed | crates/shamir-wal/src/segment/wal_segment.rs:152; crates/shamir-wal/src/segment/wal_segment.rs:193 |
| [L4-RV-WAL-3735F642](P4.md#l4-rv-wal-3735f642) | fixed | crates/shamir-wal/src/segment/mem_frames.rs:117-149; crates/shamir-wal/src/segment/mem_frames.rs:160-214 |

### shamir-tx (8 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-TX-33897BD6](P4.md#l4-rv-tx-33897bd6) | fixed | crates/shamir-tx/src/tx/tx_context.rs:827-844 |
| [L4-RV-TX-43CD8A27](P4.md#l4-rv-tx-43cd8a27) | fixed | crates/shamir-tx/src/mvcc_store/store.rs:11-19; crates/shamir-tx/src/tx/support/layered_interner.rs:6-11; crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs:1-12 |
| [L4-RV-TX-5508B0C1](P4.md#l4-rv-tx-5508b0c1) | fixed | crates/shamir-tx/src/changefeed/feed/core.rs:131-164,183-216 |
| [L4-RV-TX-95E562E9](P4.md#l4-rv-tx-95e562e9) | fixed | crates/shamir-tx/src/gate/repo_tx_gate/progress.rs:70-77; crates/shamir-tx/src/mvcc_store/store.rs:615-619 |
| [L4-RV-TX-99487168](P4.md#l4-rv-tx-99487168) | fixed | crates/shamir-collections/src/lib.rs:20-46 |
| [L4-RV-TX-C935EAB9](P4.md#l4-rv-tx-c935eab9) | fixed | crates/shamir-tx/src/mvcc_store/mod.rs:22-23; crates/shamir-tx/src/gate/mod.rs:11-12; crates/shamir-tx/src/tx/tests/mod.rs:1-5; crates/shamir-tx/src/tests/mod.rs:1-2 |
| [L4-RV-TX-D2989766](P4.md#l4-rv-tx-d2989766) | fixed | crates/shamir-tx/src/mvcc_store/store.rs:730-734,771-776,870-874,898-902 |
| [L4-RV-TX-FB4FD0FE](P4.md#l4-rv-tx-fb4fd0fe) | fixed | crates/shamir-tx/src/tx/support/tests/metrics_tests.rs:5-68; crates/shamir-tx/src/tx/support/tests/mod.rs:3; crates/shamir-tx/src/tx/support/mod.rs:13-14 |

### shamir-engine (4 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-ENGINE-06B39B8F](P4.md#l4-rv-engine-06b39b8f) | fixed | crates/shamir-engine/src/table/table_manager.rs:675-676; crates/shamir-engine/src/table/table_manager/accessors/table_accessors.rs:132-133 |
| [L4-RV-ENGINE-6CCD199D](P4.md#l4-rv-engine-6ccd199d) | fixed | crates/shamir-engine/src/table/table_manager/accessors/table_accessors.rs:132-133; crates/shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs:334-344 |
| [L4-RV-ENGINE-931318A1](P4.md#l4-rv-engine-931318a1) | fixed | crates/shamir-engine-validator/src/validator/schema/rules/cross_field.rs:83-90; crates/shamir-engine-validator/src/validator/execution/record_fields.rs:142-151,166-179 |
| [L4-RV-ENGINE-CEBE3101](P4.md#l4-rv-engine-cebe3101) | fixed | crates/shamir-engine/src/table/read/planning/read_temporal.rs:664-669; crates/shamir-tx/src/mvcc_store/history/history_many.rs:25-59,109-120 |

### shamir-db (3 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-DB-77C5A24C](P4.md#l4-rv-db-77c5a24c) | fixed | crates/shamir-db/src/shamir_db/gateway/curl_gateway.rs:124-136,182-190 |
| [L4-RV-DB-C93A58BF](P4.md#l4-rv-db-c93a58bf) | fixed | crates/shamir-db/src/shamir_db/mod.rs:9-11 |
| [L4-RV-DB-CBB8403D](P4.md#l4-rv-db-cbb8403d) | fixed | crates/shamir-db/src/main.rs:3-7; crates/shamir-db/src/api/types.rs:1-15,40-44; crates/shamir-db/src/api/tests/api_tests.rs:5-10 |

### shamir-index (7 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-INDEX-54363D3B](P4.md#l4-rv-index-54363d3b) | fixed | crates/shamir-index/src/vector/quant/brute_force.rs:74-76,151-162 |
| [L4-RV-INDEX-858BB5E1](P4.md#l4-rv-index-858bb5e1) | fixed | crates/shamir-index/src/model/runtime/reader_drain_gate.rs:82-85; crates/shamir-index/src/base_index/index_manager/lookup.rs:406-441 |
| [L4-RV-INDEX-97E36480](P4.md#l4-rv-index-97e36480) | fixed | crates/shamir-index/src/model/kind.rs:10,24; crates/shamir-index/src/model/query/expr.rs:26 |
| [L4-RV-INDEX-AFA66311](P4.md#l4-rv-index-afa66311) | fixed | crates/shamir-index/src/model/runtime/reader_drain_gate.rs:305-307; crates/shamir-index/src/tests/model/reader_drain_loom.rs:1-25 |
| [L4-RV-INDEX-B23D8FE7](P4.md#l4-rv-index-b23d8fe7) | fixed | crates/shamir-index/src/vector/quant/quant_meta.rs:103; crates/shamir-index/src/vector/tests/quant/sq8_tests.rs:23-42; crates/shamir-index/src/vector/tests/quant/mod.rs:7 |
| [L4-RV-INDEX-B683BA68](P4.md#l4-rv-index-b683ba68) | fixed | crates/shamir-index/src/model/runtime/reader_drain_gate.rs:112; crates/shamir-index/src/base_index/meta/index_definition.rs:48 |
| [L4-RV-INDEX-DE92095C](P4.md#l4-rv-index-de92095c) | fixed | crates/shamir-index/src/model/runtime/write_ops.rs:163-189 |

### shamir-types (8 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-TYPES-12EEFFA6](P4.md#l4-rv-types-12eeffa6) | fixed | crates/shamir-types/src/codecs/basic/bincode.rs:5-18,35-45; crates/shamir-types/src/types/value_error.rs:7-24; crates/shamir-types/src/record_view/lens/cursor.rs:127-138 |
| [L4-RV-TYPES-2A360D16](P4.md#l4-rv-types-2a360d16) | fixed | crates/shamir-types/src/core/README.md:64-67,83-86; crates/shamir-types/src/core/interner/interner.rs:65-92 |
| [L4-RV-TYPES-313627EB](P4.md#l4-rv-types-313627eb) | fixed | crates/shamir-types/src/types/value.rs:664-704 |
| [L4-RV-TYPES-47A3CB10](P4.md#l4-rv-types-47a3cb10) | fixed | crates/shamir-types/src/macros/mpack.rs:293-296 |
| [L4-RV-TYPES-86E037B2](P4.md#l4-rv-types-86e037b2) | fixed | crates/shamir-types/src/core/interner/interner.rs:3; crates/shamir-types/src/types/ids/record_id.rs:5,99-104 |
| [L4-RV-TYPES-8AC2CFC5](P4.md#l4-rv-types-8ac2cfc5) | fixed | crates/shamir-types/src/types/tests/record_id_tests.rs:158-171 |
| [L4-RV-TYPES-A31DDF76](P4.md#l4-rv-types-a31ddf76) | fixed | crates/shamir-types/src/core/sort_codec.rs:134-150 |
| [L4-RV-TYPES-FC6867C2](P4.md#l4-rv-types-fc6867c2) | fixed | crates/shamir-types/src/types/value.rs:19-28 |

### shamir-collections (1 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-COLLECTIONS-4ED3931B](P4.md#l4-rv-collections-4ed3931b) | fixed | crates/shamir-collections/src/lib.rs:15-18 |

### shamir-tunables (1 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-TUNABLES-363B3879](P4.md#l4-rv-tunables-363b3879) | fixed | crates/shamir-tunables/src/tests/runtime_tests.rs:95-102 |

### shamir-query-types (4 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-QUERY-TYPES-3BFD1858](P4.md#l4-rv-query-types-3bfd1858) | fixed | crates/shamir-query-types/src/read/filter/tests/filter_enum_tests.rs:263; crates/shamir-query-types/src/read/filter/tests/filter_enum_tests.rs:278; crates/shamir-query-types/src/read/filter/tests/filter_enum_tests.rs:283 |
| [L4-RV-QUERY-TYPES-881BC94F](P4.md#l4-rv-query-types-881bc94f) | fixed | crates/shamir-query-types/src/admin/types/ddl/validator_ops.rs:63; crates/shamir-query-types/src/security/hmac.rs:7; crates/shamir-query-types/src/ops/batch/req/batch_limits.rs:17; crates/shamir-query-types/src/read/clause/limi... |
| [L4-RV-QUERY-TYPES-9223F400](P4.md#l4-rv-query-types-9223f400) | fixed | crates/shamir-query-types/src/read/core/read_query.rs:7; crates/shamir-query-types/src/admin/types/repl/repl_ops.rs:38; crates/shamir-query-types/src/admin/types/db/schema_ops.rs:222 |
| [L4-RV-QUERY-TYPES-D319B6E3](P4.md#l4-rv-query-types-d319b6e3) | fixed | crates/shamir-query-types/src/ops/write/types.rs:78; crates/shamir-query-types/src/ops/write/types.rs:91 |

### shamir-query-builder (1 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-QUERY-BUILDER-202E00CD](P4.md#l4-rv-query-builder-202e00cd) | fixed | crates/shamir-query-builder/src/read/select/select_item.rs:8 |

### shamir-query-builder-macros (7 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-QUERY-BUILDER-MACROS-1AF72B7B](P4.md#l4-rv-query-builder-macros-1af72b7b) | fixed | crates/shamir-query-builder-macros/src/filter_lower.rs:22; crates/shamir-query-builder-macros/src/filter_lower.rs:333; crates/shamir-query-builder-macros/src/query_parse_lower.rs:276 |
| [L4-RV-QUERY-BUILDER-MACROS-39AA68B5](P4.md#l4-rv-query-builder-macros-39aa68b5) | fixed | crates/shamir-query-builder-macros/src/filter_lower.rs:6; crates/shamir-query-builder-macros/src/filter_lower.rs:57-58 |
| [L4-RV-QUERY-BUILDER-MACROS-7E797992](P4.md#l4-rv-query-builder-macros-7e797992) | fixed | crates/shamir-query-builder-macros/src/query_parse.rs:249; crates/shamir-query-builder-macros/src/query_parse.rs:611-615; crates/shamir-query-builder-macros/src/filter_lower.rs:364 |
| [L4-RV-QUERY-BUILDER-MACROS-83FE5F55](P4.md#l4-rv-query-builder-macros-83fe5f55) | fixed | crates/shamir-query-builder/src/lib.rs:141-143; crates/shamir-query-builder-macros/src/query_parse_lower.rs:261-268 |
| [L4-RV-QUERY-BUILDER-MACROS-A690BEA8](P4.md#l4-rv-query-builder-macros-a690bea8) | fixed | crates/shamir-query-builder-macros/src/filter_lower.rs:123-136; crates/shamir-query-builder-macros/src/tests/lowering_tests.rs:49-59 |
| [L4-RV-QUERY-BUILDER-MACROS-E78E1666](P4.md#l4-rv-query-builder-macros-e78e1666) | fixed | crates/shamir-query-builder/src/lib.rs:123-124; crates/shamir-query-builder/src/lib.rs:214-217; crates/shamir-query-builder-macros/src/filter_lower.rs:303-309 |
| [L4-RV-QUERY-BUILDER-MACROS-EC1B86B3](P4.md#l4-rv-query-builder-macros-ec1b86b3) | fixed | crates/shamir-query-builder-macros/src/query_parse.rs:611-615; crates/shamir-query-builder-macros/src/query_parse.rs:679; crates/shamir-query-builder-macros/src/query_parse.rs:706 |

### shamir-funclib (14 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-FUNCLIB-06F1EB02](P4.md#l4-rv-funclib-06f1eb02) | fixed | crates/shamir-funclib/src/value_nav/core.rs:121-139 |
| [L4-RV-FUNCLIB-110F6E17](P4.md#l4-rv-funclib-110f6e17) | fixed | crates/shamir-funclib/src/functions/crypto/core.rs:167-194,264-288,332-341 |
| [L4-RV-FUNCLIB-37BE26F9](P4.md#l4-rv-funclib-37be26f9) | fixed | crates/shamir-funclib/src/functions/numeric/agg/core.rs:799-855; crates/shamir-funclib/src/functions/text/validate/core.rs:364-365; crates/shamir-funclib/src/functions/value/canonical/core.rs:162,186-187; crates/shamir-funclib/... |
| [L4-RV-FUNCLIB-3E2AD533](P4.md#l4-rv-funclib-3e2ad533) | fixed | crates/shamir-funclib/src/lib.rs:22; crates/shamir-funclib/src/scalar_resolver.rs:11; crates/shamir-funclib/src/functions/crypto/tests/argon2_gate_tests.rs:2-6 |
| [L4-RV-FUNCLIB-42F5C1C9-FACET](P4.md#l4-rv-funclib-42f5c1c9-facet) | fixed | crates/shamir-funclib/src/functions/numeric/math/core.rs:4-5; crates/shamir-funclib/src/functions/value/encode/core.rs:3-5; crates/shamir-funclib/src/lib.rs:58-71 |
| [L4-RV-FUNCLIB-50CD6B2E](P4.md#l4-rv-funclib-50cd6b2e) | fixed | crates/shamir-funclib/src/registry.rs:167-177 |
| [L4-RV-FUNCLIB-7539483C](P4.md#l4-rv-funclib-7539483c) | fixed | crates/shamir-funclib/src/functions/numeric/math/tests/mod.rs:1; crates/shamir-funclib/src/tests/registry/mod.rs:2 |
| [L4-RV-FUNCLIB-7EDC7D1C](P4.md#l4-rv-funclib-7edc7d1c) | fixed | crates/shamir-funclib/src/functions/text/strings/core.rs:132-141 |
| [L4-RV-FUNCLIB-8447708B](P4.md#l4-rv-funclib-8447708b) | fixed | crates/shamir-funclib/src/functions/value/encode/tests/encode_tests.rs:166-169; crates/shamir-funclib/src/functions/text/validate/tests/validate_tests.rs:188-191 |
| [L4-RV-FUNCLIB-9F35E215](P4.md#l4-rv-funclib-9f35e215) | fixed | crates/shamir-funclib/src/functions/numeric/gen/core.rs:15,63; crates/shamir-funclib/src/registry.rs:352-357; crates/shamir-funclib/src/functions/numeric/gen/tests/gen_tests.rs:75-82 |
| [L4-RV-FUNCLIB-BDCBEE51](P4.md#l4-rv-funclib-bdcbee51) | fixed | crates/shamir-funclib/src/tests/mod.rs:1; crates/shamir-funclib/src/functions/value/encode/mod.rs:5-6; crates/shamir-funclib/src/functions/value/object/mod.rs:5-6; crates/shamir-funclib/src/functions/text/strings/mod.rs:5-6; cr... |
| [L4-RV-FUNCLIB-C1C6EEC7](P4.md#l4-rv-funclib-c1c6eec7) | fixed | crates/shamir-funclib/src/functions/value/encode/core.rs:6-8,142 |
| [L4-RV-FUNCLIB-E6DE0D0F](P4.md#l4-rv-funclib-e6de0d0f) | fixed | crates/shamir-funclib/src/functions/value/cast/core.rs:177-184 |
| [L4-RV-FUNCLIB-F3D98385](P4.md#l4-rv-funclib-f3d98385) | fixed | crates/shamir-funclib/src/lib.rs:10-12,58-71 |

### shamir-wasm-host (6 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-WASM-HOST-741D48B5](P4.md#l4-rv-wasm-host-741d48b5) | fixed | crates/shamir-wasm-host/src/wasm/mod.rs:6; crates/shamir-wasm-host/src/wasm/mod.rs:19 |
| [L4-RV-WASM-HOST-8062B59A](P4.md#l4-rv-wasm-host-8062b59a) | fixed | crates/shamir-wasm-host/src/wasm/bindings/batch.rs:45-46; crates/shamir-wasm-host/src/wasm/bindings/globals.rs:101-102 |
| [L4-RV-WASM-HOST-8FA4CADD](P4.md#l4-rv-wasm-host-8fa4cadd) | fixed | crates/shamir-wasm-host/src/compile.rs:824; crates/shamir-wasm-host/src/compile.rs:850-898 |
| [L4-RV-WASM-HOST-92EA3A88](P4.md#l4-rv-wasm-host-92ea3a88) | fixed | crates/shamir-wasm-host/src/compile.rs:786-798; crates/shamir-wasm-host/src/tests/runtime/compile_tests.rs:3; crates/shamir-proc-tree/src/supervise.rs:1 |
| [L4-RV-WASM-HOST-B3A8300E](P4.md#l4-rv-wasm-host-b3a8300e) | fixed | crates/shamir-wasm-host/src/wasm/bindings/call.rs:30-48 |
| [L4-RV-WASM-HOST-EBD32CA5](P4.md#l4-rv-wasm-host-ebd32ca5) | fixed | crates/shamir-wasm-host/Cargo.toml:8-54 |

### shamir-sdk (4 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-SDK-8190492D-FACET](P4.md#l4-rv-sdk-8190492d-facet) | fixed | crates/shamir-sdk/src/runtime/host_imports.rs:114-120; crates/shamir-sdk/src/runtime/__rt.rs:102-113; crates/shamir-sdk/src/runtime/__rt.rs:211-218 |
| [L4-RV-SDK-8BC846C6](P4.md#l4-rv-sdk-8bc846c6) | fixed | crates/shamir-sdk/src/api/value.rs:9; crates/shamir-sdk/src/api/db.rs:111 |
| [L4-RV-SDK-9D3236E4](P4.md#l4-rv-sdk-9d3236e4) | fixed | crates/shamir-sdk/src/tests/value/value_tests.rs:1; crates/shamir-sdk/src/tests/validation_tests.rs:9 |
| [L4-RV-SDK-DDF49297](P4.md#l4-rv-sdk-ddf49297) | fixed | crates/shamir-sdk/src/api/value.rs:9; crates/shamir-sdk/src/tests/value/mod.rs:4 |

### shamir-numa (2 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-NUMA-81E50CBF](P4.md#l4-rv-numa-81e50cbf) | fixed | crates/shamir-numa/README.md:53-54; crates/shamir-numa/README.md:58-66 |
| [L4-RV-NUMA-E3044ABE](P4.md#l4-rv-numa-e3044abe) | fixed | crates/shamir-numa/src/platform/detect.rs:22-26; crates/shamir-numa/src/platform/linux.rs:64-76 |

### shamir-bench-utils (2 closed)

| Card | Now | Evidence |
|---|---|---|
| [L4-RV-BENCH-UTILS-88D1646B](P4.md#l4-rv-bench-utils-88d1646b) | fixed | crates/shamir-bench-utils/Cargo.toml:6; crates/shamir-bench-utils/src/lib.rs:9-16 |
| [L4-RV-BENCH-UTILS-CF87C480](P4.md#l4-rv-bench-utils-cf87c480) | fixed | crates/shamir-bench-utils/src/vector_data.rs:260-261; crates/shamir-bench-utils/src/vector_data/tests/mod.rs:1-3; crates/shamir-bench-utils/src/lib.rs:16 |

## Contested cards (pass 2 disagreed with pass 1)

| Card | Pass 1 -> pass 2 (used here) | What pass 1 missed |
|---|---|---|
| [L4-RV-BENCH-UTILS-4E7A9919](P4.md#l4-rv-bench-utils-4e7a9919) | FIXED -> PARTIAL | First pass missed the workload comment still calling an unversioned five-field tuple the reproduction key while current contracts require six fields. |
| [L4-RV-CLIENT-1C219006](P4.md#l4-rv-client-1c219006) | FIXED -> PARTIAL | First pass missed public execute_with_receipts, which still creates interner cache entries without a legacy-peer version gate. |
| [L4-RV-INDEX-20415F00](P4.md#l4-rv-index-20415f00) | FIXED -> PARTIAL | First pass missed the remaining explicit two-variant enum claim in the lifecycle introduction despite its corrected later section. |

## Static acceptance and limits

- Every one of the 169 cards appears exactly once in each pass; the results were parsed from the reviewers' strict jsonl blocks and checked for exact coverage (no missing, extra or duplicate ids).
- Orchestrator spot checks in the live code: 6 of the 10 contested cards of P3/P4 (L3-RV-TRANSPORT-TCP-884DDBF5, L3-RV-WAL-48749F97, L4-RV-INDEX-20415F00, L4-RV-BENCH-UTILS-4E7A9919, L3-RV-CONNECT-2A423D73, L3-RV-QUERY-BUILDER-MACROS-13A0D25A) and 6 randomly chosen cards on which both passes agreed on `FIXED` (seed 20261030: L3-RV-QUERY-BUILDER-MACROS-A63844FD-FACET, L3-RV-TRANSPORT-TCP-D1882068, L4-RV-WAL-0CAFAF79, L4-RV-FUNCLIB-E6DE0D0F, L4-RV-NUMA-81E50CBF, L3-RV-DB-FA80A7B8). All 12 were confirmed.
- No build, test or benchmark was run by this stage; file:line evidence refers to master `96d694f5`.
