# P3 - revalidation of the active findings against current master

Navigation: [P3](P3.md) - [P4](P4.md) - [P2 revalidation](P2-REVALIDATION-2026-10-29.md) - [P2 sprint report](P2-SPRINT-REPORT-2026-10-30.md)

Revalidated on 2026-10-30 against upstream master `96d694f5` (after the P0/P1 remediation waves, the P2 sprint landed as PR #406 / `aaa22258`, and the CI trigger PR #407). The previous revalidation of [P3](P3.md) was made against `b0e1ddf7` on 2026-10-24. This document records the current state only; no code was changed by this stage.

## Method

- Scope: the 340 active cards of [P3](P3.md) (status `confirmed-open`, `partially-fixed` or `unverified` in its table "Активные / partial / unverified facets"). Cards already `fixed` / `refuted` / `not-applicable` and the higher-risk-only redirects are out of scope. Most P3/P4 cards are facets of a P0/P1/P2 mechanism; the facet itself was decided, not the parent.
- Pass 1: 19 read-only rush reviewer sessions (B01-B19, grouped by crate, 340 P3 + 169 P4 cards across both documents). Each card was re-read in [P3](P3.md), located in the live code by symbol (old line numbers are stale after the layout campaign and the P2 sprint), checked against `git log b0e1ddf7..HEAD` and given one verdict.
- Pass 2 (skeptical, independent): 21 sessions (S01-S21) re-decided EVERY one of the 509 cards of P3 and P4. Because pass 1 closed 81% of the cards as `FIXED`, the usual selection (all `FIXED`/`CHANGED`/`PARTIAL`/`UNVERIFIABLE`, high/medium `STILL_OPEN`, a sample of low/nit `STILL_OPEN`) degenerated into all cards. The reviewer decided its own verdict from the live code BEFORE weighing the first-pass claim. Where the two passes disagree, the pass-2 verdict is used here and the card is listed under "Contested cards".
- Everything here is static analysis. A verdict is evidence from source and history, not a test result; CI remains the authority.

## Result

| Status | Cards |
|---|---|
| confirmed-open | 12 |
| partially-fixed | 48 |
| changed | 2 |
| unverified | 1 |
| fixed | 274 |
| refuted | 3 |
| **Total** | 340 |

Still active (`confirmed-open` + `partially-fixed` + `changed` + `unverified`): **63**. Closed since 2026-10-24 (`fixed` + `refuted`): **277**.

Transitions against the status recorded on 2026-10-24:

| Was | Now | Cards |
|---|---|---|
| confirmed-open | fixed | 261 |
| confirmed-open | partially-fixed | 43 |
| confirmed-open | confirmed-open | 12 |
| partially-fixed | fixed | 11 |
| partially-fixed | partially-fixed | 5 |
| unverified | fixed | 2 |
| unverified | refuted | 2 |
| confirmed-open | changed | 1 |
| confirmed-open | refuted | 1 |
| unverified | changed | 1 |
| unverified | unverified | 1 |

Pass 2 disagreed with pass 1 on **7** of the 340 cards of this document (71% of them were first-pass `FIXED`). Disagreement directions: FIXED -> PARTIAL: 5, PARTIAL -> FIXED: 2.

Risk grade of the active cards (after re-grading):

| Risk | Active cards |
|---|---|
| low | 59 |
| nit | 4 |
| **Total** | 63 |

Re-graded among the active cards: 11 (low->nit 3, medium->low 5, nit->low 3).

Fix size and owner-decision flag of the active cards (pass-2 value):

| Size | Cards |
|---|---|
| S | 9 |
| M | 35 |
| L | 19 |
| **Total** | 63 |

Owner-decision flag set on **33** active cards.

## Active cards by crate

Columns: status change, risk (old -> new), fix size, owner decision, and what remains (pass-2 note; the first-pass recipe where one exists).

### shamir-connect (4 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-CONNECT-2A423D73](P3.md#l3-rv-connect-2a423d73) | confirmed-open -> partially-fixed | low | M | yes | First pass missed the card's production decoder-diagnostic facet: e.to_string remains wire-visible inside an Ok-serialized DbResponse::Error. **(contested)** |
| [L3-RV-CONNECT-387EAA05](P3.md#l3-rv-connect-387eaa05) | confirmed-open -> partially-fixed | low | L | yes | Finish fallible entropy and clock propagation through remaining callers and resolve legacy panic-wrapper contracts. / Typed alternatives exist, but public panic wrappers and concrete clock-panic callers remain; public compatibility needs a decision. |
| [L3-RV-CONNECT-F43EB41E](P3.md#l3-rv-connect-f43eb41e) | confirmed-open -> partially-fixed | low | M | no | Add old/new derivation-failure buffer-wipe cases and a CI abrupt-termination durable-counter replay oracle. / Keep partial; refine recipe because new-password derivation failure is already covered, leaving old derivation and CI abrupt-termination replay coverage. |
| [L3-RV-CONNECT-F8A5E821](P3.md#l3-rv-connect-f8a5e821) | confirmed-open -> changed | low | M | no | Read ticket-acceptance versions from the authoritative ArcSwap snapshot and remove the separately published mirror. / RCU removes lost snapshot updates; separate mirror publication still permits an inferred ticket-version coherence window. |

### shamir-server (1 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-SERVER-E8E1D11B](P3.md#l3-rv-server-e8e1d11b) | confirmed-open -> partially-fixed | low | L | yes | Define verified enrollment and rotation, then persist approved upstream pins or explicitly accept restart re-enrollment. / Configured pins and process-local continuity are enforced before replication; learned pins restart empty, requiring enrollment and rotation policy. |

### shamir-client (3 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-CLIENT-35A189A9](P3.md#l3-rv-client-35a189a9) | confirmed-open -> partially-fixed | low | M | yes | Preserve envelope encoding errors in a source-bearing variant and test the source chain. / Envelope encoding loses its typed source in the shared helper before client stringification; repair both layers and test the source chain. |
| [L3-RV-CLIENT-8997B366](P3.md#l3-rv-client-8997b366) | confirmed-open -> partially-fixed | low | M | yes | Retire or migrate the legacy raw-ticket resume entry point under an explicit compatibility decision. / Secure options and wire/framing buffers are protected; retained legacy options and never-polled legacy futures still own raw ticket vectors. |
| [L3-RV-CLIENT-B80C0D09](P3.md#l3-rv-client-b80c0d09) | confirmed-open -> partially-fixed | low | M | yes | Authorize a specific subscription-lock policy exception or design a replacement preserving atomic routing and lifecycle transitions. / Pending registry migrated to scc, but push routing still uses an unsanctioned standard mutex; redesign is M or an authorized policy exception is S. |

### shamir-transport-tcp (6 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-TRANSPORT-TCP-17E11DF1](P3.md#l3-rv-transport-tcp-17e11df1) | confirmed-open -> partially-fixed | low | S | no | Qualify the remaining TCP-spec RFC reference as project-specific without changing wire bytes. / Qualify the remaining TCP-spec RFC reference as project-specific without changing exporter bytes. |
| [L3-RV-TRANSPORT-TCP-31C5E8A6](P3.md#l3-rv-transport-tcp-31c5e8a6) | unverified -> changed | low | M | no | Implement the documented CI plain-TCP syscall and latency measurement before optimizing bounded reads. / Old read_exact pair is gone; split bounded reads remain, and the CI measurement harness/results are not implemented. |
| [L3-RV-TRANSPORT-TCP-79910BA5](P3.md#l3-rv-transport-tcp-79910ba5) | confirmed-open -> confirmed-open | low | M | no | Serialize into reusable zeroizing preframed storage while preserving write serialization and terminal cancellation. / Reachable production roundtrip allocates and copies an already serialized envelope into a second frame buffer. |
| [L3-RV-TRANSPORT-TCP-884DDBF5](P3.md#l3-rv-transport-tcp-884ddbf5) | confirmed-open -> partially-fixed | low | M | no | First pass missed oversized mismatched prefixes: the cap check returns TooLarge before body-length validation, contrary to the documented malformed-buffer contract. **(contested)** |
| [L3-RV-TRANSPORT-TCP-A38C5274](P3.md#l3-rv-transport-tcp-a38c5274) | confirmed-open -> partially-fixed | low | M | no | Add a real uncompleted rustls-connection exporter case asserting its specific error cause. / Typed PEM/key/mismatch tests exist; incomplete-handshake exporter coverage still needs a real connection seam rather than injected General errors. |
| [L3-RV-TRANSPORT-TCP-93246CAF](P3.md#l3-rv-transport-tcp-93246caf) | confirmed-open -> partially-fixed | low -> nit | S | no | Rename the old loopback test accurately while retaining the new unspecified-bind regression. / Actual unspecified-bind coverage exists, but the older loopback test still has a misleading unspecified-address name. |

### shamir-transport-ipc (1 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-TRANSPORT-IPC-FA481516](P3.md#l3-rv-transport-ipc-fa481516) | confirmed-open -> confirmed-open | low | M | yes | Retain accepted conditional exposure or require a server-owned 0700 socket parent before bind. / Bind publishes before chmod and accepts traversable trusted parents; retain conditional acceptance or require a private parent with coverage. |

### shamir-storage (4 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-STORAGE-9478B37E](P3.md#l3-rv-storage-9478b37e) | confirmed-open -> partially-fixed | low | L | yes | Introduce rename-owned staging or recovery that prevents partial-copy destination contamination. / Generic partial merge semantics are documented, but rename copies into final destinations without contamination recovery; use rename-owned staging or recovery. |
| [L3-RV-STORAGE-CCF2090E](P3.md#l3-rv-storage-ccf2090e) | partially-fixed -> partially-fixed | low | M | yes | Pin buffer-config v1 bytes and require explicit rejection or migration policy for schema changes. / Header-first rejection exists; current-type-generated tests do not pin historical buffer-schema bytes or future compatibility policy. |
| [L3-RV-STORAGE-FB7C0794-SEMANTIC-FACET](P3.md#l3-rv-storage-fb7c0794-semantic-facet) | confirmed-open -> confirmed-open | nit -> low | L | yes | Identify decoder ingress and approve compatible size budgets before implementing bounded decoding. / Public deserialization still materializes an uncapped ByteBuf; no production remote ingress established, so investigate compatible decoder budgets first. |
| [L3-RV-STORAGE-0A9E3A41-SEMANTIC-FACET](P3.md#l3-rv-storage-0a9e3a41-semantic-facet) | confirmed-open -> partially-fixed | nit | L | yes | Approve source-bearing codec/backend representations or retain explicitly deferred diagnostic debt. / Worker startup preserves I/O kind/source without panic; general codec/backend source flattening remains. |

### shamir-wal (5 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-WAL-4470BEF3](P3.md#l3-rv-wal-4470bef3) | confirmed-open -> partially-fixed | low | M | no | Reject malformed encoded bodies before mutation and add a supported-header invalid-body append/replay regression. / Supported-header invalid bodies are pinned then persisted and fail replay; reject decode failure before mutation and add a no-write regression. |
| [L3-RV-WAL-48749F97](P3.md#l3-rv-wal-48749f97) | confirmed-open -> partially-fixed | low | M | yes | First pass missed public WalSegment::open resetting the retained maximum; SegmentSet restoration closes only the managed reopen path. **(contested)** |
| [L3-RV-WAL-A25EDE9D](P3.md#l3-rv-wal-a25ede9d) | confirmed-open -> confirmed-open | low | L | yes | Introduce bounded replay consumption and migrate recovery and synchronization consumers while preserving ordering. / Decoded backlog accumulation remains reachable through recovery and synchronization; bounded consumption must preserve ordering. |
| [L3-RV-WAL-C55C2D47-FACET](P3.md#l3-rv-wal-c55c2d47-facet) | confirmed-open -> partially-fixed | medium -> low | M | yes | Preserve a source-bearing cause in legacy completion errors without changing physical-outcome classification. / Tickets retain sink causes; legacy completion conversion still discards them into fixed Storage text. |
| [L3-RV-WAL-79167DD7](P3.md#l3-rv-wal-79167dd7) | confirmed-open -> partially-fixed | low -> nit | M | no | Remove obsolete live-KV WalEntry and WalManager narratives from both module headers. / Retired exports are absent, but two module headers still describe obsolete live APIs. |

### shamir-tx (6 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-TX-1480F306](P3.md#l3-rv-tx-1480f306) | confirmed-open -> partially-fixed | low | M | no | Generate arbitrary key bytes and correct the false separator-exclusion explanation. / Production grammar is qualified, but FF-excluding property generators and misleading explanatory prose remain. |
| [L3-RV-TX-3FC2FA1D-SEMANTIC-FACET](P3.md#l3-rv-tx-3fc2fa1d-semantic-facet) | confirmed-open -> partially-fixed | nit -> low | M | yes | Separate absent timestamps from storage errors, propagating reporting failures while retaining conservative GC behavior. / Warnings restore diagnostics but reporting still returns absent timestamps after storage failures; preserve conservative GC separately. |
| [L3-RV-TX-4A1FDAAE](P3.md#l3-rv-tx-4a1fdaae) | confirmed-open -> confirmed-open | low | L | yes | Approve shared owned table-name storage and preserve event wire bytes with compatibility fixtures. / Public event projection still clones a String per record; shared ownership requires public-type migration and compatibility fixtures. |
| [L3-RV-TX-58E47F9E](P3.md#l3-rv-tx-58e47f9e) | confirmed-open -> confirmed-open | low | S | no | Delegate the legacy publisher to monotonic publication or restrict its fixture-only surface. / Public plain-store publication can regress the floor; delegate to raise_published rather than changing public visibility. |
| [L3-RV-TX-7BB0D16C](P3.md#l3-rv-tx-7bb0d16c) | confirmed-open -> confirmed-open | low | L | no | Design and prove an epoch-qualified opener-to-GC handoff with deterministic progress and safety oracles. / Overlapping public snapshot registrations can continuously force GC floor zero; internal epoch handoff needs safety and progress proofs. |
| [L3-RV-TX-FF6AB063](P3.md#l3-rv-tx-ff6ab063) | confirmed-open -> partially-fixed | low | L | yes | Migrate remaining named String-returning public boundaries to scoped typed errors and update callers. / Journal errors are typed, but named public retention and staging/remap boundaries still require coordinated public error-contract migration. |

### shamir-engine (2 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-ENGINE-2A18B647](P3.md#l3-rv-engine-2a18b647) | partially-fixed -> partially-fixed | low | L | yes | Approve the infallible adapter contract or migrate direct compilation consumers to fallible APIs with invalid-pattern oracles. / Caps and checked Boolean recursion are repaired; public infallible adapters still erase invalid-pattern errors into False. |
| [L3-RV-ENGINE-62C6F2B1](P3.md#l3-rv-engine-62c6f2b1) | confirmed-open -> partially-fixed | low | M | no | Use prepared plan-local condition slots instead of formatting the AST on cache hits. / Borrowed field-path lookup is fixed; reachable condition-cache hits still format the AST, requiring lifetime-safe plan-local slots. |

### shamir-db (3 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-DB-23A03103](P3.md#l3-rv-db-23a03103) | confirmed-open -> partially-fixed | low | M | no | Provide a maintained composite identity index or keyed repository lookup while preserving corruption semantics. / Filtered lookup removes full-result materialization but still uses an unindexed catalogue; keyed remediation must preserve corruption errors. |
| [L3-RV-DB-A7A29B02-FACET](P3.md#l3-rv-db-a7a29b02-facet) | partially-fixed -> partially-fixed | medium -> low | S | no | Warn with repository identity, backend and failure reason when factory reconstruction returns None. / Attach and missing-parent failures propagate, but None factory reconstruction silently omits persisted repositories; add contextual diagnostics. |
| [L3-RV-DB-C2DCAC96](P3.md#l3-rv-db-c2dcac96) | confirmed-open -> confirmed-open | low | M | yes | Approve the production builder dependency and migrate facade/SystemStore construction while preserving defaults and batch semantics. / Production gateway directly constructs and executes DTOs; approve builder dependency and preserve existing semantics. |

### shamir-index (4 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-INDEX-0695AADA](P3.md#l3-rv-index-0695aada) | confirmed-open -> partially-fixed | low | L | yes | Stream legacy cleanup without truncation and define a bounded/fallible public tokenizer contract. / New values are budgeted, but public infallible tokenization and old-value cleanup remain unbounded; preserve complete cleanup. |
| [L3-RV-INDEX-605EEC5D](P3.md#l3-rv-index-605eec5d) | confirmed-open -> partially-fixed | low | L | yes | Define portable persisted hash identity and explicit migration/rebuild rules in index identity v2. / 64-bit golden outputs help, but caret FxHasher and absent persisted algorithm/target identity leave compatibility policy unresolved. |
| [L3-RV-INDEX-CF86432E](P3.md#l3-rv-index-cf86432e) | confirmed-open -> partially-fixed | low | L | yes | Introduce runtime secret references and explicitly migrate or remove grandfathered inline credentials. / New credentials are rejected, but matching legacy cleartext is rewritten; secret provisioning and migration need an owner. |
| [L3-RV-INDEX-FBB72544](P3.md#l3-rv-index-fbb72544) | confirmed-open -> confirmed-open | low | L | yes | Canonicalize recursive Map identity with a versioned persisted-index migration and reversed-insertion lookup parity tests. / Reachable functional map keys still hash insertion order; recursive canonical identity needs persisted-version migration. |

### shamir-types (4 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-TYPES-5E4BFC76](P3.md#l3-rv-types-5e4bfc76) | confirmed-open -> partially-fixed | low | M | yes | Ratify trusted-root constraints and require bounded type-specific decoding for any recursive untrusted input. / The generic public helper still lacks depth enforcement; inspected storage roots are nonrecursive. |
| [L3-RV-TYPES-65DDE5B8](P3.md#l3-rv-types-65dde5b8) | partially-fixed -> partially-fixed | low | S | no | Add direct username-bridge determinism and i64-range assertions without claiming collision resistance. / Projection coverage is not direct username-bridge determinism/range coverage; other named surfaces have assertions. |
| [L3-RV-TYPES-A7B1FCEC](P3.md#l3-rv-types-a7b1fcec) | confirmed-open -> partially-fixed | low | M | no | Reuse a borrowed canonical folder key across metadata traversal and add a deep-chain CI allocation oracle. / Borrowed ancestor iteration removes Vec/clones, but slice-backed folder metadata lookups still join each prefix. |
| [L3-RV-TYPES-FE40CB54](P3.md#l3-rv-types-fe40cb54) | confirmed-open -> partially-fixed | low | M | yes | Owner-select the comparison contract and add a regression that exercises Drop dispatch rather than only the wipe helper. / Safe wiping landed; ordinary equality remains warned against, and helper-only coverage does not detect disconnected Drop dispatch. |

### shamir-query-types (3 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-QUERY-TYPES-0A0D9092](P3.md#l3-rv-query-types-0a0d9092) | partially-fixed -> partially-fixed | low | L | no | Cache metadata per static body and reuse it throughout nested receipt validation and execution. / Root metadata is fused, but nested receipt-policy preflight still recollects descendant repositories on reachable admin-write paths. |
| [L3-RV-QUERY-TYPES-182C703E](P3.md#l3-rv-query-types-182c703e) | confirmed-open -> confirmed-open | low | L | yes | Adopt thiserror and typed validation errors under an approved public-API compatibility migration. / Manual error implementations and callable public String-error validators remain; typed-error migration changes public compatibility. |
| [L3-RV-QUERY-TYPES-4F767AA5](P3.md#l3-rv-query-types-4f767aa5) | confirmed-open -> partially-fixed | low | L | yes | Migrate callers to fallible conversion and retire the lossy From adapter through an approved API change. / Fallible conversion exists, but Doc::set still reaches the legacy From adapter that release-substitutes Null for unsupported maps. |

### shamir-query-builder (3 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-QUERY-BUILDER-2A648617](P3.md#l3-rv-query-builder-2a648617) | confirmed-open -> partially-fixed | low | L | yes | Accept documented quadratic lowering or authorize a semantics-preserving shared-condition representation. / Quadratic lowering remains reachable through switch, but is documented; accept cost or authorize a semantics-preserving shared representation. |
| [L3-RV-QUERY-BUILDER-9E172C69](P3.md#l3-rv-query-builder-9e172c69) | confirmed-open -> partially-fixed | low | M | no | Migrate ownership-available send sites to consuming terminals while retaining deliberate borrowed reuse. / Consuming request and SDK terminals exist; reachable borrowed encoding still clones request payloads. |
| [L3-RV-QUERY-BUILDER-7A0F3500](P3.md#l3-rv-query-builder-7a0f3500) | confirmed-open -> partially-fixed | low -> nit | M | no | Finish independent DDL owner-file separation and resolve temporary config placement while preserving exports. / Schema/access owners are separated, but independent list builders remain bundled; finish structural separation while preserving exports. |

### shamir-query-builder-macros (4 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-QUERY-BUILDER-MACROS-002C0148](P3.md#l3-rv-query-builder-macros-002c0148) | confirmed-open -> partially-fixed | low | M | no | Normalize raw select-function identifiers before whitelist classification and pin raw/non-raw equivalence. / Select-function classification still uses escaped identifier spelling, rejecting raw count despite other normalization fixes. |
| [L3-RV-QUERY-BUILDER-MACROS-2C459E8E](P3.md#l3-rv-query-builder-macros-2c459e8e) | confirmed-open -> partially-fixed | low | M | yes | Approve a public call-construction builder entry point and route macro expansion through it. / Repository override is repaired, but expansion still constructs CallOp directly rather than using a call builder. |
| [L3-RV-QUERY-BUILDER-MACROS-30DA1AC8](P3.md#l3-rv-query-builder-macros-30da1ac8) | confirmed-open -> partially-fixed | low | S | no | Qualify the full-grammar claim with the accepted clause-keyword start-position exception. / Initial keyword reservation remains accepted and documented; the proposed documentation qualification is already substantially present. |
| [L3-RV-QUERY-BUILDER-MACROS-9BCA14B1](P3.md#l3-rv-query-builder-macros-9bca14b1) | unverified -> unverified | low | M | yes | Establish isolated pathological-input CI evidence across parsing, lowering, diagnostics and destruction before choosing limits. / Uncapped recursion is present, but accepted pathological depth and compiler abort remain unobserved; require isolated CI evidence before limits. |

### shamir-funclib (3 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-FUNCLIB-24C30E50](P3.md#l3-rv-funclib-24c30e50) | confirmed-open -> partially-fixed | low | M | no | Catalogue existing scalar_task_failed and not_indexable outputs, use builtin construction and extend catalogue assertions. / Catalogue enforcement exists, but reachable resolver-generated scalar_task_failed and not_indexable bypass it; preserve spellings and extend catalogue coverage. |
| [L3-RV-FUNCLIB-775ADABC](P3.md#l3-rv-funclib-775adabc) | confirmed-open -> partially-fixed | low | S | no | Qualify the remaining determinism and reserved-constant promises as string-key-only exclusion. / String-key-only exclusion is correctly specified in key places, but determinism and reserved-constant documentation retain unconditional exclusion promises. |
| [L3-RV-FUNCLIB-C8BC39BC](P3.md#l3-rv-funclib-c8bc39bc) | confirmed-open -> partially-fixed | low | S | no | Replace ScalarResolver's blanket lock-free-map claim with the actual shared bucket read-lock contract. / Exclusive lookup locking is removed, but ScalarResolver still labels the shared-lock map lock-free; correct that remaining description. |

### shamir-wasm-host (2 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-WASM-HOST-4669FDA6](P3.md#l3-rv-wasm-host-4669fda6) | confirmed-open -> partially-fixed | low | M | yes | Deprecate the lossy public decoder, migrate consumers to try_from_record, then remove it through an approved API transition. / Production decoding is checked and versioned, but the public lossy decoder remains; staged API deprecation is appropriate. |
| [L3-RV-WASM-HOST-91385BE0](P3.md#l3-rv-wasm-host-91385be0) | confirmed-open -> partially-fixed | low | M | no | First pass missed the explicitly scoped Params rejection obligation: added tests cover other categories but not malformed/non-map nested-call Params. **(contested)** |

### shamir-sdk (3 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-SDK-6D2497CA-FACET](P3.md#l3-rv-sdk-6d2497ca-facet) | confirmed-open -> partially-fixed | medium -> low | M | no | Classify unexpected DB response shapes as Codec and preserve structured codec sources consistently. / Categories exist, but unexpected DB shapes remain User errors and codec causes are flattened into strings. |
| [L3-RV-SDK-723F5FD2-SEMANTIC-FACET](P3.md#l3-rv-sdk-723f5fd2-semantic-facet) | confirmed-open -> confirmed-open | nit -> low | L | yes | Accept the documented lower-2-GiB limitation explicitly or authorize a coordinated unsigned-address ABI migration with boundary fixtures. / Signed exchange fields still exclude high-bit addresses; owner acceptance or coordinated ABI migration remains required. |
| [L3-RV-SDK-B5ACD67D-FACET](P3.md#l3-rv-sdk-b5acd67d-facet) | confirmed-open -> partially-fixed | medium -> low | M | yes | Require exact successful HTTP body decoding and add duplicate top-level response-field policy fixtures. / Require exact successful body decoding and add duplicate top-level response-field fixtures; choosing duplicate semantics needs an owner decision. |

### shamir-sdk-macros (1 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-SDK-MACROS-853FC350-FACET](P3.md#l3-rv-sdk-macros-853fc350-facet) | confirmed-open -> confirmed-open | medium -> low | M | no | Use concrete wrapper typing for return compatibility and add alias/foreign-type consumer fixtures. / Lexical return-name gating still rejects compatible differently named aliases; concrete SDK typing already prevents foreign-type confusion. |

### shamir-numa (1 active)

| Card | Status | Risk | Size | Owner | What remains / recipe |
|---|---|---|---|---|---|
| [L3-RV-NUMA-553CB81D](P3.md#l3-rv-numa-553cb81d) | confirmed-open -> partially-fixed | low | S | no | First pass confused affinity OOB coverage with explicit replica load_node/store_node OOB coverage; the existing load_local test is intercepted by mock clamping. **(contested)** |

## Closed since 2026-10-24

`fixed` = the mechanism is gone and the evidence points at the live code (or the commit); `refuted` = the premise is false at HEAD with positive counter-evidence. `aaa22258` is the P2 sprint squash commit.

### shamir-connect (16 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-CONNECT-226DEDF4](P3.md#l3-rv-connect-226dedf4) | fixed | crates/shamir-connect/src/client/handshake.rs:210; crates/shamir-connect/src/client/changepw.rs:45; crates/shamir-connect/src/client/bootstrap.rs:99; crates/shamir-connect/src/common/primitives/password.rs:41 |
| [L3-RV-CONNECT-2B5AF3F2](P3.md#l3-rv-connect-2b5af3f2) | fixed | crates/shamir-connect/src/server/state/session.rs:276; crates/shamir-connect/src/server/state/session.rs:480; crates/shamir-connect/src/server/auth/changepw.rs:127 |
| [L3-RV-CONNECT-2FADEBC4](P3.md#l3-rv-connect-2fadebc4) | fixed | crates/shamir-connect/src/server/infra/durable_counters.rs:127; crates/shamir-connect/src/server/infra/durable_counters.rs:163; crates/shamir-connect/src/server/infra/durable_counters.rs:198; crates/shamir-connect/src/server/in... |
| [L3-RV-CONNECT-3380EB1C](P3.md#l3-rv-connect-3380eb1c) | fixed | crates/shamir-connect/src/server/infra/config.rs:29; crates/shamir-connect/src/server/state/resume.rs:157; crates/shamir-connect/src/server/state/resume.rs:180 |
| [L3-RV-CONNECT-60D4DAB1](P3.md#l3-rv-connect-60d4dab1) | fixed | crates/shamir-connect/src/common/messages/push_envelope.rs:34; crates/shamir-server/src/engine/subscriptions/flow/push.rs:30 |
| [L3-RV-CONNECT-74705EAB](P3.md#l3-rv-connect-74705eab) | refuted | crates/shamir-connect/src/common/primitives/domain_tags.rs:17; crates/shamir-connect/src/common/messages/auth_message.rs:84; crates/shamir-connect/src/common/tests/message_tests/auth_message_tests.rs:127; crates/shamir-connect/... |
| [L3-RV-CONNECT-781E7475](P3.md#l3-rv-connect-781e7475) | fixed | crates/shamir-connect/src/common/tests/crypto_tests/test_vectors_tests.rs:58; crates/shamir-connect/src/common/tests/crypto_tests/test_vectors_tests.rs:95; crates/shamir-connect/src/common/tests/crypto_tests/mod.rs:5; crates/sh... |
| [L3-RV-CONNECT-A2618127](P3.md#l3-rv-connect-a2618127) | fixed | crates/shamir-connect/src/server/infra/audit_chain.rs:69; crates/shamir-connect/src/server/infra/audit_chain.rs:236; crates/shamir-connect/src/server/infra/audit_chain.rs:440 |
| [L3-RV-CONNECT-B11F48CA](P3.md#l3-rv-connect-b11f48ca) | fixed | crates/shamir-connect/src/server/state/resume.rs:157; crates/shamir-connect/src/server/state/resume.rs:179; crates/shamir-connect/src/server/state/resume.rs:191; crates/shamir-connect/src/server/state/resume.rs:202 |
| [L3-RV-CONNECT-C48FDE17](P3.md#l3-rv-connect-c48fde17) | fixed | crates/shamir-connect/src/server/infra/audit_chain.rs:90; crates/shamir-connect/src/server/infra/audit_chain.rs:117; crates/shamir-connect/src/server/infra/audit_chain.rs:140; crates/shamir-connect/src/server/infra/audit_chain.... |
| [L3-RV-CONNECT-C9066D95](P3.md#l3-rv-connect-c9066d95) | fixed | crates/shamir-connect/src/server/auth/changepw.rs:133; crates/shamir-connect/src/server/tests/session_flow/changepw_challenge_tests.rs:402 |
| [L3-RV-CONNECT-CA7F732D](P3.md#l3-rv-connect-ca7f732d) | fixed | crates/shamir-connect/src/server/tests/security/post_auth_rate_limit_tests.rs:158; crates/shamir-connect/src/server/tests/security/post_auth_rate_limit_tests.rs:183 |
| [L3-RV-CONNECT-CD221FBC](P3.md#l3-rv-connect-cd221fbc) | fixed | crates/shamir-connect/src/common/tests/message_tests/auth_message_tests.rs:151; crates/shamir-connect/src/common/tests/message_tests/auth_message_tests.rs:178 |
| [L3-RV-CONNECT-D068976D](P3.md#l3-rv-connect-d068976d) | fixed | crates/shamir-connect/src/server/infra/audit_chain.rs:145; crates/shamir-connect/src/server/infra/audit_chain.rs:239; crates/shamir-connect/src/server/infra/audit_chain.rs:259; crates/shamir-connect/src/server/infra/audit_chain... |
| [L3-RV-CONNECT-E0A24E86](P3.md#l3-rv-connect-e0a24e86) | fixed | crates/shamir-connect/src/server/state/ticket.rs:33; crates/shamir-connect/src/server/state/ticket.rs:167; crates/shamir-connect/src/server/state/ticket.rs:177; crates/shamir-connect/src/server/state/ticket.rs:235; crates/shami... |
| [L3-RV-CONNECT-F34827AC](P3.md#l3-rv-connect-f34827ac) | fixed | crates/shamir-connect/src/server/state/ticket.rs:117; crates/shamir-connect/src/server/state/ticket.rs:191 |

### shamir-server (5 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-SERVER-024B9BC4](P3.md#l3-rv-server-024b9bc4) | fixed | crates/shamir-server/tests/misc/runtime/scheduler.rs:50-54; crates/shamir-server/tests/misc/runtime/scheduler.rs:266-286; crates/shamir-server/tests/misc/main.rs:20 |
| [L3-RV-SERVER-4046C528](P3.md#l3-rv-server-4046c528) | fixed | crates/shamir-server/tests/misc/audit_log/observability_http.rs:509-542; crates/shamir-server/tests/misc/main.rs:16 |
| [L3-RV-SERVER-5ED48F45](P3.md#l3-rv-server-5ed48f45) | fixed | crates/shamir-server/src/security/observability/mod.rs:26-29; crates/shamir-server/src/security/observability/mod.rs:633-637 |
| [L3-RV-SERVER-9B8FC4F8](P3.md#l3-rv-server-9b8fc4f8) | fixed | crates/shamir-server/src/sync/connection/handshake.rs:37-39; crates/shamir-server/src/sync/connection/handshake.rs:695-696; crates/shamir-server/src/sync/connection/session/resume.rs:110-111 |
| [L3-RV-SERVER-F60C70B4](P3.md#l3-rv-server-f60c70b4) | fixed | crates/shamir-server/src/security/observability/logging/mod.rs:162-183 |

### shamir-client (20 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-CLIENT-17C0D288-SEMANTIC-FACET](P3.md#l3-rv-client-17c0d288-semantic-facet) | fixed | crates/shamir-client/src/client/api.rs:482-494,528-533; crates/shamir-client/src/client/connect.rs:264; crates/shamir-client/src/tests/client/lifecycle_tests.rs:127-161 |
| [L3-RV-CLIENT-1CF072D5](P3.md#l3-rv-client-1cf072d5) | fixed | crates/shamir-client/src/interner_cache/ops.rs:672-714 |
| [L3-RV-CLIENT-267BD36E](P3.md#l3-rv-client-267bd36e) | fixed | crates/shamir-client/src/client/api.rs:412-438; crates/shamir-client/src/tests/client/lifecycle_tests.rs:89-122 |
| [L3-RV-CLIENT-2B10814D](P3.md#l3-rv-client-2b10814d) | fixed | crates/shamir-client/src/interner_cache/ops.rs:249-291 |
| [L3-RV-CLIENT-2D0142DD](P3.md#l3-rv-client-2d0142dd) | fixed | crates/shamir-client/src/tests/auth/auth_identity_tests.rs:68-88; crates/shamir-client/src/tests/auth/resume_and_scratch_tests.rs:104-115; crates/shamir-client/src/tests/client/resume_wire_tests.rs:474-588 |
| [L3-RV-CLIENT-2D08F353](P3.md#l3-rv-client-2d08f353) | fixed | crates/shamir-client/src/tests/auth/resume_and_scratch_tests.rs:91-146; crates/shamir-client/src/tests/client/lifecycle_tests.rs:525-583; crates/shamir-client/tests/batch/when.rs:299-300; crates/shamir-client/tests/batch/for_ea... |
| [L3-RV-CLIENT-3D598780](P3.md#l3-rv-client-3d598780) | fixed | crates/shamir-client/src/client/api.rs:260-270,648-650 |
| [L3-RV-CLIENT-462B7413](P3.md#l3-rv-client-462b7413) | fixed | crates/shamir-client/src/interner_cache/ops.rs:424-425,643-657 |
| [L3-RV-CLIENT-47644B74](P3.md#l3-rv-client-47644b74) | fixed | crates/shamir-client/src/tests/interner/interner_cache_tests.rs:704-735,772-788; crates/shamir-client/src/tests/client/lifecycle_tests.rs:256-296 |
| [L3-RV-CLIENT-56E4E99D](P3.md#l3-rv-client-56e4e99d) | fixed | crates/shamir-client/src/client/transport.rs:126-148,351-360,642-645,677-685 |
| [L3-RV-CLIENT-5E5B78A0](P3.md#l3-rv-client-5e5b78a0) | fixed | crates/shamir-client/src/client/cursor_stream.rs:136,227-245,264-265 |
| [L3-RV-CLIENT-67D785CC](P3.md#l3-rv-client-67d785cc) | fixed | crates/shamir-client/src/client/api.rs:220-221; crates/shamir-client/src/client/subscription.rs:103-110 |
| [L3-RV-CLIENT-98656AAA-FACET](P3.md#l3-rv-client-98656aaa-facet) | fixed | crates/shamir-types/src/record_view/lens/view.rs:342-389 |
| [L3-RV-CLIENT-9DEFDE43](P3.md#l3-rv-client-9defde43) | fixed | crates/shamir-client/src/interner_cache/ops.rs:390-399,484-503; crates/shamir-client/src/tests/interner/set_insert_tests.rs:12-34 |
| [L3-RV-CLIENT-AE50D83D](P3.md#l3-rv-client-ae50d83d) | fixed | crates/shamir-client/src/client/transport.rs:137-140,648-650; crates/shamir-client/src/error.rs:52-57 |
| [L3-RV-CLIENT-C93ADFED](P3.md#l3-rv-client-c93adfed) | fixed | crates/shamir-client/src/lib.rs:10-20; crates/shamir-client/examples/connect.rs:4-19; crates/shamir-client/Cargo.toml:49-52 |
| [L3-RV-CLIENT-E16A04E5](P3.md#l3-rv-client-e16a04e5) | fixed | crates/shamir-client/src/client/api.rs:495-513,583; crates/shamir-connect/src/common/messages/envelope.rs:92-109 |
| [L3-RV-CLIENT-E38BE183](P3.md#l3-rv-client-e38be183) | fixed | crates/shamir-client/src/tests/client/lifecycle_tests.rs:237-244,413-428,437-476,566-583; crates/shamir-client/src/tests/client/mod.rs:4 |
| [L3-RV-CLIENT-E5D2A811](P3.md#l3-rv-client-e5d2a811) | fixed | crates/shamir-client/src/tests/wire/v2_passthrough_tests.rs:147-173 |
| [L3-RV-CLIENT-EB08BA4C](P3.md#l3-rv-client-eb08ba4c) | fixed | crates/shamir-client/src/interner_cache/ops.rs:249-252,390-399,460-489 |

### shamir-client-node (5 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-CLIENT-NODE-050EF626](P3.md#l3-rv-client-node-050ef626) | fixed | crates/shamir-client-node/js/wrapper.js:163-167; crates/shamir-client-node/src/lib.rs:322-325 |
| [L3-RV-CLIENT-NODE-4E3D7A44](P3.md#l3-rv-client-node-4e3d7a44) | fixed | crates/shamir-client-node/src/secrets.rs:3-13,24-27; crates/shamir-client-node/src/lib.rs:376-382 |
| [L3-RV-CLIENT-NODE-C35E4678-SEMANTIC-FACET](P3.md#l3-rv-client-node-c35e4678-semantic-facet) | fixed | crates/shamir-client-node/src/lib.rs:72-73,129-142,312-317; crates/shamir-client-node/src/secrets.rs:34-40 |
| [L3-RV-CLIENT-NODE-C5E06B98](P3.md#l3-rv-client-node-c5e06b98) | refuted | crates/shamir-client-node/native/index.js:204-222; crates/shamir-client-node/package.json:7-12; README.md:18 |
| [L3-RV-CLIENT-NODE-F64125FA](P3.md#l3-rv-client-node-f64125fa) | fixed | crates/shamir-client-node/js/wrapper.js:103-122,153-167 |

### shamir-transport-tcp (7 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-TRANSPORT-TCP-0A23323D](P3.md#l3-rv-transport-tcp-0a23323d) | fixed | crates/shamir-transport-tcp/src/tls.rs:101-124 |
| [L3-RV-TRANSPORT-TCP-11F795D1](P3.md#l3-rv-transport-tcp-11f795d1) | fixed | crates/shamir-transport-tcp/tests/tls13_only.rs:130-167; crates/shamir-transport-tcp/src/tls.rs:204-212 |
| [L3-RV-TRANSPORT-TCP-3A576732](P3.md#l3-rv-transport-tcp-3a576732) | fixed | crates/shamir-transport-tcp/src/framing.rs:196,241,301-303,327-334 |
| [L3-RV-TRANSPORT-TCP-A3B66C24](P3.md#l3-rv-transport-tcp-a3b66c24) | fixed | crates/shamir-transport-tcp/src/framing.rs:77-79,115-117,133-134 |
| [L3-RV-TRANSPORT-TCP-AC0BEB92](P3.md#l3-rv-transport-tcp-ac0beb92) | fixed | crates/shamir-transport-tcp/tests/framing.rs:53-122,231-267; crates/shamir-transport-tcp/src/tests/framing_tests.rs:188-194; crates/shamir-transport-tcp/src/tests/listener_tests.rs:28-48 |
| [L3-RV-TRANSPORT-TCP-D1882068](P3.md#l3-rv-transport-tcp-d1882068) | fixed | crates/shamir-transport-tcp/src/tls.rs:133-138; crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs:121-126,354-359; crates/shamir-client/src/client/connect.rs:392-393,644-645 |
| [L3-RV-TRANSPORT-TCP-FF2983EC](P3.md#l3-rv-transport-tcp-ff2983ec) | fixed | crates/shamir-transport-tcp/src/framing.rs:71-79,115-117; crates/shamir-transport-tcp/src/tests/framing_tests.rs:94-106 |

### shamir-transport-ws (5 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-TRANSPORT-WS-67E355C7](P3.md#l3-rv-transport-ws-67e355c7) | fixed | crates/shamir-transport-ws/src/server.rs:196-230; crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs:26-28,441-443 |
| [L3-RV-TRANSPORT-WS-6C58DE5C](P3.md#l3-rv-transport-ws-6c58de5c) | fixed | crates/shamir-transport-ws/src/server.rs:131-184,220-283 |
| [L3-RV-TRANSPORT-WS-A8502651](P3.md#l3-rv-transport-ws-a8502651) | fixed | crates/shamir-transport-ws/src/tls_exporter.rs:21-24; crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs:354-359,467-471 |
| [L3-RV-TRANSPORT-WS-B7D06174](P3.md#l3-rv-transport-ws-b7d06174) | fixed | crates/shamir-server/src/sync/connection/request_loop.rs:248-263; crates/shamir-server/src/sync/framer.rs:471-474,555-558; crates/shamir-transport-ws/src/framing.rs:161-185 |
| [L3-RV-TRANSPORT-WS-F0A54173](P3.md#l3-rv-transport-ws-f0a54173) | fixed | crates/shamir-transport-ws/src/browser.rs:49-64,82-87,117-137; crates/shamir-server/src/engine/server/server_launcher/helpers.rs:58-66 |

### shamir-transport-ipc (5 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-TRANSPORT-IPC-07D10AFD](P3.md#l3-rv-transport-ipc-07d10afd) | fixed | crates/shamir-transport-ipc/src/tests/windows_tests.rs:181-207; crates/shamir-transport-ipc/src/tests/unix_tests.rs:24-48; crates/shamir-transport-ipc/src/tests/unix_tests.rs:221-251 |
| [L3-RV-TRANSPORT-IPC-466D0307](P3.md#l3-rv-transport-ipc-466d0307) | fixed | crates/shamir-transport-ipc/src/windows.rs:45-70; crates/shamir-transport-ipc/src/windows.rs:129-140; crates/shamir-transport-ipc/src/tests/windows_tests.rs:215-262 |
| [L3-RV-TRANSPORT-IPC-4E8BE4FD](P3.md#l3-rv-transport-ipc-4e8be4fd) | fixed | crates/shamir-transport-ipc/src/tests/windows_tests.rs:270-295 |
| [L3-RV-TRANSPORT-IPC-8ED1B450](P3.md#l3-rv-transport-ipc-8ed1b450) | fixed | crates/shamir-transport-ipc/src/tests/windows_tests.rs:52-69; crates/shamir-transport-ipc/src/tests/windows_dacl.rs:18-110; crates/shamir-transport-ipc/src/tests/windows_dacl.rs:114-169 |
| [L3-RV-TRANSPORT-IPC-ACD301ED](P3.md#l3-rv-transport-ipc-acd301ed) | fixed | crates/shamir-transport-ipc/src/unix.rs:37; crates/shamir-transport-ipc/src/unix.rs:47; crates/shamir-transport-ipc/src/unix.rs:131; crates/shamir-transport-ipc/src/windows.rs:56; crates/shamir-transport-ipc/src/windows.rs:87; ... |

### shamir-storage (14 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-STORAGE-1A5A9374](P3.md#l3-rv-storage-1a5a9374) | fixed | crates/shamir-storage/src/stores/storage_fjall.rs:138-142; crates/shamir-storage/src/stores/storage_fjall.rs:514-523; docs/dev-artifacts/prompts/remediation/p2/08-storage.md:34 |
| [L3-RV-STORAGE-2F8C6A8C](P3.md#l3-rv-storage-2f8c6a8c) | fixed | crates/shamir-storage/src/stores/storage_membuffer/core.rs:134; crates/shamir-storage/src/stores/storage_membuffer/core.rs:297-334 |
| [L3-RV-STORAGE-38F208C4](P3.md#l3-rv-storage-38f208c4) | fixed | crates/shamir-storage/src/stores/storage_fjall.rs:210-220; crates/shamir-storage/src/stores/storage_fjall.rs:563-566; docs/dev-artifacts/prompts/remediation/p2/08-storage.md:30 |
| [L3-RV-STORAGE-7B098ADB-SEMANTIC-FACET](P3.md#l3-rv-storage-7b098adb-semantic-facet) | fixed | crates/shamir-storage/src/stores/storage_cached.rs:215-218,275-280,366-448,541-559 |
| [L3-RV-STORAGE-8DFF95BF](P3.md#l3-rv-storage-8dff95bf) | fixed | crates/shamir-storage/src/stores/storage_cached.rs:256-272; crates/shamir-storage/src/stores/storage_cached.rs:321-346; crates/shamir-storage/src/stores/storage_cached.rs:433-446; crates/shamir-storage/src/stores/storage_cached... |
| [L3-RV-STORAGE-8FEF1C5A](P3.md#l3-rv-storage-8fef1c5a) | fixed | crates/shamir-collections/src/lib.rs:20-46 |
| [L3-RV-STORAGE-9794FD53](P3.md#l3-rv-storage-9794fd53) | fixed | crates/shamir-storage/src/stores/storage_mirrored.rs:163-171 |
| [L3-RV-STORAGE-AE7F22EF](P3.md#l3-rv-storage-ae7f22ef) | fixed | crates/shamir-storage/src/stores/storage_cached.rs:366-446; crates/shamir-storage/src/stores/storage_cached.rs:661-662; crates/shamir-storage/src/stores/storage_cached.rs:756-762 |
| [L3-RV-STORAGE-B7839F16](P3.md#l3-rv-storage-b7839f16) | fixed | crates/shamir-storage/src/core/types.rs:427-429; crates/shamir-storage/src/core/types.rs:482-484; crates/shamir-storage/src/key_bytes/mod.rs:3-5; crates/shamir-storage/src/key_bytes/mod.rs:45-52 |
| [L3-RV-STORAGE-BE72410B](P3.md#l3-rv-storage-be72410b) | fixed | crates/shamir-storage/src/core/error.rs:252-256; crates/shamir-storage/src/stores/storage_fjall.rs:655; crates/shamir-storage/src/stores/storage_in_memory.rs:217; crates/shamir-storage/src/stores/storage_cached.rs:791; crates/s... |
| [L3-RV-STORAGE-C985620C](P3.md#l3-rv-storage-c985620c) | fixed | crates/shamir-storage/src/stores/storage_membuffer/core.rs:297-328 |
| [L3-RV-STORAGE-CC5C79BA](P3.md#l3-rv-storage-cc5c79ba) | fixed | crates/shamir-storage/src/stores/storage_fjall.rs:77-85,258-281,342-348,422-449 |
| [L3-RV-STORAGE-DCBCB721](P3.md#l3-rv-storage-dcbcb721) | fixed | crates/shamir-storage/src/key_bytes/mod.rs:3-5,45-52; crates/shamir-storage/src/core/types.rs:11 |
| [L3-RV-STORAGE-FA461E63](P3.md#l3-rv-storage-fa461e63) | fixed | crates/shamir-storage/src/tests/stores/storage_cached_tests/basic_tests.rs:708-725; crates/shamir-storage/src/tests/stores/storage_cached_tests/mod.rs:4 |

### shamir-wal (15 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-WAL-2859EC6A](P3.md#l3-rv-wal-2859ec6a) | fixed | crates/shamir-wal/src/tests/entry/segment_meta_tests.rs:18-70; crates/shamir-wal/src/tests/entry/mod.rs:3 |
| [L3-RV-WAL-4BD5E020](P3.md#l3-rv-wal-4bd5e020) | fixed | crates/shamir-wal/src/segment/frame_reader.rs:72-98; crates/shamir-wal/src/segment/wal_segment.rs:556,766-770; crates/shamir-wal/src/segment_set/sync_snapshot.rs:95 |
| [L3-RV-WAL-5839FF0D](P3.md#l3-rv-wal-5839ff0d) | fixed | crates/shamir-wal/src/segment/wal_segment.rs:50-60,218,406,585,746; crates/shamir-wal/src/entry/wal_entry_v2.rs:228-231,319-349 |
| [L3-RV-WAL-59E3DBA1](P3.md#l3-rv-wal-59e3dba1) | fixed | crates/shamir-wal/src/wal_group_commit/wal_group_commit.rs:235-251,312,315-334 |
| [L3-RV-WAL-645DBB85](P3.md#l3-rv-wal-645dbb85) | fixed | crates/shamir-wal/src/entry/wal_entry_v2.rs:228-231,319-349; crates/shamir-wal/src/segment/wal_segment.rs:798 |
| [L3-RV-WAL-67475C86](P3.md#l3-rv-wal-67475c86) | fixed | crates/shamir-wal/src/segment_set/segment_set.rs:600-665 |
| [L3-RV-WAL-705D095F](P3.md#l3-rv-wal-705d095f) | fixed | crates/shamir-wal/src/segment_set/segment_set.rs:15-18 |
| [L3-RV-WAL-79483DA7](P3.md#l3-rv-wal-79483da7) | fixed | crates/shamir-wal/src/segment/wal_segment.rs:350-406 |
| [L3-RV-WAL-8021EB80](P3.md#l3-rv-wal-8021eb80) | fixed | crates/shamir-wal/src/segment/wal_sink.rs:302-317; crates/shamir-wal/src/segment_set/segment_set.rs:194-252 |
| [L3-RV-WAL-9CB60C5A](P3.md#l3-rv-wal-9cb60c5a) | fixed | crates/shamir-wal/src/entry/wal_entry_v2.rs:182-231 |
| [L3-RV-WAL-B8E9884C](P3.md#l3-rv-wal-b8e9884c) | fixed | crates/shamir-wal/src/segment/mem_frames.rs:124-126; crates/shamir-wal/src/segment/wal_sink.rs:423-426 |
| [L3-RV-WAL-C37B110A](P3.md#l3-rv-wal-c37b110a) | fixed | crates/shamir-wal/src/segment/wal_segment.rs:260-265 |
| [L3-RV-WAL-CC3131A9](P3.md#l3-rv-wal-cc3131a9) | fixed | crates/shamir-wal/src/segment/frame_reader.rs:72-90; crates/shamir-wal/src/entry/segment_meta.rs:132-135 |
| [L3-RV-WAL-DD492F5C](P3.md#l3-rv-wal-dd492f5c) | fixed | crates/shamir-wal/src/lib.rs:25-40; docs/guide-docs/security/data-protection.md:105-128 |
| [L3-RV-WAL-FE36ADFC](P3.md#l3-rv-wal-fe36adfc) | fixed | crates/shamir-wal/src/entry/wal_entry_v2.rs:78-96; crates/shamir-engine/src/tx/recovery/v2.rs:181-203 |

### shamir-tx (8 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-TX-21B69CC7](P3.md#l3-rv-tx-21b69cc7) | fixed | crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs:892-917; crates/shamir-tx/src/mvcc_store/history/version_entry.rs:628-650 |
| [L3-RV-TX-3B9F4D3B](P3.md#l3-rv-tx-3b9f4d3b) | fixed | crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs:224-250 |
| [L3-RV-TX-4D85CE16](P3.md#l3-rv-tx-4d85ce16) | fixed | crates/shamir-tx/src/version/versioned_overlay.rs:199-229 |
| [L3-RV-TX-52FB938F](P3.md#l3-rv-tx-52fb938f) | fixed | crates/shamir-types/src/core/sorted_index_layout.rs:3-31; crates/shamir-tx/src/gate/predicate_set.rs:159-179; crates/shamir-index/src/base_index/sorted_index_manager/scan.rs:670 |
| [L3-RV-TX-56651C4A](P3.md#l3-rv-tx-56651c4a) | fixed | crates/shamir-tx/src/changefeed/feed/batch.rs:24-35; crates/shamir-engine/src/repo/journal/changelog_store.rs:94-128 |
| [L3-RV-TX-782352E1](P3.md#l3-rv-tx-782352e1) | fixed | crates/shamir-tx/src/mvcc_store/store.rs:558-562,681-688,835-840; crates/shamir-tx/src/mvcc_store/write_receipt/writes.rs:18-57 |
| [L3-RV-TX-D70EC152](P3.md#l3-rv-tx-d70ec152) | fixed | crates/shamir-tx/src/tx/support/id_remap.rs:76-105; crates/shamir-tx/src/tx/tx_context.rs:977-983 |
| [L3-RV-TX-ED8562B0](P3.md#l3-rv-tx-ed8562b0) | fixed | crates/shamir-tx/src/tx/repo_wal_manager.rs:30-33,53-75; crates/shamir-tx/src/gate/repo_tx_gate.rs:370 |

### shamir-engine (7 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-ENGINE-3425FDE6](P3.md#l3-rv-engine-3425fde6) | fixed | crates/shamir-engine/src/query/batch/control/authorized.rs:50-55,74-80,99-135; crates/shamir-engine/src/query/batch/exec/batch_execute.rs:92; crates/shamir-engine/src/db_instance/db_instance.rs:102-105 |
| [L3-RV-ENGINE-80890C84](P3.md#l3-rv-engine-80890c84) | fixed | crates/shamir-engine/src/query/batch/tests/fk/actions/fk_indexed_action_read_error_tests.rs:610-635; crates/shamir-engine/src/query/batch/tests/fk/actions/mod.rs:4; crates/shamir-engine/src/table/tests/read/query/mod.rs:5; crat... |
| [L3-RV-ENGINE-95ECC2FE](P3.md#l3-rv-engine-95ecc2fe) | fixed | crates/shamir-engine-validator/src/validator/schema/rules/one_of_set.rs:15-17,54-74; crates/shamir-types/src/types/value.rs:375-400,782-800; crates/shamir-engine-validator/src/validator/schema/tests/schema_rules/check_tests.rs:... |
| [L3-RV-ENGINE-BE59E793](P3.md#l3-rv-engine-be59e793) | fixed | crates/shamir-engine/src/table/table_manager/diagnostics/doctor.rs:635-668; crates/shamir-index/src/base_index/index_manager_unique/create.rs:106-147 |
| [L3-RV-ENGINE-BE70B015](P3.md#l3-rv-engine-be70b015) | fixed | crates/shamir-engine-validator/src/validator/schema/schema_validator.rs:41-46,147-167; crates/shamir-engine-validator/src/validator/schema/rules/field_rule.rs:109-119; crates/shamir-engine-validator/src/validator/schema/rules/o... **(contested)** |
| [L3-RV-ENGINE-D0CF9DFA-SEMANTIC-FACET](P3.md#l3-rv-engine-d0cf9dfa-semantic-facet) | fixed | crates/shamir-engine/src/repo/instance/support/catalog/fk_reverse_cache.rs:349-361,379-392 |
| [L3-RV-ENGINE-FEF268D2](P3.md#l3-rv-engine-fef268d2) | fixed | crates/shamir-tx/src/tx/staging_store.rs:104-119,167-191,353-357; crates/shamir-engine/src/tx/preparation/pre_commit/prelock.rs:148-151 |

### shamir-db (19 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-DB-0541AF4D](P3.md#l3-rv-db-0541af4d) | fixed | crates/shamir-db/src/shamir_db/gateway/curl_gateway.rs:60-69,182-190; crates/shamir-db/src/shamir_db/gateway/curl_supervisor.rs:131-146 |
| [L3-RV-DB-140F9E99](P3.md#l3-rv-db-140f9e99) | fixed | crates/shamir-db/Cargo.toml:36-71; crates/shamir-db/src/lib.rs:8-9 |
| [L3-RV-DB-2A0B65CB](P3.md#l3-rv-db-2a0b65cb) | fixed | crates/shamir-db/src/shamir_db/shamir_db/db/db_gateway.rs:387-390; crates/shamir-db/src/shamir_db/gateway/net_grants.rs:32-50; crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs:660-662 |
| [L3-RV-DB-33CEC62D](P3.md#l3-rv-db-33cec62d) | fixed | crates/shamir-db/src/shamir_db/execute/core/helpers.rs:50-84; crates/shamir-types/src/core/interner/interner.rs:288-298,317-335 |
| [L3-RV-DB-3EB43AC7](P3.md#l3-rv-db-3eb43ac7) | fixed | crates/shamir-db/src/shamir_db/shamir_db/core.rs:363-369,394-421 |
| [L3-RV-DB-3FBFC9A8](P3.md#l3-rv-db-3fbfc9a8) | fixed | crates/shamir-db/src/shamir_db/execute/core/helpers.rs:93-103; crates/shamir-db/src/shamir_db/execute/admin_db/admin_retention.rs:238-258 |
| [L3-RV-DB-425999DD-SEMANTIC-FACET](P3.md#l3-rv-db-425999dd-semantic-facet) | fixed | crates/shamir-db/src/shamir_db/gateway/curl_gateway.rs:247-259; crates/shamir-db/src/shamir_db/gateway/curl_supervisor.rs:95-102 |
| [L3-RV-DB-48DA8907](P3.md#l3-rv-db-48da8907) | fixed | crates/shamir-db/src/shamir_db/shamir_db/core.rs:972-980 |
| [L3-RV-DB-49FED152](P3.md#l3-rv-db-49fed152) | fixed | crates/shamir-db/src/api/types.rs:8-11,31-40 |
| [L3-RV-DB-55B0A5D9](P3.md#l3-rv-db-55b0a5d9) | fixed | crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs:336-342; crates/shamir-db/src/shamir_db/execute/admin_meta/admin_function.rs:137-151 |
| [L3-RV-DB-72E8717C](P3.md#l3-rv-db-72e8717c) | fixed | crates/shamir-db/src/shamir_db/shamir_db/db/table_management.rs:302-306,409-429,460-479; crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_table_index.rs:169 |
| [L3-RV-DB-830B18FC](P3.md#l3-rv-db-830b18fc) | fixed | crates/shamir-db/src/shamir_db/gateway/curl_gateway.rs:148,200-220 |
| [L3-RV-DB-8FCABB03](P3.md#l3-rv-db-8fcabb03) | fixed | crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:40-85,107-110; crates/shamir-db/src/shamir_db/execute/admin_db/admin_db_repo.rs:54,69-71 |
| [L3-RV-DB-A05E9048](P3.md#l3-rv-db-a05e9048) | fixed | crates/shamir-db/src/shamir_db/execute/admin_ddl/schema_codec.rs:298-314,363,395-415; crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_schema.rs:627 |
| [L3-RV-DB-A571C2F8](P3.md#l3-rv-db-a571c2f8) | fixed | crates/shamir-db/src/shamir_db/execute/admin_db/admin_migration.rs:91-97; crates/shamir-engine/src/table/table_manager/barriers/ddl_barriers.rs:77-83 |
| [L3-RV-DB-D22B2BE9](P3.md#l3-rv-db-d22b2be9) | fixed | crates/shamir-db/src/shamir_db/execute/core/ambient_interner.rs:8-22,43-47 |
| [L3-RV-DB-D5441CCE](P3.md#l3-rv-db-d5441cce) | fixed | crates/shamir-db/src/shamir_db/shamir_db/db/schema_management.rs:15-30 |
| [L3-RV-DB-FA80A7B8](P3.md#l3-rv-db-fa80a7b8) | fixed | crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_users_roles.rs:147-154,186-193; crates/shamir-db/src/shamir_db/gateway/ports.rs:31-64 |
| [L3-RV-DB-FF77D3CB-SEMANTIC-FACET](P3.md#l3-rv-db-ff77d3cb-semantic-facet) | fixed | crates/shamir-db/src/shamir_db/execute/core/helpers.rs:50-84; crates/shamir-db/src/shamir_db/shamir_db/db/db_gateway.rs:95-117 |

### shamir-index (17 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-INDEX-0256E6B5](P3.md#l3-rv-index-0256e6b5) | fixed | crates/shamir-index/src/vector/quant/brute_force.rs:317-332,344-365 |
| [L3-RV-INDEX-12B88835](P3.md#l3-rv-index-12b88835) | fixed | crates/shamir-index/src/base_index/infra/ddl_op_log.rs:67-70; crates/shamir-index/src/base_index/index_manager/drop_rename.rs:263; crates/shamir-index/src/base_index/index_manager_unique/create.rs:360; crates/shamir-index/src/b... |
| [L3-RV-INDEX-1443E02B](P3.md#l3-rv-index-1443e02b) | fixed | crates/shamir-index/src/base_index/index_manager/lookup.rs:124-140; crates/shamir-index/src/base_index/index_manager/flags.rs:299-312 |
| [L3-RV-INDEX-3854F3CC](P3.md#l3-rv-index-3854f3cc) | fixed | crates/shamir-index/src/model/runtime/registry.rs:680-705,329-340,475-485 |
| [L3-RV-INDEX-55A6887F](P3.md#l3-rv-index-55a6887f) | fixed | crates/shamir-index/src/vector/quant/brute_force.rs:40-49,104,181-204 |
| [L3-RV-INDEX-6BE64AFC](P3.md#l3-rv-index-6be64afc) | fixed | crates/shamir-index/src/vector/hnsw_adapter/vector_impl.rs:575-596 |
| [L3-RV-INDEX-7826036A](P3.md#l3-rv-index-7826036a) | fixed | crates/shamir-index/src/model/meta/meta_envelope.rs:62-78; crates/shamir-index/src/model/meta/persistence.rs:219-223 |
| [L3-RV-INDEX-7BB73DA5](P3.md#l3-rv-index-7bb73da5) | fixed | crates/shamir-index/src/vector/hnsw_adapter/vector_impl.rs:202-205,441-444; crates/shamir-index/src/vector/hnsw_adapter/fit.rs:281,285-314 |
| [L3-RV-INDEX-8533E947](P3.md#l3-rv-index-8533e947) | fixed | crates/shamir-index/src/vector/snapshot/load.rs:74-80,143-148; crates/shamir-index/src/vector/snapshot/manifest.rs:35-60; crates/shamir-index/src/vector/snapshot/quantized_load.rs:22-25 |
| [L3-RV-INDEX-9733F1F3](P3.md#l3-rv-index-9733f1f3) | fixed | crates/shamir-index/src/backends/fts_ranked_backend.rs:193-206,401 |
| [L3-RV-INDEX-AA2CD737](P3.md#l3-rv-index-aa2cd737) | fixed | crates/shamir-index/src/vector/quant/adapter.rs:107-118; crates/shamir-index/src/vector/hnsw_adapter/vector_impl.rs:227-239 |
| [L3-RV-INDEX-ABD60B4B](P3.md#l3-rv-index-abd60b4b) | fixed | crates/shamir-index/src/lib.rs:8-16 |
| [L3-RV-INDEX-AEEDB51B](P3.md#l3-rv-index-aeedb51b) | fixed | crates/shamir-index/src/fts/bm25.rs:63-82,97-102,123-134 |
| [L3-RV-INDEX-B424A8D5-SEMANTIC-FACET](P3.md#l3-rv-index-b424a8d5-semantic-facet) | fixed | crates/shamir-index/src/model/runtime/actor.rs:104-114; crates/shamir-index/src/vector/quant/brute_force.rs:143-162 |
| [L3-RV-INDEX-CF7D545B](P3.md#l3-rv-index-cf7d545b) | fixed | crates/shamir-index/src/fts/bm25.rs:114-134 |
| [L3-RV-INDEX-ECE5EBA5](P3.md#l3-rv-index-ece5eba5) | fixed | crates/shamir-index/src/base_index/index_manager/flags.rs:529-537; crates/shamir-index/src/base_index/sorted_index_manager/ddl.rs:155-163 |
| [L3-RV-INDEX-F39434C2](P3.md#l3-rv-index-f39434c2) | fixed | crates/shamir-index/src/backends/fts_ranked_backend.rs:197-206,480-487; crates/shamir-index/src/base_index/sorted_index_manager/covering.rs:85-94; crates/shamir-index/src/fts/bm25.rs:123-128 |

### shamir-types (17 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-TYPES-1C6C4F54](P3.md#l3-rv-types-1c6c4f54) | fixed | crates/shamir-types/src/record_view/record_ref.rs:332-340; crates/shamir-types/src/record_view/lens/view.rs:416-455 |
| [L3-RV-TYPES-1EBA1D8A](P3.md#l3-rv-types-1eba1d8a) | fixed | crates/shamir-types/src/core/interner/interner.rs:183-194,309-340,553-556,616-622 |
| [L3-RV-TYPES-3CFA3FB6](P3.md#l3-rv-types-3cfa3fb6) | fixed | crates/shamir-types/src/core/interner/interner.rs:309-340,553-556,616-635 |
| [L3-RV-TYPES-437423C0](P3.md#l3-rv-types-437423c0) | fixed | crates/shamir-types/src/codecs/README.md:3-26,45-67; crates/shamir-types/src/codecs/interned/common.rs:40-43 |
| [L3-RV-TYPES-58E7E6DC](P3.md#l3-rv-types-58e7e6dc) | fixed | crates/shamir-types/src/codecs/interned/messagepack.rs:130-140,331-348; crates/shamir-types/src/codecs/interned/common.rs:26-33 |
| [L3-RV-TYPES-5DD242F6](P3.md#l3-rv-types-5dd242f6) | fixed | crates/shamir-types/src/core/interner/interner.rs:290-340,529-589 |
| [L3-RV-TYPES-6866D8B8](P3.md#l3-rv-types-6866d8b8) | fixed | crates/shamir-types/src/record_view/lens/skip.rs:185-201 |
| [L3-RV-TYPES-6C7C9E1E](P3.md#l3-rv-types-6c7c9e1e) | fixed | crates/shamir-types/src/record_view/tests/cmp/having_cards.rs:9-55; crates/shamir-types/src/record_view/tests/cmp/mod.rs:4 |
| [L3-RV-TYPES-71AFA12F](P3.md#l3-rv-types-71afa12f) | fixed | crates/shamir-types/src/entries/access/policy.rs:77-81; crates/shamir-types/src/entries/access/access_error.rs:5; crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs:73-78,121-125 |
| [L3-RV-TYPES-83113A25](P3.md#l3-rv-types-83113a25) | fixed | crates/shamir-types/src/codecs/basic/bincode.rs:5,19-25,46-50; crates/shamir-types/src/codecs/error.rs:3-10 |
| [L3-RV-TYPES-88540F60](P3.md#l3-rv-types-88540f60) | fixed | crates/shamir-types/src/entries/access.rs:12-27 |
| [L3-RV-TYPES-E82C6BE1](P3.md#l3-rv-types-e82c6be1) | fixed | crates/shamir-types/src/codecs/interned/messagepack.rs:659-682 |
| [L3-RV-TYPES-EFA1B8F4](P3.md#l3-rv-types-efa1b8f4) | fixed | crates/shamir-types/src/macros/mpack.rs:248-262; crates/shamir-types/src/macros/tests/mpack_tests.rs:341 |
| [L3-RV-TYPES-F070E432](P3.md#l3-rv-types-f070e432) | fixed | crates/shamir-types/src/codecs/interned/messagepack.rs:130-155,204-232,332-343 |
| [L3-RV-TYPES-F8FA8E1F](P3.md#l3-rv-types-f8fa8e1f) | fixed | crates/shamir-types/src/types/ids/record_id.rs:31-32,51-57,79-85,121-126 |
| [L3-RV-TYPES-F9A7C76C](P3.md#l3-rv-types-f9a7c76c) | fixed | crates/shamir-types/src/core/interner/interner.rs:309-340,553-556 |
| [L3-RV-TYPES-FEB0B5E4](P3.md#l3-rv-types-feb0b5e4) | fixed | crates/shamir-types/src/types/ids/record_id.rs:24-26,57-63,97-105 |

### shamir-collections (3 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-COLLECTIONS-201D84D3](P3.md#l3-rv-collections-201d84d3) | fixed | crates/shamir-collections/src/lib.rs:20-47; crates/shamir-collections/src/lib.rs:81-175 |
| [L3-RV-COLLECTIONS-A5293F80](P3.md#l3-rv-collections-a5293f80) | fixed | crates/shamir-collections/src/lib.rs:94-175; crates/shamir-types/src/codecs/interned/messagepack.rs:247-254; crates/shamir-types/src/codecs/interned/messagepack.rs:322-325 |
| [L3-RV-COLLECTIONS-E3B33E36](P3.md#l3-rv-collections-e3b33e36) | fixed | crates/shamir-collections/src/lib.rs:125-129 |

### shamir-tunables (2 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-TUNABLES-30F8ABA8](P3.md#l3-rv-tunables-30f8aba8) | fixed | crates/shamir-tunables/src/tests/runtime_tests.rs:61-102; crates/shamir-tunables/src/tests/mod.rs:3 |
| [L3-RV-TUNABLES-C09F4AA6](P3.md#l3-rv-tunables-c09f4aa6) | fixed | crates/shamir-tunables/src/runtime.rs:60-69; crates/shamir-tunables/src/tests/runtime_tests.rs:61-72 |

### shamir-query-types (24 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-QUERY-TYPES-0172CFB1](P3.md#l3-rv-query-types-0172cfb1) | fixed | crates/shamir-query-types/src/security/hmac.rs:41-46; crates/shamir-query-types/src/ops/write/inserted_record.rs:3-7 |
| [L3-RV-QUERY-TYPES-02448485](P3.md#l3-rv-query-types-02448485) | fixed | crates/shamir-query-types/src/read/core/query_record.rs:246-273 |
| [L3-RV-QUERY-TYPES-1C72378C](P3.md#l3-rv-query-types-1c72378c) | fixed | crates/shamir-query-types/src/wire/db_message.rs:202; crates/shamir-query-types/src/wire/change_password_secrets.rs:17-37 |
| [L3-RV-QUERY-TYPES-1EC04E32](P3.md#l3-rv-query-types-1ec04e32) | fixed | crates/shamir-query-types/src/ops/batch/ops/batch_op.rs:283-305; crates/shamir-query-types/src/ops/batch/ops/query_value_deserializer.rs:48-69 |
| [L3-RV-QUERY-TYPES-283D65CC](P3.md#l3-rv-query-types-283d65cc) | fixed | crates/shamir-query-types/src/read/filter/tests/filter_enum_tests.rs:235-247 |
| [L3-RV-QUERY-TYPES-3D922433](P3.md#l3-rv-query-types-3d922433) | fixed | crates/shamir-query-types/src/read/clause/limit.rs:125; crates/shamir-query-types/src/read/clause/limit.rs:157-197 |
| [L3-RV-QUERY-TYPES-4B4C84BF](P3.md#l3-rv-query-types-4b4c84bf) | fixed | crates/shamir-query-types/src/ops/batch/tests/planner_error_path_tests.rs:4-14 |
| [L3-RV-QUERY-TYPES-4BAE89B5](P3.md#l3-rv-query-types-4bae89b5) | fixed | crates/shamir-query-types/src/ops/batch/req/batch_limits.rs:35-91 |
| [L3-RV-QUERY-TYPES-4E49851D](P3.md#l3-rv-query-types-4e49851d) | fixed | crates/shamir-query-types/src/ops/batch/plan/planner.rs:486-524; crates/shamir-query-types/src/read/filter/filter_value.rs:65-72 |
| [L3-RV-QUERY-TYPES-5F9D5468](P3.md#l3-rv-query-types-5f9d5468) | fixed | crates/shamir-query-types/src/read/core/query_record.rs:282-285 |
| [L3-RV-QUERY-TYPES-952F5AB7](P3.md#l3-rv-query-types-952f5ab7) | fixed | crates/shamir-query-types/src/read/filter/tests/filter_enum_tests.rs:75-85 |
| [L3-RV-QUERY-TYPES-98401770](P3.md#l3-rv-query-types-98401770) | fixed | crates/shamir-query-types/src/read/tests/core/mod.rs:3; crates/shamir-query-types/src/ops/write/tests/mod.rs:3 |
| [L3-RV-QUERY-TYPES-A125F638](P3.md#l3-rv-query-types-a125f638) | fixed | crates/shamir-query-types/src/security/validator/mod.rs:5; crates/shamir-query-types/src/ops/call/mod.rs:9 |
| [L3-RV-QUERY-TYPES-A3D93402](P3.md#l3-rv-query-types-a3d93402) | fixed | crates/shamir-query-types/src/security/hmac.rs:238; crates/shamir-server/src/engine/db_handler/handlers/admin.rs:703; crates/shamir-server/src/engine/db_handler/repo_admin/tx_handlers.rs:170 |
| [L3-RV-QUERY-TYPES-B53EC5CE](P3.md#l3-rv-query-types-b53ec5ce) | fixed | crates/shamir-query-types/src/ops/batch/tests/planner_error_path_tests.rs:68; crates/shamir-query-types/src/ops/batch/tests/reference_parse_tests.rs:47; crates/shamir-query-types/src/read/tests/clause/pagination_page_tests.rs:3... |
| [L3-RV-QUERY-TYPES-B5653A74](P3.md#l3-rv-query-types-b5653a74) | fixed | crates/shamir-query-types/src/read/clause/limit.rs:247; crates/shamir-query-types/src/read/clause/limit.rs:259; crates/shamir-query-types/src/read/clause/limit.rs:382; crates/shamir-query-types/src/read/clause/limit.rs:403 |
| [L3-RV-QUERY-TYPES-BFFB2B3A](P3.md#l3-rv-query-types-bffb2b3a) | fixed | crates/shamir-query-types/src/ops/write/inserted_record.rs:1; crates/shamir-query-types/src/ops/write/inserted_record.rs:11; crates/shamir-query-types/src/ops/write/inserted_record.rs:39 |
| [L3-RV-QUERY-TYPES-C000A440-SEMANTIC-FACET](P3.md#l3-rv-query-types-c000a440-semantic-facet) | fixed | crates/shamir-query-types/src/read/clause/limit.rs:125; crates/shamir-query-types/src/read/clause/limit.rs:157; crates/shamir-query-types/src/read/clause/limit.rs:167 |
| [L3-RV-QUERY-TYPES-C7AEC31F](P3.md#l3-rv-query-types-c7aec31f) | fixed | crates/shamir-query-types/src/security/hmac.rs:108; crates/shamir-query-types/src/security/hmac.rs:139; crates/shamir-query-types/src/security/hmac.rs:170 |
| [L3-RV-QUERY-TYPES-D1C4DEAB](P3.md#l3-rv-query-types-d1c4deab) | fixed | crates/shamir-query-types/src/ops/batch/plan/planner.rs:185; crates/shamir-query-types/src/ops/batch/plan/planner.rs:258; crates/shamir-query-types/src/ops/batch/plan/planner.rs:295 |
| [L3-RV-QUERY-TYPES-D687DBA3](P3.md#l3-rv-query-types-d687dba3) | fixed | crates/shamir-query-types/src/read/filter/filter_expr.rs:8; crates/shamir-query-types/src/read/filter/mod.rs:18 |
| [L3-RV-QUERY-TYPES-F05184D8](P3.md#l3-rv-query-types-f05184d8) | fixed | crates/shamir-query-types/src/ops/write/inserted_record.rs:70; crates/shamir-query-types/src/ops/write/inserted_record.rs:81 |
| [L3-RV-QUERY-TYPES-F1DC8E9A](P3.md#l3-rv-query-types-f1dc8e9a) | fixed | crates/shamir-query-types/src/security/tests/hmac_tests.rs:268; crates/shamir-query-types/src/security/tests/hmac_tests.rs:284; crates/shamir-query-types/src/security/tests/hmac_tests.rs:294; crates/shamir-query-types/src/secur... |
| [L3-RV-QUERY-TYPES-FF6DD2A7](P3.md#l3-rv-query-types-ff6dd2a7) | fixed | crates/shamir-query-types/src/read/filter/filter_value.rs:12; crates/shamir-query-types/src/read/filter_value_decoder.rs:10; crates/shamir-query-types/src/read/filter_value_decoder.rs:95 |

### shamir-query-builder (13 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-QUERY-BUILDER-028443E5](P3.md#l3-rv-query-builder-028443e5) | fixed | crates/shamir-query-builder/src/wire/response/batch_response_ext.rs:171; crates/shamir-query-builder/src/wire/response/batch_response_ext.rs:201; crates/shamir-query-builder/src/wire/response/tests/response_tests.rs:182 |
| [L3-RV-QUERY-BUILDER-320AD085](P3.md#l3-rv-query-builder-320ad085) | fixed | crates/shamir-engine/src/query/batch/tests/executor/ops/when_skip_tests.rs:23; crates/shamir-engine/src/query/batch/tests/executor/ops/when_skip_tests.rs:52; crates/shamir-engine/src/query/batch/tests/executor/ops/mod.rs:4 |
| [L3-RV-QUERY-BUILDER-6931DBDE](P3.md#l3-rv-query-builder-6931dbde) | fixed | crates/shamir-query-builder/src/wire/to_wire.rs:7; crates/shamir-query-builder/src/wire/error.rs:3; crates/shamir-query-builder/src/wire/response/batch_response_ext.rs:39; crates/shamir-query-builder/src/wire/response/batch_res... |
| [L3-RV-QUERY-BUILDER-7B79D840](P3.md#l3-rv-query-builder-7b79d840) | fixed | crates/shamir-query-builder/src/write/ops/doc.rs:70; crates/shamir-query-builder/src/write/ops/doc.rs:76; crates/shamir-query-builder/src/write/ops/doc.rs:84 |
| [L3-RV-QUERY-BUILDER-8BEB53C0](P3.md#l3-rv-query-builder-8beb53c0) | fixed | crates/shamir-query-builder/src/batch/batch_refs.rs:602 |
| [L3-RV-QUERY-BUILDER-A548E006](P3.md#l3-rv-query-builder-a548e006) | fixed | crates/shamir-query-builder/src/macros/mod.rs:3 |
| [L3-RV-QUERY-BUILDER-C861C539](P3.md#l3-rv-query-builder-c861c539) | fixed | crates/shamir-query-builder/src/read/cursor.rs:81 |
| [L3-RV-QUERY-BUILDER-CFBEC44F](P3.md#l3-rv-query-builder-cfbec44f) | fixed | crates/shamir-query-builder/src/wire/to_wire.rs:6 |
| [L3-RV-QUERY-BUILDER-DE20E96B](P3.md#l3-rv-query-builder-de20e96b) | fixed | crates/shamir-query-builder/src/wire/response/batch_response_ext.rs:15 |
| [L3-RV-QUERY-BUILDER-E2602921](P3.md#l3-rv-query-builder-e2602921) | fixed | crates/shamir-query-builder/src/batch/batch_refs.rs:12 |
| [L3-RV-QUERY-BUILDER-E6F53E38](P3.md#l3-rv-query-builder-e6f53e38) | fixed | crates/shamir-query-builder/src/ddl/schema/function.rs:101 |
| [L3-RV-QUERY-BUILDER-FC88A91A](P3.md#l3-rv-query-builder-fc88a91a) | fixed | crates/shamir-query-builder/src/ddl/policy/auth.rs:12 |
| [L3-RV-QUERY-BUILDER-FE2003D1](P3.md#l3-rv-query-builder-fe2003d1) | fixed | crates/shamir-query-builder/src/read/val/filter_value.rs:74 |

### shamir-query-builder-macros (9 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-QUERY-BUILDER-MACROS-02993B77-SEMANTIC-FACET](P3.md#l3-rv-query-builder-macros-02993b77-semantic-facet) | fixed | crates/shamir-query-builder/src/lib.rs:135-136; crates/shamir-query-builder/src/lib.rs:263-264; crates/shamir-query-builder-macros/src/query_parse_lower.rs:261-268 |
| [L3-RV-QUERY-BUILDER-MACROS-03BA722D](P3.md#l3-rv-query-builder-macros-03ba722d) | fixed | crates/shamir-query-builder-macros/src/query_parse.rs:229 |
| [L3-RV-QUERY-BUILDER-MACROS-13A0D25A](P3.md#l3-rv-query-builder-macros-13a0d25a) | fixed | crates/shamir-query-builder-macros/src/filter_lower.rs:328 **(contested)** |
| [L3-RV-QUERY-BUILDER-MACROS-3C03A7DA](P3.md#l3-rv-query-builder-macros-3c03a7da) | fixed | crates/shamir-query-builder-macros/src/query_parse_lower.rs:54 |
| [L3-RV-QUERY-BUILDER-MACROS-A63844FD-FACET](P3.md#l3-rv-query-builder-macros-a63844fd-facet) | fixed | crates/shamir-query-builder-macros/src/tests/diagnostics_tests.rs:186 |
| [L3-RV-QUERY-BUILDER-MACROS-E24D5F62](P3.md#l3-rv-query-builder-macros-e24d5f62) | fixed | crates/shamir-query-builder-macros/src/query_parse.rs:566 |
| [L3-RV-QUERY-BUILDER-MACROS-E50875AB](P3.md#l3-rv-query-builder-macros-e50875ab) | fixed | crates/shamir-query-builder-macros/src/query_parse.rs:531 |
| [L3-RV-QUERY-BUILDER-MACROS-EEE42189](P3.md#l3-rv-query-builder-macros-eee42189) | fixed | crates/shamir-query-builder-macros/src/query_parse_lower.rs:88 |
| [L3-RV-QUERY-BUILDER-MACROS-FB738525](P3.md#l3-rv-query-builder-macros-fb738525) | fixed | crates/shamir-query-builder-macros/src/query_parse.rs:334; crates/shamir-query-builder-macros/src/query_parse.rs:413-438 |

### shamir-funclib (10 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-FUNCLIB-0418C20F](P3.md#l3-rv-funclib-0418c20f) | fixed | crates/shamir-funclib/src/functions/value/encode/core.rs:19-23; crates/shamir-funclib/src/functions/value/encode/core.rs:165; crates/shamir-types/src/types/value.rs:266-278 |
| [L3-RV-FUNCLIB-0CAAB4E1](P3.md#l3-rv-funclib-0caab4e1) | fixed | crates/shamir-funclib/src/functions/value/canonical/core.rs:162; crates/shamir-funclib/src/functions/value/canonical/core.rs:186-187; crates/shamir-funclib/src/functions/value/canonical/core.rs:201-222 |
| [L3-RV-FUNCLIB-1EF53AB4](P3.md#l3-rv-funclib-1ef53ab4) | fixed | crates/shamir-funclib/src/registry.rs:265-268; crates/shamir-funclib/src/functions/value/cast/core.rs:125-128 |
| [L3-RV-FUNCLIB-22DF7C3C](P3.md#l3-rv-funclib-22df7c3c) | fixed | crates/shamir-funclib/src/registry.rs:89-93; crates/shamir-funclib/src/registry.rs:132-144; crates/shamir-funclib/src/functions/numeric/arrays/core.rs:29 |
| [L3-RV-FUNCLIB-42F5C1C9](P3.md#l3-rv-funclib-42f5c1c9) | fixed | crates/shamir-funclib/src/functions/value/encode/core.rs:3-5; crates/shamir-funclib/src/functions/numeric/arrays/core.rs:4-5; crates/shamir-funclib/src/lib.rs:10-12 |
| [L3-RV-FUNCLIB-4936C6ED](P3.md#l3-rv-funclib-4936c6ed) | fixed | crates/shamir-funclib/src/tests/registry/scalar_resolver_tests.rs:102; crates/shamir-funclib/src/tests/registry/scalar_resolver_tests.rs:120; crates/shamir-funclib/src/tests/registry/scalar_resolver_tests.rs:176; crates/shamir-... |
| [L3-RV-FUNCLIB-54359750](P3.md#l3-rv-funclib-54359750) | fixed | crates/shamir-funclib/src/functions/value/canonical/core.rs:5-21; crates/shamir-funclib/src/functions/value/canonical/core.rs:162-168 |
| [L3-RV-FUNCLIB-935392A1](P3.md#l3-rv-funclib-935392a1) | fixed | crates/shamir-funclib/src/functions/crypto/tests/argon2_gate_tests.rs:17-67; crates/shamir-funclib/src/functions/crypto/core.rs:280-288; crates/shamir-funclib/src/functions/crypto/core.rs:332-341 |
| [L3-RV-FUNCLIB-AC384A3A](P3.md#l3-rv-funclib-ac384a3a) | fixed | crates/shamir-funclib/src/functions/value/canonical/core.rs:43-46; crates/shamir-funclib/src/functions/value/canonical/core.rs:190-191 |
| [L3-RV-FUNCLIB-E5A20918](P3.md#l3-rv-funclib-e5a20918) | fixed | crates/shamir-funclib/src/functions/value/object/core.rs:155-167 |

### shamir-wasm-host (16 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-WASM-HOST-1CF6A3BC](P3.md#l3-rv-wasm-host-1cf6a3bc) | fixed | crates/shamir-wasm-host/src/gateway/env_policy.rs:52,77,92-110; crates/shamir-wasm-host/src/gateway/net_gateway.rs:20 |
| [L3-RV-WASM-HOST-203F2147](P3.md#l3-rv-wasm-host-203f2147) | fixed | crates/shamir-wasm-host/src/wasm/bindings/call.rs:96,161; crates/shamir-wasm-host/src/runtime/context.rs:665-667 |
| [L3-RV-WASM-HOST-5377DABE](P3.md#l3-rv-wasm-host-5377dabe) | fixed | crates/shamir-wasm-host/src/builtin/meta.rs:85-92,244-251,268 |
| [L3-RV-WASM-HOST-57CD974C](P3.md#l3-rv-wasm-host-57cd974c) | fixed | crates/shamir-wasm-host/src/runtime/context.rs:545-550,654-659,710-714 |
| [L3-RV-WASM-HOST-58C052B0](P3.md#l3-rv-wasm-host-58c052b0) | fixed | crates/shamir-wasm-host/src/compile.rs:786-798; crates/shamir-proc-tree/src/process_tree_windows.rs:29-45; crates/shamir-proc-tree/src/process_tree_unix.rs:35-64,105-139; crates/shamir-proc-tree/src/supervise.rs:203-225 |
| [L3-RV-WASM-HOST-796ECAE4](P3.md#l3-rv-wasm-host-796ecae4) | fixed | crates/shamir-wasm-host/src/compile.rs:774,829-838 |
| [L3-RV-WASM-HOST-935F15D1](P3.md#l3-rv-wasm-host-935f15d1) | fixed | crates/shamir-wasm-host/src/gateway/env_policy.rs:92-110; crates/shamir-wasm-host/src/gateway/net_gateway.rs:20,107 |
| [L3-RV-WASM-HOST-98551A44](P3.md#l3-rv-wasm-host-98551a44) | fixed | crates/shamir-wasm-host/src/wasm/wasm_engine.rs:263,282-291; crates/shamir-wasm-host/src/wasm/wasm_function.rs:268,310,338 |
| [L3-RV-WASM-HOST-AAAE2B68](P3.md#l3-rv-wasm-host-aaae2b68) | fixed | crates/shamir-wasm-host/src/compile.rs:334-367 |
| [L3-RV-WASM-HOST-C51052DF](P3.md#l3-rv-wasm-host-c51052df) | fixed | crates/shamir-wasm-host/src/runtime/registry.rs:60-67; crates/shamir-wasm-host/src/runtime/context.rs:56-63,347-377,835-836 |
| [L3-RV-WASM-HOST-C5833F4E](P3.md#l3-rv-wasm-host-c5833f4e) | fixed | crates/shamir-wasm-host/src/wasm/wasm_function.rs:469-479 |
| [L3-RV-WASM-HOST-D808C5EB](P3.md#l3-rv-wasm-host-d808c5eb) | fixed | crates/shamir-wasm-host/src/wasm/wasm_function.rs:761-778 |
| [L3-RV-WASM-HOST-DF4AD1DA](P3.md#l3-rv-wasm-host-df4ad1da) | fixed | crates/shamir-wasm-host/src/gateway/db_gateway.rs:90-121; crates/shamir-wasm-host/src/gateway/net_gateway.rs:86-95 |
| [L3-RV-WASM-HOST-EAE6ABE8](P3.md#l3-rv-wasm-host-eae6abe8) | fixed | crates/shamir-wasm-host/src/runtime/registry.rs:90-115 |
| [L3-RV-WASM-HOST-EB1654FA](P3.md#l3-rv-wasm-host-eb1654fa) | fixed | crates/shamir-wasm-host/src/compile.rs:786-798; crates/shamir-proc-tree/src/supervise.rs:79-98 |
| [L3-RV-WASM-HOST-FE02BB1D](P3.md#l3-rv-wasm-host-fe02bb1d) | fixed | crates/shamir-wasm-host/src/builtin/argon2id.rs:34-44; crates/shamir-wasm-host/src/builtin/argon2id.rs:91-93 |

### shamir-sdk (8 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-SDK-152055F3](P3.md#l3-rv-sdk-152055f3) | fixed | crates/shamir-sdk/src/api/db.rs:70-91; crates/shamir-db/src/shamir_db/shamir_db/db/db_gateway.rs:65-72 |
| [L3-RV-SDK-17BC120C](P3.md#l3-rv-sdk-17bc120c) | fixed | crates/shamir-sdk/src/runtime/host_imports.rs:130-134; crates/shamir-sdk/src/runtime/host_imports.rs:275; crates/shamir-sdk/src/runtime/__rt.rs:131-171 |
| [L3-RV-SDK-23E9EE61](P3.md#l3-rv-sdk-23e9ee61) | fixed | crates/shamir-sdk/src/api/context.rs:81-94 |
| [L3-RV-SDK-77261152](P3.md#l3-rv-sdk-77261152) | fixed | crates/shamir-sdk/src/api/context.rs:14-17; crates/shamir-sdk-macros/src/lib.rs:625 |
| [L3-RV-SDK-8E69412C](P3.md#l3-rv-sdk-8e69412c) | fixed | crates/shamir-sdk/tests/function_std_result_spelling_853fc350.rs:3; crates/shamir-sdk/tests/validator_qualified_validation_853fc350.rs:3; crates/shamir-sdk/tests/h9/consumer_compile.rs:57-79 |
| [L3-RV-SDK-9ABDB893](P3.md#l3-rv-sdk-9abdb893) | fixed | crates/shamir-sdk/src/api/value.rs:125-130; crates/shamir-sdk/src/tests/value/value_tests.rs:92-117 |
| [L3-RV-SDK-E3875787](P3.md#l3-rv-sdk-e3875787) | fixed | crates/shamir-sdk/src/runtime/__rt.rs:114; crates/shamir-sdk/src/runtime/__rt.rs:189 |
| [L3-RV-SDK-E6E616ED](P3.md#l3-rv-sdk-e6e616ed) | fixed | crates/shamir-sdk/src/api/http.rs:245; crates/shamir-sdk/src/api/http.rs:252; crates/shamir-sdk/src/api/http.rs:273 |

### shamir-sdk-macros (10 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-SDK-MACROS-0A76D271](P3.md#l3-rv-sdk-macros-0a76d271) | fixed | crates/shamir-sdk-macros/src/lib.rs:197; crates/shamir-sdk-macros/src/lib.rs:351; crates/shamir-sdk-macros/src/lib.rs:494; crates/shamir-sdk-macros/src/lib.rs:708 |
| [L3-RV-SDK-MACROS-44A9CC7A](P3.md#l3-rv-sdk-macros-44a9cc7a) | fixed | crates/shamir-sdk-macros/src/lib.rs:622; crates/shamir-sdk-macros/src/lib.rs:676; crates/shamir-sdk/tests/h9/consumer_compile.rs:85 |
| [L3-RV-SDK-MACROS-5D1F61B0](P3.md#l3-rv-sdk-macros-5d1f61b0) | fixed | crates/shamir-sdk-macros/src/lib.rs:414; crates/shamir-sdk/tests/h9/consumer_compile.rs:61 |
| [L3-RV-SDK-MACROS-68D9A5D0](P3.md#l3-rv-sdk-macros-68d9a5d0) | refuted | crates/shamir-sdk-macros/src/lib.rs:306; crates/shamir-sdk-macros/src/lib.rs:314 |
| [L3-RV-SDK-MACROS-966AA3AD](P3.md#l3-rv-sdk-macros-966aa3ad) | fixed | crates/shamir-sdk-macros/src/lib.rs:75; crates/shamir-sdk-macros/src/lib.rs:141; crates/shamir-sdk-macros/src/lib.rs:647 |
| [L3-RV-SDK-MACROS-C8189C06](P3.md#l3-rv-sdk-macros-c8189c06) | fixed | crates/shamir-sdk-macros/src/lib.rs:17; crates/shamir-sdk-macros/src/lib.rs:637; crates/shamir-sdk/tests/h9/consumer_compile.rs:58 |
| [L3-RV-SDK-MACROS-D3AD2579](P3.md#l3-rv-sdk-macros-d3ad2579) | fixed | crates/shamir-sdk-macros/src/lib.rs:164; crates/shamir-sdk-macros/src/lib.rs:185; crates/shamir-sdk-macros/src/lib.rs:697 |
| [L3-RV-SDK-MACROS-D44EF500-FACET](P3.md#l3-rv-sdk-macros-d44ef500-facet) | fixed | crates/shamir-sdk/src/runtime/__rt.rs:266; crates/shamir-sdk/src/tests/runtime_tests.rs:116; crates/shamir-sdk/src/tests/runtime_tests.rs:132 |
| [L3-RV-SDK-MACROS-D7F6AEC5](P3.md#l3-rv-sdk-macros-d7f6aec5) | fixed | crates/shamir-sdk-macros/src/lib.rs:392; crates/shamir-sdk/src/runtime/__rt.rs:275; crates/shamir-sdk/src/runtime/__rt.rs:304 |
| [L3-RV-SDK-MACROS-FB968C81](P3.md#l3-rv-sdk-macros-fb968c81) | fixed | crates/shamir-sdk-macros/src/lib.rs:35; crates/shamir-sdk-macros/src/lib.rs:39; crates/shamir-sdk-macros/src/lib.rs:657 |

### shamir-numa (10 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-NUMA-43F38611](P3.md#l3-rv-numa-43f38611) | fixed | crates/shamir-numa/src/platform/linux.rs:178; crates/shamir-numa/src/platform/tests/linux_probe_tests.rs:92; crates/shamir-numa/src/platform/tests/mod.rs:4 |
| [L3-RV-NUMA-5CCCBB1A](P3.md#l3-rv-numa-5cccbb1a) | fixed | crates/shamir-numa/src/platform/linux.rs:78; crates/shamir-numa/src/platform/tests/linux_probe_tests.rs:19 |
| [L3-RV-NUMA-6FDBD35C](P3.md#l3-rv-numa-6fdbd35c) | fixed | crates/shamir-numa/src/platform/detect.rs:69; crates/shamir-index/src/base_index/meta/index_info.rs:146; crates/shamir-index/src/base_index/meta/index_info.rs:374; crates/shamir-index/src/base_index/sorted_index_manager/inner.r... |
| [L3-RV-NUMA-79E4752D](P3.md#l3-rv-numa-79e4752d) | fixed | crates/shamir-numa/src/core/node_replicated.rs:35; crates/shamir-numa/src/lib.rs:26 |
| [L3-RV-NUMA-B44795F2](P3.md#l3-rv-numa-b44795f2) | fixed | crates/shamir-numa/src/platform/detect.rs:22; crates/shamir-numa/src/platform/detect.rs:43; crates/shamir-numa/src/platform/detect.rs:51 |
| [L3-RV-NUMA-C46D0E41](P3.md#l3-rv-numa-c46d0e41) | fixed | crates/shamir-numa/src/platform/linux.rs:52-81; crates/shamir-numa/src/platform/tests/linux_probe_tests.rs:8-46; crates/shamir-numa/src/platform/tests/mod.rs:3-5 |
| [L3-RV-NUMA-CA89A9AE](P3.md#l3-rv-numa-ca89a9ae) | fixed | crates/shamir-numa/src/lib.rs:33-38; crates/shamir-numa/README.md:45-54; crates/shamir-numa/src/tests/mod.rs:1-3 |
| [L3-RV-NUMA-CF3796D2](P3.md#l3-rv-numa-cf3796d2) | fixed | crates/shamir-numa/src/platform/cpulist.rs:16; crates/shamir-numa/src/platform/cpulist.rs:44-60 |
| [L3-RV-NUMA-DB772C02](P3.md#l3-rv-numa-db772c02) | fixed | scripts/runners/numa-smoke-oracle.sh:3-8; scripts/runners/ci-qemu-numa-test.sh:250; scripts/runners/ci-qemu-numa-test.sh:267; scripts/runners/test-numa-smoke-oracle.sh:23-29 |
| [L3-RV-NUMA-DC25AAC1](P3.md#l3-rv-numa-dc25aac1) | fixed | crates/shamir-numa/tests/linux_topology.rs:19-40; crates/shamir-numa/src/platform/tests/linux_probe_tests.rs:49-63 |

### shamir-bench-utils (12 closed)

| Card | Now | Evidence |
|---|---|---|
| [L3-RV-BENCH-UTILS-126E5E6F](P3.md#l3-rv-bench-utils-126e5e6f) | fixed | crates/shamir-bench-utils/src/vector_data.rs:47-50; crates/shamir-engine/examples/vector_report.rs:339; crates/shamir-engine/examples/persisted_hnsw.rs:306 |
| [L3-RV-BENCH-UTILS-2CAF36EA](P3.md#l3-rv-bench-utils-2caf36ea) | fixed | crates/shamir-bench-utils/src/vector_data.rs:188-211; crates/shamir-bench-utils/src/vector_data/tests/generation_tests.rs:154-166 |
| [L3-RV-BENCH-UTILS-2F1D995C](P3.md#l3-rv-bench-utils-2f1d995c) | fixed | crates/shamir-bench-utils/src/vector_data.rs:245-254; crates/shamir-bench-utils/src/vector_data/vector_rows.rs:10-20; crates/shamir-engine/benches/vector_search/vector_search.rs:120-126 |
| [L3-RV-BENCH-UTILS-325F3AC6](P3.md#l3-rv-bench-utils-325f3ac6) | fixed | crates/shamir-bench-utils/src/vector_data/tests/gaussian_tests.rs:10-27; crates/shamir-bench-utils/src/vector_data/tests/gaussian_tests.rs:35-57; crates/shamir-bench-utils/src/vector_data/tests/mod.rs:1 |
| [L3-RV-BENCH-UTILS-67097ED5](P3.md#l3-rv-bench-utils-67097ed5) | fixed | crates/shamir-bench-utils/src/peak_mem.rs:12-24 |
| [L3-RV-BENCH-UTILS-77EB8D3E](P3.md#l3-rv-bench-utils-77eb8d3e) | fixed | crates/shamir-bench-utils/src/peak_mem/tests/measurement_tests.rs:64-108; crates/shamir-bench-utils/src/peak_mem.rs:202-203; crates/shamir-bench-utils/Cargo.toml:13-16 |
| [L3-RV-BENCH-UTILS-92355828](P3.md#l3-rv-bench-utils-92355828) | fixed | crates/shamir-bench-utils/src/vector_data.rs:95-113; crates/shamir-bench-utils/src/vector_data/tests/lcg_tests.rs:28-47 |
| [L3-RV-BENCH-UTILS-A7605FF1](P3.md#l3-rv-bench-utils-a7605ff1) | fixed | crates/shamir-index/src/vector/tests/quant/sq8_tests.rs:532-576; crates/shamir-index/src/vector/tests/quant/mod.rs:7; docs/dev-artifacts/audits/2026-10-30-p2-19-generator-lineage-audit.md:44-66 |
| [L3-RV-BENCH-UTILS-AF0F6362](P3.md#l3-rv-bench-utils-af0f6362) | fixed | crates/shamir-bench-utils/src/vector_data.rs:138-149; crates/shamir-bench-utils/src/vector_data.rs:188-194; crates/shamir-bench-utils/src/vector_data.rs:214-221 |
| [L3-RV-BENCH-UTILS-C5422902](P3.md#l3-rv-bench-utils-c5422902) | fixed | crates/shamir-bench-utils/src/peak_mem.rs:138-143; crates/shamir-bench-utils/src/peak_mem.rs:159-198; crates/shamir-bench-utils/src/peak_mem/tests/measurement_tests.rs:31-60 |
| [L3-RV-BENCH-UTILS-E1F3D036-FACET](P3.md#l3-rv-bench-utils-e1f3d036-facet) | fixed | crates/shamir-bench-utils/src/peak_mem.rs:33-42; crates/shamir-index/benches/create_index_streaming.rs:76 |
| [L3-RV-BENCH-UTILS-F3BD32CC](P3.md#l3-rv-bench-utils-f3bd32cc) | fixed | crates/shamir-bench-utils/src/vector_data.rs:89-93; crates/shamir-bench-utils/src/vector_data/tests/lcg_tests.rs:9-24 |

## Contested cards (pass 2 disagreed with pass 1)

| Card | Pass 1 -> pass 2 (used here) | What pass 1 missed |
|---|---|---|
| [L3-RV-CONNECT-2A423D73](P3.md#l3-rv-connect-2a423d73) | FIXED -> PARTIAL | First pass missed the card's production decoder-diagnostic facet: e.to_string remains wire-visible inside an Ok-serialized DbResponse::Error. |
| [L3-RV-ENGINE-BE70B015](P3.md#l3-rv-engine-be70b015) | PARTIAL -> FIXED | First pass missed that direct no-set evaluation is test/test-util-only; production nonempty constraints receive precomputed hash membership. |
| [L3-RV-NUMA-553CB81D](P3.md#l3-rv-numa-553cb81d) | FIXED -> PARTIAL | First pass confused affinity OOB coverage with explicit replica load_node/store_node OOB coverage; the existing load_local test is intercepted by mock clamping. |
| [L3-RV-QUERY-BUILDER-MACROS-13A0D25A](P3.md#l3-rv-query-builder-macros-13a0d25a) | PARTIAL -> FIXED | First pass missed that all authored field/alias literals now retain source spans; absent compiler-underlining fixtures are extra coverage, not surviving span loss. |
| [L3-RV-TRANSPORT-TCP-884DDBF5](P3.md#l3-rv-transport-tcp-884ddbf5) | FIXED -> PARTIAL | First pass missed oversized mismatched prefixes: the cap check returns TooLarge before body-length validation, contrary to the documented malformed-buffer contract. |
| [L3-RV-WAL-48749F97](P3.md#l3-rv-wal-48749f97) | FIXED -> PARTIAL | First pass missed public WalSegment::open resetting the retained maximum; SegmentSet restoration closes only the managed reopen path. |
| [L3-RV-WASM-HOST-91385BE0](P3.md#l3-rv-wasm-host-91385be0) | FIXED -> PARTIAL | First pass missed the explicitly scoped Params rejection obligation: added tests cover other categories but not malformed/non-map nested-call Params. |

## Static acceptance and limits

- Every one of the 340 cards appears exactly once in each pass; the results were parsed from the reviewers' strict jsonl blocks and checked for exact coverage (no missing, extra or duplicate ids).
- Orchestrator spot checks in the live code: 6 of the 10 contested cards of P3/P4 (L3-RV-TRANSPORT-TCP-884DDBF5, L3-RV-WAL-48749F97, L4-RV-INDEX-20415F00, L4-RV-BENCH-UTILS-4E7A9919, L3-RV-CONNECT-2A423D73, L3-RV-QUERY-BUILDER-MACROS-13A0D25A) and 6 randomly chosen cards on which both passes agreed on `FIXED` (seed 20261030: L3-RV-QUERY-BUILDER-MACROS-A63844FD-FACET, L3-RV-TRANSPORT-TCP-D1882068, L4-RV-WAL-0CAFAF79, L4-RV-FUNCLIB-E6DE0D0F, L4-RV-NUMA-81E50CBF, L3-RV-DB-FA80A7B8). All 12 were confirmed.
- No build, test or benchmark was run by this stage; file:line evidence refers to master `96d694f5`.
