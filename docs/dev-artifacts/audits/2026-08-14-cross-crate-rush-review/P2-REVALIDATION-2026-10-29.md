# P2 — revalidation of the active findings against current master

Navigation: [P2](P2.md) · [P1 post-merge audit](P1-POST-MERGE-AUDIT.md) · [P0](P0.md) · [P1](P1.md)

Revalidated on 2026-10-29 against upstream master `7f04e096` (after the P0/P1 remediation waves, PRs #311–#405). The previous revalidation of [P2](P2.md) was made against `b0e1ddf7` on 2026-10-24. This document records the current state only; no code was changed, nothing was built or run.

## Method

- Scope: the 339 active cards of [P2](P2.md) (312 `confirmed-open`, 18 `partially-fixed`, 9 `unverified`), including the extra card `XXS-NEW-TX-PREFIX`. The 298 closed or calibration cards are out of scope.
- Pass 1: 13 read-only rush reviewer sessions (R01–R13, grouped by crate). Each card was re-read in [P2](P2.md), located in the live code by symbol (old line numbers are stale after the layout campaign), checked against `git log b0e1ddf7..HEAD`, and given one verdict: `STILL_OPEN`, `PARTIAL`, `FIXED`, `REFUTED`, `CHANGED`, `UNVERIFIABLE`, with file:line evidence, a fix size (S ≤1 file/<40 lines, M a few files, L cross-crate or design), a kind and an owner-decision flag.
- Pass 2 (skeptical, independent): 6 sessions tried to refute pass 1 on 135 cards: every `FIXED` and `CHANGED` (29), every `PARTIAL` (32), every `UNVERIFIABLE` (8), every `high`/`medium` `STILL_OPEN` (26), and a stratified random sample of 40 of the 244 `low`/`nit` `STILL_OPEN` verdicts (seed 20261029, about 14% per crate).
- Orchestrator spot checks in the code: `XXS-NEW-TX-PREFIX` (fixed, `RV-TX-1480F306` filter on the decoded key), `RV-DB-01C2041B`, `RV-ENGINE-905A1F59`, `RV-STORAGE-0786A4D6` and the `MvccStore::locks` registry (all still open as described).
- Everything here is static analysis. A verdict is evidence from source and history, not a test result; CI remains the authority.

## Result

| Verdict | Status | Cards |
|---|---|---:|
| `STILL_OPEN` | `confirmed-open` | 270 |
| `PARTIAL` | `partially-fixed` | 32 |
| `CHANGED` | `changed` | 4 |
| `UNVERIFIABLE` | `unverified` | 8 |
| `FIXED` | `fixed` | 25 |
| `REFUTED` | `refuted` | 0 |
| **Total** | | **339** |

Still active (`confirmed-open` + `partially-fixed` + `changed`): **306**. Closed since 2026-10-24: **25** (all `fixed`, none refuted). Undecidable statically: **8**.

Transitions against the status recorded on 2026-10-24:

| Was | Now | Cards |
|---|---|---:|
| `confirmed-open` | `confirmed-open` | 270 |
| `confirmed-open` | `fixed` | 25 |
| `partially-fixed` | `partially-fixed` | 18 |
| `confirmed-open` | `partially-fixed` | 14 |
| `unverified` | `unverified` | 8 |
| `confirmed-open` | `changed` | 3 |
| `unverified` | `changed` | 1 |

Pass 2 changed no verdict and no risk grade on any of its 135 cards (135 of 135 agree). It adjusted the fix size on 14 cards and the owner-decision flag on 11 cards; this document uses the pass-2 value where one exists. The calibration sample found 0 of 40 false `STILL_OPEN`, so the other 204 `low`/`nit` verdicts stand without a second look.

Risk grade of the active cards (after re-grading):

| Risk | Active cards |
|---|---:|
| high | 3 |
| medium | 23 |
| low | 135 |
| nit | 145 |

Re-graded among the active cards: 21 (low→nit 13, nit→low 6, low→medium 1, medium→low 1). The per-crate tables show the old grade next to the new one.

Fix size and kind of the active cards:

| Size | Cards | | Kind | Cards |
|---|---:|---|---|---:|
| S | 85 | | code | 184 |
| M | 179 | | doc | 61 |
| L | 42 | | test | 30 |
| - | 0 | | decision | 24 |
|  |  | | dep | 7 |
|  |  | | - | 0 |

Cards flagged as needing an owner decision (public contract, wire format, dependency or policy): **85** of 306 active.

## By crate

| Crate | Active | high | medium | low | nit | Closed | Unverified | Owner decision |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| shamir-index | 35 | 0 | 11 | 13 | 11 | 1 | 0 | 11 |
| shamir-connect | 26 | 0 | 0 | 10 | 16 | 1 | 0 | 9 |
| shamir-storage | 21 | 0 | 1 | 11 | 9 | 3 | 0 | 7 |
| shamir-db | 18 | 1 | 2 | 8 | 7 | 2 | 0 | 5 |
| shamir-query-types | 18 | 0 | 0 | 10 | 8 | 0 | 0 | 5 |
| shamir-client | 16 | 0 | 2 | 8 | 6 | 3 | 0 | 2 |
| shamir-tx | 16 | 0 | 1 | 9 | 6 | 2 | 0 | 4 |
| shamir-engine | 15 | 1 | 1 | 8 | 5 | 0 | 3 | 1 |
| shamir-wal | 15 | 0 | 0 | 7 | 8 | 1 | 0 | 5 |
| shamir-funclib | 15 | 0 | 1 | 2 | 12 | 0 | 0 | 1 |
| shamir-transport-tcp | 14 | 0 | 0 | 3 | 11 | 1 | 0 | 2 |
| shamir-types | 14 | 0 | 1 | 8 | 5 | 3 | 0 | 4 |
| shamir-wasm-host | 14 | 0 | 1 | 7 | 6 | 4 | 0 | 7 |
| shamir-query-builder | 13 | 0 | 0 | 5 | 8 | 1 | 0 | 3 |
| shamir-client-node | 9 | 1 | 2 | 3 | 3 | 0 | 1 | 3 |
| shamir-numa | 8 | 0 | 0 | 6 | 2 | 0 | 0 | 1 |
| shamir-sdk | 8 | 0 | 0 | 4 | 4 | 0 | 0 | 3 |
| shamir-transport-ws | 8 | 0 | 0 | 2 | 6 | 1 | 0 | 4 |
| shamir-query-builder-macros | 7 | 0 | 0 | 2 | 5 | 0 | 1 | 1 |
| shamir-server | 6 | 0 | 0 | 3 | 3 | 1 | 2 | 3 |
| shamir-bench-utils | 3 | 0 | 0 | 2 | 1 | 0 | 1 | 1 |
| shamir-sdk-macros | 3 | 0 | 0 | 3 | 0 | 0 | 0 | 2 |
| shamir-transport-ipc | 3 | 0 | 0 | 1 | 2 | 0 | 0 | 1 |
| shamir-collections | 1 | 0 | 0 | 0 | 1 | 1 | 0 | 0 |

## Closed since 2026-10-24 (`fixed`)

| Card | Risk was | Evidence (code and commit) | Why it is closed |
|---|---|---|---|
| [XXS-NEW-TX-PREFIX](P2.md#xxs-tx-prefix-vacuum) | high | crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs:209-215; 199331cb (#329) | exact decoded-key filtering removes neighbouring-key selection. |
| [RV-CLIENT-1D274C86](P2.md#rv-client-1d274c86) | medium | crates/shamir-client/src/client/api.rs:211-226; crates/shamir-client/src/client/transport.rs:550-583; a50f2992 | One mutex now makes routing, installation and buffered flush coherent. |
| [RV-CLIENT-67D785CC](P2.md#rv-client-67d785cc) | low | crates/shamir-client/src/client/subscription.rs:44,103-110; crates/shamir-client/src/client/api.rs:215-216; a50f2992 | Sender-identity checking prevents an old handle from removing its replacement. |
| [RV-COLLECTIONS-E3B33E36](P2.md#rv-collections-e3b33e36) | low | crates/shamir-collections/src/lib.rs:126-130; commit 166e8244 (#354) | Both unconditional 15-20% performance assertions were removed. |
| [RV-CONNECT-C48FDE17](P2.md#rv-connect-c48fde17) | low | crates/shamir-connect/src/server/infra/audit_chain.rs:82-101,117-128; a25c84de (#315) | Every canonical length prefix is checked before concatenation. |
| [RV-DB-8FCABB03](P2.md#rv-db-8fcabb03) | low | crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:52-85,107-110; commit 3116812e | Serialized duplicate creation returns the original instance before persistence or publication. |
| [RV-DB-D22B2BE9](P2.md#rv-db-d22b2be9) | low | crates/shamir-db/src/shamir_db/execute/core/ambient_interner.rs:10-28,43-47; commit 643f75aa | Best-effort omission is now explicit; silent loading failure remains intentional policy. |
| [RV-INDEX-7826036A](P2.md#rv-index-7826036a) | low | crates/shamir-index/src/model/meta/meta_envelope.rs:62-77; commit 313bccf6 | Header magic and version are checked before typed payload decoding. |
| [RV-QUERY-BUILDER-CFBEC44F](P2.md#rv-query-builder-cfbec44f) | low | crates/shamir-query-builder/src/wire/mod.rs:21-30; crates/shamir-query-builder/src/wire/to_wire.rs:6-18; 3616e3ba | H8 moved the trait and blanket implementation out of mod.rs. |
| [RV-STORAGE-8DFF95BF](P2.md#rv-storage-8dff95bf) | low | crates/shamir-storage/src/stores/storage_cached.rs:321-345,433-448; 973d0a0 (#269) | Sync removes acknowledge backing first; async fills are fenced and failures terminal. |
| [RV-STORAGE-AE7F22EF](P2.md#rv-storage-ae7f22ef) | low | crates/shamir-storage/src/stores/storage_cached.rs:366-448; 973d0a0 (#269) | FIFO private-tree reload and epoch-checked fills replace live clearing. |
| [RV-TYPES-F9A7C76C](P2.md#rv-types-f9a7c76c) | low | crates/shamir-types/src/core/interner/interner.rs:270-301; f90cea3d (#361) | Occupied losers no longer advance current_id; only successful publication stores it. |
| [RV-WAL-4BD5E020](P2.md#rv-wal-4bd5e020) | low | crates/shamir-wal/src/segment/wal_segment.rs:515,742; e27aa271 | Remaining-byte guards precede frame-end arithmetic in both parsers. |
| [RV-WASM-HOST-AAAE2B68](P2.md#rv-wasm-host-aaae2b68) | low | crates/shamir-wasm-host/src/compile.rs:334-367; be062ea0 (#271) | Scanner consumes the escaped apostrophe before requiring the closing quote. |
| [RV-WASM-HOST-D808C5EB](P2.md#rv-wasm-host-d808c5eb) | low | crates/shamir-wasm-host/src/wasm/wasm_function.rs:756-770; be062ea0 (#271) | Classification now uses typed nested errors and Wasmtime traps. |
| [RV-CLIENT-CFE6B78B](P2.md#rv-client-cfe6b78b) | nit | crates/shamir-client/src/client/connect.rs:9,685-689; 24ac7a97 | RngCore is now imported in the file header. |
| [RV-SERVER-9C77C713](P2.md#rv-server-9c77c713) | nit | crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs:12,468 | Server-local zero-array duplication is gone. |
| [RV-STORAGE-7B098ADB](P2.md#rv-storage-7b098adb) | nit | crates/shamir-storage/src/stores/storage_cached.rs:143-168,275-280,412-446; 973d0a0 (#269) | Membership changes are conditional and serialized through one owner. |
| [RV-TRANSPORT-TCP-6392C63D](P2.md#rv-transport-tcp-6392c63d) | nit | crates/shamir-transport-tcp/src/tests/mod.rs:1; crates/shamir-transport-tcp/src/tests/framing_tests.rs:12; crates/shamir-transport-tcp/src/tests/tls_tests.rs:15 | Unit manifest now covers framing, listener, TLS and verifier modules. |
| [RV-TRANSPORT-WS-F6AAB91C](P2.md#rv-transport-ws-f6aab91c) | nit | crates/shamir-transport-ws/tests/framing_round_trip.rs:3-14 | The futures_util import is now in the file header. |
| [RV-TX-99487168](P2.md#rv-tx-99487168) | nit | crates/shamir-collections/src/lib.rs:23-47; b3cb621d (#334) | RU8 explicitly documents client-controlled inputs, bounds and the reopen trigger. |
| [RV-TYPES-47A3CB10](P2.md#rv-types-47a3cb10) | nit | crates/shamir-types/src/macros/mpack.rs:286-290 | Documentation explicitly says the public macro helper is not sealed. |
| [RV-TYPES-FC6867C2](P2.md#rv-types-fc6867c2) | nit | crates/shamir-types/src/types/value.rs:19-28 | Deprecated alias documentation now recommends QueryValue instead of blanket InnerValue migration. |
| [RV-WASM-HOST-8062B59A](P2.md#rv-wasm-host-8062b59a) | nit | crates/shamir-wasm-host/src/wasm/bindings/batch.rs:45-46; crates/shamir-wasm-host/src/wasm/bindings/globals.rs:101-102; 70f8ee60 (#343) | Async getter rewrite removed the disposable handle clones. |
| [RV-WASM-HOST-B3A8300E](P2.md#rv-wasm-host-b3a8300e) | nit | crates/shamir-wasm-host/src/wasm/bindings/call.rs:30-49 | Only one host_call rustdoc summary remains. |

## Changed shape (`changed`)

| Card | Risk | What is left |
|---|---|---|
| [RV-STORAGE-8FEF1C5A](P2.md#rv-storage-8fef1c5a) | low | Explicit accepted threat model replaces the unacknowledged premise; policy narration still drifts. |
| [RV-SERVER-A65118CC](P2.md#rv-server-a65118cc) | low | Upgraded from nit: transport accepts mapped loopback while server configuration rejects it. |
| [RV-CLIENT-17C0D288](P2.md#rv-client-17c0d288) | nit | scc insertion no longer replaces occupied senders, but ignored collisions and late-response reuse remain. |
| [RV-TRANSPORT-TCP-31C5E8A6](P2.md#rv-transport-tcp-31c5e8a6) | low | read_exact pair replaced; split staging remains, performance consequence unmeasured. |

## Undecidable statically (`unverified`)

| Card | What is missing |
|---|---|
| [RV-BENCH-UTILS-F53F775F](P2.md#rv-bench-utils-f53f775f) | No boundary-margin analysis or CI target witness establishes a deterministic-fixture flake. |
| [RV-SERVER-D5F412E2](P2.md#rv-server-d5f412e2) | Crate-wide absence proof is missing; shared awaited mutex use is observed, not a proven deadlock. |
| [RV-SERVER-CEE352CA](P2.md#rv-server-cee352ca) | Indexed matching and CV-first removal exist; claimed concurrent asymptotic bound is unproved. |
| [RV-CLIENT-NODE-C5E06B98](P2.md#rv-client-node-c5e06b98) | Published artifacts are missing evidence; local non-Windows binary loading is positively supported. |
| [RV-ENGINE-DD871E71](P2.md#rv-engine-dd871e71) | Selected meaningful tests cannot establish the universal coverage claim. |
| [RV-ENGINE-84C0E18F](P2.md#rv-engine-84c0e18f) | Named publication and collision sites improved; blanket benignity remains unproved. |
| [RV-ENGINE-1220BF81](P2.md#rv-engine-1220bf81) | Explicit compiled-size and pattern caps exist; complexity and timing assurance remains incomplete. |
| [RV-QUERY-BUILDER-MACROS-9BCA14B1](P2.md#rv-query-builder-macros-9bca14b1) | Uncapped recursion exists, but accepted depth and rustc-abort manifestation are missing. |

## Active `high` and `medium` cards

| Card | Crate | Risk | Size/kind | Owner | Title |
|---|---|---|---|---|---|
| [RV-CLIENT-NODE-C42AA40A](P2.md#rv-client-node-c42aa40a) | shamir-client-node | high | L/code | yes | Default object-level MessagePack conversion loses integer fidelity and rejects BigInt inputs |
| [RV-DB-01C2041B](P2.md#rv-db-01c2041b) | shamir-db | high | M/code | no | `resolve_in_group` silently converts group-lookup errors into `false` without a log |
| [RV-ENGINE-905A1F59](P2.md#rv-engine-905a1f59) | shamir-engine | high | L/code | no | A8 captured-ID flag covers only the latest staging call, not every staged record |
| [RV-CLIENT-56E4E99D](P2.md#rv-client-56e4e99d) | shamir-client | medium | M/code | no | Undecodable response frames are dropped at `debug` level; default-unbounded waiters hang |
| [RV-CLIENT-A911BA9E](P2.md#rv-client-a911ba9e) | shamir-client | medium | L/code | no | Malformed id-keyed rows can be returned successfully as empty or partial records |
| [RV-CLIENT-NODE-144A5920](P2.md#rv-client-node-144a5920) | shamir-client-node | medium | L/code | yes | `execute(object)` teaches hand-assembled wire shapes; no builder, no exported BatchRequest/BatchResponse types |
| [RV-CLIENT-NODE-684A48EB](P2.md#rv-client-node-684a48eb) | shamir-client-node | medium | S/doc | no | "Mirrors the Rust SDK 1:1" is false; resumption ticket getters dead-end with no resume path |
| [RV-DB-2C736C9E](P2.md#rv-db-2c736c9e) | shamir-db | medium | M/code | no | Function invocation scans the function catalogue twice plus two settings scans per call |
| [RV-DB-39178D2D](P2.md#rv-db-39178d2d) | shamir-db | medium | L/code | no | `InternerTouch` computes the epoch via a full interner traversal per touch |
| [RV-ENGINE-7CA0C179](P2.md#rv-engine-7ca0c179) | shamir-engine | medium | M/code | no | ContainsAllSet's seen-index optimization loses coercing numeric membership |
| [RV-FUNCLIB-0FCB1771](P2.md#rv-funclib-0fcb1771) | shamir-funclib | medium | M/code | yes | `compare`'s documented "total (transitive)" order is not strictly guaranteed on the lossy fallback paths |
| [RV-INDEX-002DDDA6](P2.md#rv-index-002ddda6) | shamir-index | medium | M/code | no | `IndexExpr::Scalar` evaluation constructs a fresh `Interner` per row |
| [RV-INDEX-2BA3B945](P2.md#rv-index-2ba3b945) | shamir-index | medium | L/code | yes | `IndexError` is stringly-typed; structured `DbError`s are flattened at every boundary |
| [RV-INDEX-40BCFF22](P2.md#rv-index-40bcff22) | shamir-index | medium | M/code | no | Vector snapshot v1 back-compat is claimed but has no working decode path, and the only test is vacuous |
| [RV-INDEX-4DC29EFB](P2.md#rv-index-4dc29efb) | shamir-index | medium | L/code | yes | Vector snapshot dump/load fully materializes the graph and sidecar in RAM |
| [RV-INDEX-53CEF405](P2.md#rv-index-53cef405) | shamir-index | medium | L/decision | yes | FTS posting keys hash untrusted token text with unkeyed `FxHasher` (`token_hash`) |
| [RV-INDEX-872016D9](P2.md#rv-index-872016d9) | shamir-index | medium | M/code | no | `FtsRankedBackend::plan_update` tokenizes the old record twice |
| [RV-INDEX-95108B6D](P2.md#rv-index-95108b6d) | shamir-index | medium | M/code | yes | Posting cache is bounded by entry count, not bytes |
| [RV-INDEX-9E0EC9A5](P2.md#rv-index-9e0ec9a5) | shamir-index | medium | M/code | yes | Snapshot load path can panic on corrupt-but-decodable persisted data; the sidecar carries no checksum |
| [RV-INDEX-9E472725](P2.md#rv-index-9e472725) | shamir-index | medium | L/decision | yes | Regular + unique index keys use two correlated FxHasher streams as "collision resistance"; unique constraints… |
| [RV-INDEX-C3FBD7AC](P2.md#rv-index-c3fbd7ac) | shamir-index | medium | M/code | no | AArch64 safe SQ8 byte-slice callers can reach an alignment-requiring u32 lane load |
| [RV-INDEX-FDC0161A](P2.md#rv-index-fdc0161a) | shamir-index | medium | M/code | no | Vector delta-log replay on restart applies ops one at a time |
| [RV-STORAGE-0786A4D6](P2.md#rv-storage-0786a4d6) | shamir-storage | medium | M/code | no | Trait-default range filter scans PAST the upper bound forever |
| [RV-TX-D1C20D85](P2.md#rv-tx-d1c20d85) | shamir-tx | medium | M/code | no | MvccStore::locks registry never evicts empty entries — unbounded growth under pessimistic workloads |
| [RV-TYPES-973FEEFD](P2.md#rv-types-973feefd) | shamir-types | medium | L/code | no | `Interner::with_state` silently collapses duplicate ids/names in hydrated state |
| [RV-WASM-HOST-C17D12C8](P2.md#rv-wasm-host-c17d12c8) | shamir-wasm-host | medium | M/decision | yes | `compile_rust_source` hardwires the SDK path to the build machine's `CARGO_MANIFEST_DIR` |

## Cards that need an owner decision

One line per card: what has to be decided is the first clause of the recipe.

**shamir-bench-utils** (1)

- `RV-BENCH-UTILS-92355828` (low, M/code): Enforce exclusive range endpoints, define invalid-bound handling and add adjacent-float tests with revision bump.

**shamir-client** (2)

- `RV-CLIENT-8997B366` (low, L/code): Protect resume ticket ownership, serialized payloads and framing scratch on all exits while retaining wire bytes.
- `RV-CLIENT-0157DABC` (nit, S/doc): Document RequestIdMismatch as reserved and not emitted, or remove it under an explicit API compatibility decision.

**shamir-client-node** (3)

- `RV-CLIENT-NODE-C42AA40A` (high, L/code): Define a lossless integer policy shared with TS DTOs and test independent int64 fixtures, bigint inputs and integer markers.
- `RV-CLIENT-NODE-144A5920` (medium, L/code): Integrate supported builder and batch DTO exports plus an execution adapter and replace handwritten wire examples.
- `RV-CLIENT-NODE-31D3BBE5` (nit, S/doc): Document the owner-approved Rust and npm version relationship rather than silently changing release versions.

**shamir-connect** (9)

- `RV-CONNECT-2B5AF3F2` (low, L/code): Bind immutable session identity before HMAC caching and use that identity for password-change proof binding.
- `RV-CONNECT-3380EB1C` (low, M/code): Use zeroizing ownership for server and resume secrets while preserving immutable key/cache consistency.
- `RV-CONNECT-387EAA05` (low, L/code): Introduce fallible entropy and clock APIs and propagate typed environmental errors through callers.
- `RV-CONNECT-7D41FB60` (low, L/code): Resolve the four component cards and close this aggregate without duplicating implementation work.
- `RV-CONNECT-E0A24E86` (low, M/code): Centralize the supported ticket version and reject unsupported versions during encryption.
- `RV-CONNECT-F34827AC` (low, M/code): Make ticket serialization reject ciphertext lengths above u16::MAX and test boundary round trips.
- `RV-CONNECT-3BAF631D` (nit, M/code): Reject an all-zero client nonce before replacing pending challenge state and test preservation of the previous challenge.
- `RV-CONNECT-C7F16BFB` (nit, S/dep): Remove the unused direct unicode-normalization dependency while retaining PRECIS normalization.
- `RV-CONNECT-E4631CD9` (nit, M/code): Return the common typed KDF error instead of allocated discarded String diagnostics.

**shamir-db** (5)

- `RV-DB-0541AF4D` (low, M/code): Define a response-byte budget and enforce it during transfer and bounded file reading.
- `RV-DB-140F9E99` (low, S/dep): Remove unused direct argon2/TLS dependencies and stale legacy-network rationale.
- `RV-DB-C2DCAC96` (low, L/code): Promote the builder to a production dependency and migrate facade construction with genuine exceptions documented.
- `RV-DB-0065D2A2` (nit, L/decision): Define informational versus integrity/version semantics before adding consumers or changing persisted metadata.
- `RV-DB-7C59E01B` (nit, M/decision): Choose bounded striped locking or an explicit retained-key budget without unsafe live-entry eviction.

**shamir-engine** (1)

- `RV-ENGINE-3425FDE6` (low, L/decision): Ratify and document trusted embedding APIs versus authorization-enforced batch execution.

**shamir-funclib** (1)

- `RV-FUNCLIB-0FCB1771` (medium, M/code): Implement exact cross-numeric ordering, reconcile distinct hashing, and add boundary-triple and permutation oracles.

**shamir-index** (11)

- `RV-INDEX-2BA3B945` (medium, L/code): Preserve typed storage errors and sources through index and engine boundaries.
- `RV-INDEX-4DC29EFB` (medium, L/code): Stream bounded snapshot chunks and move sidecar ownership while preserving generation coherence.
- `RV-INDEX-53CEF405` (medium, L/decision): Choose collision-resolving token identity with an explicit persisted FTS rebuild/version policy.
- `RV-INDEX-95108B6D` (medium, M/code): Reserve retained bytes atomically and account for admission, replacement, removal and invalidation.
- `RV-INDEX-9E0EC9A5` (medium, M/code): Validate persisted parameters fallibly and add versioned sidecar integrity coverage with corruption fixtures.
- `RV-INDEX-9E472725` (medium, L/decision): Choose collision-resolving canonical-value identity and an index migration/rebuild policy.
- `RV-INDEX-0695AADA` (low, L/decision): Define explicit token/work budgets and propagate typed over-budget errors without silent truncation.
- `RV-INDEX-3854F3CC` (low, M/decision): Choose multi-match selection semantics and maintain a lifecycle-synchronized reverse lookup.
- `RV-INDEX-605EEC5D` (low, L/decision): Define portable persisted-hash identity, golden keys and explicit upgrade or rebuild rules.
- `RV-INDEX-8533E947` (low, M/code): Validate single-component basenames and introduce versioned metadata integrity with a legacy-read policy.
- `RV-INDEX-FBB72544` (low, M/code): Canonicalize recursive Map hashing and provide persisted-index rebuild handling and order-parity tests.

**shamir-numa** (1)

- `RV-NUMA-6FDBD35C` (low, L/code): Introduce shared cached topology for constructors while retaining an explicit fresh-probe path.

**shamir-query-builder** (3)

- `RV-QUERY-BUILDER-9E172C69` (low, M/code): Add a consuming validated build path and use it where callers own the batch, retaining borrowed reuse.
- `RV-QUERY-BUILDER-FE2003D1` (low, L/decision): Specify and test large-integer ordering, range and membership across Big and decimal-string storage while preserving RU7.
- `RV-QUERY-BUILDER-DE20E96B` (nit, M/dep): Add thiserror and migrate the five error implementations while preserving messages and sources.

**shamir-query-builder-macros** (1)

- `RV-QUERY-BUILDER-MACROS-02993B77` (low, M/dep): Resolve the consumer dependency alias for all builder expansion paths and add an alias-only consumer fixture.

**shamir-query-types** (5)

- `RV-QUERY-TYPES-182C703E` (low, L/code): Migrate formatting to thiserror and adopt typed validation errors under an explicit API compatibility plan.
- `RV-QUERY-TYPES-4F767AA5` (low, L/code): Introduce fallible QueryValue conversion, migrate callers and reject unsupported maps without Null substitution.
- `RV-QUERY-TYPES-5F9D5468` (low, M/code): Make opaque-versus-nonopaque comparisons false and add a three-value transitivity regression.
- `RV-QUERY-TYPES-F05184D8` (low, M/code): Accept existing non-map wire values in the decoder and add scalar/list round-trip tests.
- `RV-QUERY-TYPES-CC015A3C` (nit, L/decision): Decide whether measured cost justifies changing public String ownership or explicitly accept default-name allocations.

**shamir-sdk** (3)

- `RV-SDK-77261152` (low, L/decision): Choose signature-only purity documentation or host-enforced scalar capability isolation.
- `RV-SDK-E3875787` (low, M/code): Restrict packed result ABI to wasm32 and use native-width interfaces for native testing.
- `RV-SDK-723F5FD2` (nit, L/doc): Explicitly define the below-2-GiB ABI range and align memory documentation and boundary fixtures.

**shamir-sdk-macros** (2)

- `RV-SDK-MACROS-44A9CC7A` (low, L/decision): Define host-enforced scalar capability policy and replace the lexical purity guarantee with accurate documentation and tests.
- `RV-SDK-MACROS-C8189C06` (low, M/code): Resolve dependency aliases and reject nonempty unsupported attribute tokens with consumer CI fixtures.

**shamir-server** (3)

- `RV-SERVER-A65118CC` (low, M/code): Choose mapped-IPv4 loopback policy and align or contract-test both validation paths.
- `RV-SERVER-E8E1D11B` (low, L/decision): Define persisted leader-pin enrollment, rotation, mismatch, and reconnect policy and wire it into replication.
- `RV-SERVER-97C240D7` (nit, M/decision): Choose protected-file/stdin credential input and deprecate or explicitly document plaintext argv input.

**shamir-storage** (7)

- `RV-STORAGE-0A9E3A41` (low, M/code): Return worker-start failures through DbResult and approve source-preserving error representations.
- `RV-STORAGE-1A5A9374` (low, S/doc): Document per-worker ordering and explicitly decide whether cross-route submission ordering is required.
- `RV-STORAGE-38F208C4` (low, L/decision): Approve the actual probabilistic uniqueness contract or implement cross-handle collision-safe insertion.
- `RV-STORAGE-9478B37E` (low, L/decision): Choose explicit partial-copy semantics or a rename-owned staging and recovery protocol.
- `RV-STORAGE-CC5C79BA` (low, M/code): Apply one approved backend-compatible name validator before ordinary and atomic keyspace operations.
- `RV-STORAGE-88C4F2CC` (nit, L/decision): Approve a cohesive API-polish scope before changing aliases, bounds, copy semantics or public errors.
- `RV-STORAGE-FB7C0794` (nit, M/code): Approve a key-size limit and enforce it with a bounded deserialization visitor before growth.

**shamir-transport-ipc** (1)

- `RV-TRANSPORT-IPC-FA481516` (low, M/decision): Record accepted conditional exposure and offer an owner-only socket parent as structural mitigation.

**shamir-transport-tcp** (2)

- `RV-TRANSPORT-TCP-4D95A5F8` (nit, M/code): Add capped writer companions while retaining existing default-cap wrappers.
- `RV-TRANSPORT-TCP-9C706A16` (nit, S/dep): Narrow Tokio library features and declare test-only runtime capabilities separately.

**shamir-transport-ws** (4)

- `RV-TRANSPORT-WS-F0A54173` (low, M/code): Validate origin patterns and serialized origins while preserving explicit one-level wildcard semantics.
- `RV-TRANSPORT-WS-460A02D6` (nit, S/dep): Remove the unused hex, serde, serde_bytes and rmp-serde dev-dependencies.
- `RV-TRANSPORT-WS-5D3BB859` (nit, S/code): Return a fixed browser origin-rejection body while preserving typed diagnostics internally.
- `RV-TRANSPORT-WS-B1243564` (nit, S/code): Classify present non-text browser origins as NotAllowed rather than Missing.

**shamir-tx** (4)

- `RV-TX-4A1FDAAE` (low, L/decision): Choose shared in-memory table-name storage while preserving serialized string values.
- `RV-TX-56651C4A` (low, L/code): Add ordered journal batch persistence without crossing gap or barrier boundaries.
- `RV-TX-782352E1` (low, M/code): Reject reserved empty payloads before version allocation while preserving an explicit deletion path.
- `RV-TX-ED8562B0` (low, M/code): Reject or normalize zero seeds consistently in both allocators and pin external-zero semantics.

**shamir-types** (4)

- `RV-TYPES-5E4BFC76` (low, L/decision): Define trusted-type/input constraints and require explicitly bounded decoding for recursive untrusted roots.
- `RV-TYPES-A7B1FCEC` (low, L/code): Introduce borrowed ancestor traversal and compatible metadata lookup while preserving nearest-first awaited authorization.
- `RV-TYPES-EFA1B8F4` (low, M/code): Add negative-literal object arms and CI cases with and without trailing commas while retaining explicit expression escapes.
- `RV-TYPES-FE40CB54` (low, M/code): Use safe String zeroization, settle public comparison semantics, and add lifecycle and feature CI coverage.

**shamir-wal** (5)

- `RV-WAL-A25EDE9D` (low, L/code): Introduce bounded incremental replay and migrate startup and live gap recovery consumers.
- `RV-WAL-DD492F5C` (low, M/doc): Document trusted directory writers, required ACLs and the absence of WAL authenticity guarantees.
- `RV-WAL-0843B96B` (nit, M/code): Remove the empty public segment_meta namespace and align documentation visibility.
- `RV-WAL-3F10E8A1` (nit, M/decision): Define operator diagnostic versus public-error policy and redact externally surfaced paths.
- `RV-WAL-5DFE355F` (nit, M/decision): Gate or restrict mark_poisoned after consumer review, or explicitly support it as an administrative API.

**shamir-wasm-host** (7)

- `RV-WASM-HOST-C17D12C8` (medium, M/decision): Define a deployment-supported SDK source and configurable runtime resolution with actionable errors.
- `RV-WASM-HOST-4669FDA6` (low, L/code): Introduce checked version-aware metadata decoding with explicit malformed-field errors and legacy defaults.
- `RV-WASM-HOST-98551A44` (low, M/decision): Define an engine memory ceiling and reconcile accepted function limits with pooling capacity.
- `RV-WASM-HOST-EB1654FA` (low, M/code): Drain both pipes continuously while retaining capped diagnostics with explicit truncation markers.
- `RV-WASM-HOST-FE02BB1D` (low, S/code): Distinguish blocking-task panic from cancellation and preserve a diagnostic panic cause.
- `RV-WASM-HOST-EBD32CA5` (nit, S/dep): Remove the unused direct serde dependency.
- `RV-WASM-HOST-FF7C2D6E` (nit, M/code): Complete the remaining path and metadata components through cards 796ECAE4 and 4669FDA6.

## All cards by crate

Columns: verdict, current risk (and the old one when it changed), size/kind of the fix, owner-decision flag, pass-2 result (`agree` = a skeptical reviewer confirmed it independently, `—` = not in the pass-2 set), evidence at `7f04e096` (symbols and files, not stale line numbers from the card), recipe.

### shamir-bench-utils (4)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-BENCH-UTILS-92355828](P2.md#rv-bench-utils-92355828) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-bench-utils/src/vector_data.rs:84-87 | Enforce exclusive range endpoints, define invalid-bound handling and add adjacent-float tests with revision bump. |
| [RV-BENCH-UTILS-F3BD32CC](P2.md#rv-bench-utils-f3bd32cc) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-bench-utils/src/vector_data.rs:50,73-81 | Use an exact 24-bit half-open mapping, add endpoint predecessor-seed tests and bump generator revision. |
| [RV-BENCH-UTILS-88D1646B](P2.md#rv-bench-utils-88d1646b) | `STILL_OPEN` | nit | S/doc | no | agree | crates/shamir-bench-utils/Cargo.toml:6; crates/shamir-bench-utils/src/lib.rs:9-12 | Describe the shipped fixture-generation and memory-measurement helpers instead of BENCH_QUICK. |
| [RV-BENCH-UTILS-F53F775F](P2.md#rv-bench-utils-f53f775f) | `UNVERIFIABLE` | nit | S/test | no | agree | crates/shamir-bench-utils/src/vector_data/tests/generation_tests.rs:57-75; crates/shamir-bench-utils/src/vector_data.rs:222-226 | Test zero-noise per-row construction identity and keep noisy geometric assertions separate. |

### shamir-client (19)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-CLIENT-56E4E99D](P2.md#rv-client-56e4e99d) | `STILL_OPEN` | medium (was low) | M/code | no | agree | crates/shamir-client/src/client/transport.rs:467-475,599-605 | Treat invalid envelopes as terminal protocol failures and drain waiters; test a peer that remains connected. |
| [RV-CLIENT-A911BA9E](P2.md#rv-client-a911ba9e) | `STILL_OPEN` | medium | L/code | no | agree | crates/shamir-types/src/record_view/lens/view.rs:41-45,268-304; crates/shamir-types/src/codecs/interned/codec.rs:197-203; crates/shamir-client/src/interner_cache/ops.rs:691-715 | Use fallible complete traversal with nested-map, cardinality and consumption validation while preserving unknown-ID refresh. |
| [RV-CLIENT-267BD36E](P2.md#rv-client-267bd36e) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-client/src/client/api.rs:425-439,602-603; crates/shamir-client/src/visibility/error_mapping.rs:21-30 | Remove the unreachable special error arm, document current behavior and test unknown versus unsupported status requests. |
| [RV-CLIENT-3D598780](P2.md#rv-client-3d598780) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-client/src/client/api.rs:256-262,637-638 | For v2 peers fill missing per-repository epoch entries without overwriting explicit values and test mixed maps. |
| [RV-CLIENT-462B7413](P2.md#rv-client-462b7413) | `STILL_OPEN` | low | S/code | no | — | crates/shamir-client/src/interner_cache/ops.rs:643-645 | Replace Bytes::to_vec with consuming Bytes-to-Vec conversion while preserving exact wire bytes. |
| [RV-CLIENT-5E5B78A0](P2.md#rv-client-5e5b78a0) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-client/src/client/cursor_stream.rs:237-252 | Publish the immutable cursor ID through a one-time cell and remove per-record locking while preserving cleanup. |
| [RV-CLIENT-8997B366](P2.md#rv-client-8997b366) | `STILL_OPEN` | low | L/code | yes | — | crates/shamir-client/src/client/options.rs:106; crates/shamir-client/src/client/connect.rs:677,709-715 | Protect resume ticket ownership, serialized payloads and framing scratch on all exits while retaining wire bytes. |
| [RV-CLIENT-9DEFDE43](P2.md#rv-client-9defde43) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-client/src/interner_cache/ops.rs:480-491,511; crates/shamir-types/src/codecs/interned/messagepack.rs:974-978 | Traverse Value::Set during key collection and add a cold-cache set-of-maps insertion regression. |
| [RV-CLIENT-C93ADFED](P2.md#rv-client-c93adfed) | `STILL_OPEN` | low | M/doc | no | agree | crates/shamir-client/src/lib.rs:10-18; crates/shamir-client/src/client/options.rs:38,52; crates/shamir-client/Cargo.toml:49-52 | Complete the ConnectOptions example and add an equivalent CI compile-checked example without enabling project-wide doctests. |
| [RV-CLIENT-EB08BA4C](P2.md#rv-client-eb08ba4c) | `STILL_OPEN` | low | M/code | no | agree | crates/shamir-client/src/interner_cache/ops.rs:390-393,483 | Collect borrowed unique names per repository and allocate only when ownership is needed. |
| [RV-CLIENT-0157DABC](P2.md#rv-client-0157dabc) | `STILL_OPEN` | nit | S/doc | yes | — | crates/shamir-client/src/error.rs:97; crates/shamir-client/src/client/transport.rs:630-632 | Document RequestIdMismatch as reserved and not emitted, or remove it under an explicit API compatibility decision. |
| [RV-CLIENT-153A118E](P2.md#rv-client-153a118e) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-client/src/client/transport.rs:94-107,540-544 | Dispatch once using existing envelope discriminators without changing wire format and retain malformed-input coverage. |
| [RV-CLIENT-17C0D288](P2.md#rv-client-17c0d288) | `CHANGED` | nit | M/code | no | agree | crates/shamir-client/src/client/api.rs:78-80,488,522 | Prevent request-ID reuse within a connection and close or reconnect on exhaustion; test forced wrap and late responses. |
| [RV-CLIENT-66E9A5FC](P2.md#rv-client-66e9a5fc) | `STILL_OPEN` | nit | M/test | no | — | crates/shamir-client/src/client/api.rs:480-487; crates/shamir-client/Cargo.toml:42-55 | Add allocation-count oracles and fixed-iteration client benchmarks with results recorded from CI. |
| [RV-CLIENT-E39C43D4](P2.md#rv-client-e39c43d4) | `PARTIAL` | nit | S/code | no | agree | crates/shamir-client/src/tests/receipts/batch_has_refs_tests.rs:19-20; crates/shamir-client/src/tests/wire/wire_version_tests.rs:8-9 | Hoist the remaining ReadQuery import from read_op into the file header. |
| [RV-CLIENT-F1276114](P2.md#rv-client-f1276114) | `STILL_OPEN` | nit | M/test | no | — | crates/shamir-client/src/tests/client/timeout_tests.rs:27-30,41-62,71-82 | Inject a controlled pending connector into the establishment-timeout path instead of depending on private-address routing. |
| [RV-CLIENT-1D274C86](P2.md#rv-client-1d274c86) | `FIXED` | medium | -/- | no | agree | crates/shamir-client/src/client/api.rs:211-226; crates/shamir-client/src/client/transport.rs:550-583; a50f2992 | None. |
| [RV-CLIENT-67D785CC](P2.md#rv-client-67d785cc) | `FIXED` | low | -/- | no | agree | crates/shamir-client/src/client/subscription.rs:44,103-110; crates/shamir-client/src/client/api.rs:215-216; a50f2992 | None. |
| [RV-CLIENT-CFE6B78B](P2.md#rv-client-cfe6b78b) | `FIXED` | nit | -/- | no | agree | crates/shamir-client/src/client/connect.rs:9,685-689; 24ac7a97 | None. |

### shamir-client-node (10)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-CLIENT-NODE-C42AA40A](P2.md#rv-client-node-c42aa40a) | `STILL_OPEN` | high | L/code | yes | agree | crates/shamir-client-node/js/wrapper.js:82,105; crates/shamir-client-node/package.json:33 | Define a lossless integer policy shared with TS DTOs and test independent int64 fixtures, bigint inputs and integer markers. |
| [RV-CLIENT-NODE-144A5920](P2.md#rv-client-node-144a5920) | `STILL_OPEN` | medium | L/code | yes | agree | crates/shamir-client-node/src/lib.rs:19-22; crates/shamir-client-node/js/wrapper.d.ts:67 | Integrate supported builder and batch DTO exports plus an execution adapter and replace handwritten wire examples. |
| [RV-CLIENT-NODE-684A48EB](P2.md#rv-client-node-684a48eb) | `STILL_OPEN` | medium | S/doc | no | agree | crates/shamir-client-node/src/lib.rs:3; crates/shamir-client-node/js/wrapper.d.ts:58-77 | Document the supported Node subset and ticket-getter limitation instead of claiming Rust SDK parity. |
| [RV-CLIENT-NODE-050EF626](P2.md#rv-client-node-050ef626) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-client-node/src/lib.rs:310; crates/shamir-client-node/js/wrapper.js:83,129-135 | Distinguish raw successful user IDs from error payloads and test the valid 16-byte error-marker collision fixture. |
| [RV-CLIENT-NODE-4E3D7A44](P2.md#rv-client-node-4e3d7a44) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-client-node/src/lib.rs:101-105,163,205,370-373 | Zeroize owned session and ticket snapshots on destruction and clear them on close without claiming JS-copy erasure. |
| [RV-CLIENT-NODE-F64125FA](P2.md#rv-client-node-f64125fa) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-client-node/js/wrapper.js:82,118,132 | Decode Buffers directly and avoid decoding discarded raw success payloads through unambiguous error discrimination. |
| [RV-CLIENT-NODE-31D3BBE5](P2.md#rv-client-node-31d3bbe5) | `STILL_OPEN` | nit | S/doc | yes | — | crates/shamir-client-node/Cargo.toml:3; crates/shamir-client-node/package.json:3; crates/shamir-client-node/native/index.js:197 | Document the owner-approved Rust and npm version relationship rather than silently changing release versions. |
| [RV-CLIENT-NODE-4B69BB13](P2.md#rv-client-node-4b69bb13) | `STILL_OPEN` | nit | S/doc | no | agree | crates/shamir-client-node/src/lib.rs:243-244,271-272 | Add an explicit sanctioned-FFI explanation covering both incoming MessagePack deserialization boundaries. |
| [RV-CLIENT-NODE-C35E4678](P2.md#rv-client-node-c35e4678) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-client-node/src/lib.rs:63-64,121-146,300-305 | Wrap native passwords immediately on entry and qualify the guarantee to owned native buffers, excluding JS copies. |
| [RV-CLIENT-NODE-C5E06B98](P2.md#rv-client-node-c5e06b98) | `UNVERIFIABLE` | low | M/decision | no | agree | crates/shamir-client-node/package.json:32-38; crates/shamir-client-node/native/index.js:204-216 | Choose source-first or prebuilt distribution, inspect packed artifacts and verify every advertised platform in CI. |

### shamir-collections (2)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-COLLECTIONS-4ED3931B](P2.md#rv-collections-4ed3931b) | `STILL_OPEN` | nit | S/code | no | agree | crates/shamir-collections/src/lib.rs:17-19 | Remove only the redundant std::cmp::Eq import. |
| [RV-COLLECTIONS-E3B33E36](P2.md#rv-collections-e3b33e36) | `FIXED` | low | -/- | no | agree | crates/shamir-collections/src/lib.rs:126-130; commit 166e8244 (#354) | None. |

### shamir-connect (27)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-CONNECT-2B5AF3F2](P2.md#rv-connect-2b5af3f2) | `STILL_OPEN` | low | L/code | yes | — | crates/shamir-connect/src/server/state/session.rs:244,285-288,491-497; crates/shamir-connect/src/server/auth/changepw.rs:115-144 | Bind immutable session identity before HMAC caching and use that identity for password-change proof binding. |
| [RV-CONNECT-3380EB1C](P2.md#rv-connect-3380eb1c) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-connect/src/server/infra/config.rs:26-31; crates/shamir-connect/src/server/state/resume.rs:145-153 | Use zeroizing ownership for server and resume secrets while preserving immutable key/cache consistency. |
| [RV-CONNECT-387EAA05](P2.md#rv-connect-387eaa05) | `STILL_OPEN` | low | L/code | yes | agree | crates/shamir-connect/src/common/primitives/crypto.rs:66-69; crates/shamir-connect/src/common/runtime/time.rs:18-21 | Introduce fallible entropy and clock APIs and propagate typed environmental errors through callers. |
| [RV-CONNECT-74705EAB](P2.md#rv-connect-74705eab) | `STILL_OPEN` | low | M/test | no | — | crates/shamir-connect/src/common/primitives/domain_tags.rs:17; crates/shamir-connect/src/common/tests/message_tests/auth_message_tests.rs:127,200; crates/shamir-connect/test-vectors/auth_message_default.toml:21 | Correct header and length oracles to 13 and 148 bytes without changing wire bytes. |
| [RV-CONNECT-781E7475](P2.md#rv-connect-781e7475) | `STILL_OPEN` | low | M/test | no | — | crates/shamir-connect/src/common/tests/crypto_tests/test_vectors_tests.rs:36-45; crates/shamir-connect/src/common/messages/changepw.rs:54; crates/shamir-connect/src/common/messages/bootstrap_message.rs:21 | Add independently derived complete-byte password-change and bootstrap payload fixtures and registered assertions. |
| [RV-CONNECT-7D41FB60](P2.md#rv-connect-7d41fb60) | `STILL_OPEN` | low (was nit) | L/code | yes | — | crates/shamir-connect/src/common/messages/auth_message.rs:6; crates/shamir-connect/src/common/primitives/kdf_params.rs:77; crates/shamir-connect/src/common/primitives/crypto.rs:69; crates/shamir-connect/src/server/auth/handshake.… | Resolve the four component cards and close this aggregate without duplicating implementation work. |
| [RV-CONNECT-CD221FBC](P2.md#rv-connect-cd221fbc) | `STILL_OPEN` | low | S/test | no | — | crates/shamir-connect/src/common/tests/message_tests/auth_message_tests.rs:149-151,173-175 | Assert both matches expressions and pin the intended nonce error diagnostics. |
| [RV-CONNECT-E0A24E86](P2.md#rv-connect-e0a24e86) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-connect/src/server/state/ticket.rs:172,188,224 | Centralize the supported ticket version and reject unsupported versions during encryption. |
| [RV-CONNECT-F34827AC](P2.md#rv-connect-f34827ac) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-connect/src/server/state/ticket.rs:114-120,132-135 | Make ticket serialization reject ciphertext lengths above u16::MAX and test boundary round trips. |
| [RV-CONNECT-F43EB41E](P2.md#rv-connect-f43eb41e) | `PARTIAL` | low | M/test | no | agree | crates/shamir-connect/src/server/tests/infra_tests/durable_counters_tests.rs:141-164; crates/shamir-connect/src/server/tests/session_flow/changepw_challenge_tests.rs:125-156; crates/shamir-connect/src/server/tests/security/rate_l… | Add failing snapshot load/save oracles and complete password-change exit-path buffer-wipe coverage. |
| [RV-CONNECT-349809B3](P2.md#rv-connect-349809b3) | `STILL_OPEN` | nit | S/doc | no | agree | crates/shamir-connect/src/common/user/username.rs:8-10,41-46; crates/shamir-connect/tests/identity/integration_username_precis.rs:30-41 | Document Lowercase_Mapping and preserved sharp-s without changing username identity semantics. |
| [RV-CONNECT-3BAF631D](P2.md#rv-connect-3baf631d) | `STILL_OPEN` | nit | M/code | yes | — | crates/shamir-connect/src/server/auth/changepw.rs:55-74,130-145; crates/shamir-connect/src/common/messages/changepw.rs:59-60 | Reject an all-zero client nonce before replacing pending challenge state and test preservation of the previous challenge. |
| [RV-CONNECT-4496810D](P2.md#rv-connect-4496810d) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-connect/src/server/state/session.rs:112,144-149 | Replace the stale RwLock Debug placeholder with an accurate immutable-snapshot label. |
| [RV-CONNECT-66EA32BD](P2.md#rv-connect-66ea32bd) | `STILL_OPEN` | nit | S/code | no | agree | crates/shamir-connect/src/common/messages/envelope.rs:95; crates/shamir-connect/src/server/state/session.rs:133 | Use limits::SESSION_ID_BYTES for the borrowed envelope session-id array length. |
| [RV-CONNECT-68CAD3B6](P2.md#rv-connect-68cad3b6) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-connect/src/server/limits/lockout.rs:254-257 | Declare an explicit approved Fx hasher for both lockout DashMaps. |
| [RV-CONNECT-699EBFCF](P2.md#rv-connect-699ebfcf) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-connect/src/server/ops/admin.rs:326-329; crates/shamir-connect/src/server/tests/security/post_auth_rate_limit_tests.rs:97-99 | Hoist the named Entry and concurrency-test imports to their respective file headers. |
| [RV-CONNECT-850A9AFD](P2.md#rv-connect-850a9afd) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-connect/src/server/auth/handshake.rs:244,423-425 | Remove the no-op function reference, dead documentation-target helper and imports kept solely by them. |
| [RV-CONNECT-8B168854](P2.md#rv-connect-8b168854) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-connect/src/server/infra/audit_chain.rs:429-435 | Derive thiserror::Error with explicit diagnostic messages for AuditError variants. |
| [RV-CONNECT-90EC2E05](P2.md#rv-connect-90ec2e05) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-connect/src/server/limits/rate_limit.rs:89,375-384 | Replace sliding-window terminology with token-bucket terminology in the trait and module documentation. |
| [RV-CONNECT-A57D515B](P2.md#rv-connect-a57d515b) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-connect/src/server/limits/argon2_semaphore.rs:10-12,80-101 | Require blocking acquisition inside spawn_blocking for async callers and correct the nonexistent wait reference. |
| [RV-CONNECT-C75B520F](P2.md#rv-connect-c75b520f) | `STILL_OPEN` | nit | S/test | no | — | crates/shamir-connect/src/common/tests/message_tests/push_envelope_tests.rs:5-10; crates/shamir-server/src/engine/subscriptions/core/bridge.rs:577 | Include Ready in the local PushKind round-trip loop. |
| [RV-CONNECT-C7F16BFB](P2.md#rv-connect-c7f16bfb) | `STILL_OPEN` | nit | S/dep | yes | — | crates/shamir-connect/Cargo.toml:40; crates/shamir-connect/src/common/user/username.rs:44-47 | Remove the unused direct unicode-normalization dependency while retaining PRECIS normalization. |
| [RV-CONNECT-C8866EB1](P2.md#rv-connect-c8866eb1) | `STILL_OPEN` | nit | M/doc | no | — | crates/shamir-connect/src/common/messages/auth_message.rs:6; crates/shamir-connect/src/common/messages/envelope.rs:90; crates/shamir-connect/tests/session/integration_session.rs:333 | Point documentation to the flat fixture directory and the exact owning_and_view compatibility test name. |
| [RV-CONNECT-E4631CD9](P2.md#rv-connect-e4631cd9) | `STILL_OPEN` | nit | M/code | yes | — | crates/shamir-connect/src/common/primitives/kdf_params.rs:77-88; crates/shamir-connect/src/client/handshake.rs:214-216 | Return the common typed KDF error instead of allocated discarded String diagnostics. |
| [RV-CONNECT-E6CF11C8](P2.md#rv-connect-e6cf11c8) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-connect/src/common/messages/auth_message.rs:82-97; crates/shamir-connect/src/common/primitives/domain_tags.rs:17 | Derive exact capacity from AUTH_V1.len() and the serialized fixed field sizes. |
| [RV-CONNECT-E71CB30A](P2.md#rv-connect-e71cb30a) | `STILL_OPEN` | nit | S/doc | no | agree | crates/shamir-connect/src/server/auth/changepw.rs:83-101 | Separate verification documentation from the session-invalidation finalization helper documentation. |
| [RV-CONNECT-C48FDE17](P2.md#rv-connect-c48fde17) | `FIXED` | low | -/- | no | agree | crates/shamir-connect/src/server/infra/audit_chain.rs:82-101,117-128; a25c84de (#315) | - |

### shamir-db (20)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-DB-01C2041B](P2.md#rv-db-01c2041b) | `STILL_OPEN` | high | M/code | no | agree | crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs:160-167; crates/shamir-types/src/entries/access/policy.rs:23-29 | Propagate group lookup errors into authorization denial and add a 0704 membership-read-failure regression. |
| [RV-DB-2C736C9E](P2.md#rv-db-2c736c9e) | `STILL_OPEN` | medium | M/code | no | agree | crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs:679-688; crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs:248 | Reuse one invocation authorization snapshot for access checks and effective-actor selection. |
| [RV-DB-39178D2D](P2.md#rv-db-39178d2d) | `STILL_OPEN` | medium | L/code | no | agree | crates/shamir-db/src/shamir_db/execute/admin_meta/admin_interner.rs:174-179; crates/shamir-types/src/core/interner/interner.rs:454-462 | Provide allocation-free published-ID maximum access while preserving explicit-ID and publication-gap semantics. |
| [RV-DB-0541AF4D](P2.md#rv-db-0541af4d) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-db/src/shamir_db/gateway/curl_gateway.rs:136,205-219 | Define a response-byte budget and enforce it during transfer and bounded file reading. |
| [RV-DB-140F9E99](P2.md#rv-db-140f9e99) | `STILL_OPEN` | low | S/dep | yes | — | crates/shamir-db/Cargo.toml:64,70-77 | Remove unused direct argon2/TLS dependencies and stale legacy-network rationale. |
| [RV-DB-2A0B65CB](P2.md#rv-db-2a0b65cb) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-db/src/shamir_db/shamir_db/db/db_gateway.rs:387-406; crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs:789 | Share precomputed effective grants with invalidation on function grants and global allowlist changes. |
| [RV-DB-3EB43AC7](P2.md#rv-db-3eb43ac7) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-db/src/shamir_db/shamir_db/core.rs:361-397 | Group table records once by database/repository while preserving registration-before-recovery order. |
| [RV-DB-830B18FC](P2.md#rv-db-830b18fc) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-db/src/shamir_db/gateway/curl_gateway.rs:250-259; crates/shamir-db/src/shamir_db/tests/gateway/curl_gateway_tests.rs:49-52 | Emit one comma-separated resolve address list with correct IPv6 syntax and replace the multi-IP oracle. |
| [RV-DB-A05E9048](P2.md#rv-db-a05e9048) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-db/src/shamir_db/execute/admin_ddl/schema_codec.rs:369-383; crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_schema.rs:629-632,703-741 | Make catalogue conversion fallible and default only absent legacy action fields before any persistence. |
| [RV-DB-A571C2F8](P2.md#rv-db-a571c2f8) | `PARTIAL` | low | M/code | no | agree | crates/shamir-db/src/shamir_db/execute/admin_db/admin_migration.rs:94-98,151-154; crates/shamir-engine/src/table/table_manager/barriers/ddl_barriers.rs:77-83 | Scope active migration identity to database/repository/table and cover concurrent and cross-database starts. |
| [RV-DB-C2DCAC96](P2.md#rv-db-c2dcac96) | `STILL_OPEN` | low | L/code | yes | — | crates/shamir-db/src/shamir_db/system_store/catalogue.rs:23-47; crates/shamir-db/Cargo.toml:83-109 | Promote the builder to a production dependency and migrate facade construction with genuine exceptions documented. |
| [RV-DB-0065D2A2](P2.md#rv-db-0065d2a2) | `STILL_OPEN` | nit | L/decision | yes | agree | crates/shamir-db/src/shamir_db/shamir_db/db/function_management.rs:195-217; crates/shamir-db/src/shamir_db/shamir_db/db/validator_management.rs:242-245 | Define informational versus integrity/version semantics before adding consumers or changing persisted metadata. |
| [RV-DB-425999DD](P2.md#rv-db-425999dd) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-db/src/shamir_db/gateway/curl_gateway.rs:281-283; crates/shamir-db/src/shamir_db/execute/admin_ddl/admin_table_index.rs:398-399,464 | Log contextual header-read failures; do not treat the guarded index unwrap as a current panic bug. |
| [RV-DB-77C5A24C](P2.md#rv-db-77c5a24c) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-db/src/shamir_db/gateway/curl_gateway.rs:177-178,227 | Remove the standalone empty statement and redundant cleanup commentary. |
| [RV-DB-7C59E01B](P2.md#rv-db-7c59e01b) | `STILL_OPEN` | nit | M/decision | yes | — | crates/shamir-db/src/shamir_db/shamir_db/core.rs:82-99 | Choose bounded striped locking or an explicit retained-key budget without unsafe live-entry eviction. |
| [RV-DB-C93A58BF](P2.md#rv-db-c93a58bf) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-db/src/shamir_db/mod.rs:9-10; crates/shamir-db/src/lib.rs:38 | Justify module_inception allowance as preservation of existing public module paths. |
| [RV-DB-CBB8403D](P2.md#rv-db-cbb8403d) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-db/src/main.rs:1; crates/shamir-db/src/api/types.rs:1,6-9; crates/shamir-db/src/api/tests/api_tests.rs:1 | Narrow deprecated allowances to legacy compatibility uses with a named reason. |
| [RV-DB-D5441CCE](P2.md#rv-db-d5441cce) | `STILL_OPEN` | nit (was low) | M/code | no | agree | crates/shamir-db/src/shamir_db/shamir_db/db/schema_management.rs:110-111,278,303,376,419; crates/shamir-db/src/shamir_db/shamir_db/core.rs:963-964 | Hoist production imports into file headers while preserving aliases and cfg requirements. |
| [RV-DB-8FCABB03](P2.md#rv-db-8fcabb03) | `FIXED` | low | -/- | no | agree | crates/shamir-db/src/shamir_db/shamir_db/db/db_management.rs:52-85,107-110; commit 3116812e | None required for live-instance overwrite. |
| [RV-DB-D22B2BE9](P2.md#rv-db-d22b2be9) | `FIXED` | low | -/- | no | agree | crates/shamir-db/src/shamir_db/execute/core/ambient_interner.rs:10-28,43-47; commit 643f75aa | None required for the former documentation/API contradiction. |

### shamir-engine (18)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-ENGINE-905A1F59](P2.md#rv-engine-905a1f59) | `STILL_OPEN` | high | L/code | no | agree | crates/shamir-tx/src/tx/staging_store.rs:126-134; crates/shamir-engine/src/tx/preparation/pre_commit/prelock.rs:152-158 | Track captured IDs per staged key and cover mixed raw/captured writes above the persisted watermark. |
| [RV-ENGINE-7CA0C179](P2.md#rv-engine-7ca0c179) | `STILL_OPEN` | medium | M/code | no | agree | crates/shamir-engine-query/src/query/filter/evaluation/filter_node/membership.rs:292-304,364-378; crates/shamir-engine-query/src/query/filter/evaluation/filter_node.rs:603-613 | Mark every coercion-equivalent requirement and add literal/dynamic parity regressions for both bitmask sizes. |
| [RV-ENGINE-3425FDE6](P2.md#rv-engine-3425fde6) | `PARTIAL` | low | L/decision | yes | agree | crates/shamir-engine/src/query/batch/control/authorized.rs:57,99-116; crates/shamir-engine/src/query/batch/exec/batch_execute.rs:91-96; crates/shamir-engine/src/db_instance/db_instance.rs:99 | Ratify and document trusted embedding APIs versus authorization-enforced batch execution. |
| [RV-ENGINE-3C71057D](P2.md#rv-engine-3c71057d) | `PARTIAL` | low (was nit) | L/code | no | agree | crates/shamir-engine/src/validator/engine_bound/validator_db.rs:291-304,328-360 | Reuse batch-scoped parent membership while preserving staged transaction visibility. |
| [RV-ENGINE-62C6F2B1](P2.md#rv-engine-62c6f2b1) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-engine-query/src/query/filter/caches/cond_cache.rs:56-57,98-99; crates/shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs:337 | Use stable query-plan-local cache slots instead of formatting content on each evaluation. |
| [RV-ENGINE-95ECC2FE](P2.md#rv-engine-95ecc2fe) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-engine-validator/src/validator/schema/rules/one_of_set.rs:80-83; crates/shamir-types/src/types/value.rs:381-386 | Preserve QueryValue NaN equality in precomputed membership and add optimized/fallback parity tests. |
| [RV-ENGINE-B442D16F](P2.md#rv-engine-b442d16f) | `PARTIAL` | low (was medium) | M/code | no | agree | crates/shamir-engine/src/db_instance/migration/coordinator.rs:400-422 | Consume owned shadow pages and apply ordered mixed batches without cloning Put payloads. |
| [RV-ENGINE-BE59E793](P2.md#rv-engine-be59e793) | `PARTIAL` | low | M/code | no | agree | crates/shamir-engine/src/table/table_manager/diagnostics/doctor.rs:635-660 | Provide streaming or borrowed unique-index backfill under the existing repair barrier. |
| [RV-ENGINE-BE70B015](P2.md#rv-engine-be70b015) | `PARTIAL` | low | M/code | no | agree | crates/shamir-engine-validator/src/validator/schema/rules/one_of_set.rs:73-90; crates/shamir-engine-validator/src/validator/schema/rules/field_rule.rs:340-350 | Preserve equality and introduce hashed residual membership plus borrowed probes where available. |
| [RV-ENGINE-FEF268D2](P2.md#rv-engine-fef268d2) | `PARTIAL` | low | L/code | no | agree | crates/shamir-engine/src/tx/preparation/pre_commit/prelock.rs:152-158; crates/shamir-tx/src/tx/staging_store.rs:126-134 | Share the per-key capture repair with 905A1F59 and decode only outstanding uncaptured rows. |
| [RV-ENGINE-06B39B8F](P2.md#rv-engine-06b39b8f) | `STILL_OPEN` | nit | M/code | no | agree | crates/shamir-engine/src/table/table_manager/accessors/table_accessors.rs:132-133; crates/shamir-engine/src/table/table_manager.rs:68 | Compute and store the existing table token at construction without changing its hash algorithm. |
| [RV-ENGINE-6CCD199D](P2.md#rv-engine-6ccd199d) | `PARTIAL` | nit | M/code | no | agree | crates/shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs:335-345; crates/shamir-engine/src/table/table_manager/accessors/table_accessors.rs:132-133; crates/shamir-engine-query/src/query/read/projection/select_p… | Consolidate token caching with 06B39B8F and retire fixed path-clone claims without changing owned outputs. |
| [RV-ENGINE-931318A1](P2.md#rv-engine-931318a1) | `PARTIAL` | nit | M/code | no | agree | crates/shamir-engine-validator/src/validator/schema/schema_validator.rs:146; crates/shamir-engine-validator/src/validator/schema/rules/cross_field.rs:83; crates/shamir-engine-validator/src/validator/execution/record_fields.rs:130… | Use inline cross-field paths and retain borrowed cached ID snapshots through probes. |
| [RV-ENGINE-CEBE3101](P2.md#rv-engine-cebe3101) | `PARTIAL` | nit | L/code | no | agree | crates/shamir-engine/src/table/read/planning/read_temporal.rs:684-698 | Add true batched history lookup preserving ID ordering and deterministic error selection. |
| [RV-ENGINE-D0CF9DFA](P2.md#rv-engine-d0cf9dfa) | `PARTIAL` | nit | M/code | no | agree | crates/shamir-engine/src/repo/instance/support/catalog/fk_reverse_cache.rs:349-361,379-392 | Release and reacquire the build lock between failed publication attempts and recheck cached state. |
| [RV-ENGINE-1220BF81](P2.md#rv-engine-1220bf81) | `UNVERIFIABLE` | low (was unverified) | M/doc | no | agree | crates/shamir-engine-query/src/query/filter/evaluation/semantics/pattern_guard.rs:31-42,71-79 | Document precise version-specific regex guarantees and replace historical timings with CI measurements. |
| [RV-ENGINE-84C0E18F](P2.md#rv-engine-84c0e18f) | `UNVERIFIABLE` | low (was unverified) | L/code | no | agree | crates/shamir-engine-validator/src/validator/registry/validator_registry.rs:106-123; crates/shamir-engine/src/repo/instance/repo_instance/catalog/repo_membership_impl.rs:216-246 | Inventory ignored scc outcomes and prove each caller contract or handle the failure explicitly. |
| [RV-ENGINE-DD871E71](P2.md#rv-engine-dd871e71) | `UNVERIFIABLE` | low (was unverified) | L/test | no | agree | crates/shamir-engine-query/src/query/filter/tests/eval_tests/collections/membership_literals_tests.rs:553-583; crates/shamir-engine-validator/src/validator/schema/tests/schema_rules/schema_validator_tests.rs:210-230 | Inventory assertions and registration exhaustively and add numeric, NaN, and mixed-A8 witnesses. |

### shamir-funclib (15)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-FUNCLIB-0FCB1771](P2.md#rv-funclib-0fcb1771) | `STILL_OPEN` | medium | M/code | yes | agree | crates/shamir-funclib/src/functions/value/compare/core.rs:107,124,143; crates/shamir-funclib/src/functions/numeric/agg/dedup.rs:1 | Implement exact cross-numeric ordering, reconcile distinct hashing, and add boundary-triple and permutation oracles. |
| [RV-FUNCLIB-22DF7C3C](P2.md#rv-funclib-22df7c3c) | `STILL_OPEN` | low | M/doc | no | agree | crates/shamir-funclib/src/registry.rs:95,121,143; crates/shamir-funclib/src/functions/numeric/arrays/core.rs:29; crates/shamir-engine/src/table/table_manager/admission/domain/index_admission/write_domain_index_admitted.rs:230 | Correct indexability claims and clarify explicit native embedder vouching while retaining the eligibility gate. |
| [RV-FUNCLIB-935392A1](P2.md#rv-funclib-935392a1) | `STILL_OPEN` | low | M/test | no | — | crates/shamir-funclib/src/functions/crypto/tests/crypto_tests.rs:246,279; crates/shamir-funclib/src/functions/crypto/core.rs:276 | Establish saturation with a permit-held test rendezvous and controlled release rather than KDF duration. |
| [RV-FUNCLIB-06F1EB02](P2.md#rv-funclib-06f1eb02) | `STILL_OPEN` | nit | S/code | no | agree | crates/shamir-funclib/src/value_nav.rs:121 | Format integer map keys into a stack buffer and borrow the formatted signed decimal string. |
| [RV-FUNCLIB-110F6E17](P2.md#rv-funclib-110f6e17) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-funclib/src/functions/crypto/core.rs:158,160,170,233,280,285 | Use RAII in-flight accounting and poison recovery for the unit notification mutex, with targeted unwind oracles. |
| [RV-FUNCLIB-37BE26F9](P2.md#rv-funclib-37be26f9) | `PARTIAL` | nit | M/code | no | agree | crates/shamir-funclib/src/functions/text/validate/core.rs:364; crates/shamir-funclib/src/functions/value/canonical/core.rs:186; crates/shamir-funclib/src/functions/numeric/agg/core.rs:830,866; crates/shamir-funclib/src/tests/valu… | Remove unused boolean state, complete type_of coverage, and consolidate remaining layout and header facets with related cards. |
| [RV-FUNCLIB-3E2AD533](P2.md#rv-funclib-3e2ad533) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-funclib/src/lib.rs:77; crates/shamir-funclib/src/scalar_resolver.rs:103; crates/shamir-funclib/src/functions/crypto/tests/crypto_tests.rs:208,288 | Hoist the cited function-local imports into their file headers with appropriate test-only boundaries. |
| [RV-FUNCLIB-50CD6B2E](P2.md#rv-funclib-50cd6b2e) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-funclib/src/registry.rs:166,175 | Document last-registration-wins for duplicate qualified scalar names. |
| [RV-FUNCLIB-7539483C](P2.md#rv-funclib-7539483c) | `STILL_OPEN` | nit | M/test | no | — | crates/shamir-funclib/src/functions/numeric/math/tests/mod.rs:2; crates/shamir-funclib/src/functions/numeric/math/tests/registry_tests.rs:1 | Move registry contract tests into the existing root registry test group and update manifests and imports. |
| [RV-FUNCLIB-7EDC7D1C](P2.md#rv-funclib-7edc7d1c) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-funclib/src/functions/text/strings.rs:133,140; crates/shamir-funclib/src/tests/strings_tests.rs:73 | Document past-end substring starts as empty output and negative arguments as errors. |
| [RV-FUNCLIB-8447708B](P2.md#rv-funclib-8447708b) | `STILL_OPEN` | nit | M/test | no | — | crates/shamir-funclib/src/tests/encode_tests.rs:164; crates/shamir-funclib/src/functions/text/validate/tests/validate_tests.rs:186 | Reformat the cited valid object JSON fixtures as multiline indented literals without changing test semantics. |
| [RV-FUNCLIB-9F35E215](P2.md#rv-funclib-9f35e215) | `STILL_OPEN` | nit | M/doc | no | — | crates/shamir-funclib/src/functions/numeric/gen/core.rs:15,63; crates/shamir-funclib/src/registry.rs:350; crates/shamir-funclib/src/functions/numeric/gen/tests/gen_tests.rs:75 | Document random as returning Dec and tighten its shape oracle to that existing return type. |
| [RV-FUNCLIB-BDCBEE51](P2.md#rv-funclib-bdcbee51) | `STILL_OPEN` | nit | M/test | no | — | crates/shamir-funclib/src/tests/mod.rs:1; crates/shamir-funclib/src/lib.rs:89 | Relocate the four category suites into owner-module tests directories while preserving root-owned registry tests. |
| [RV-FUNCLIB-C1C6EEC7](P2.md#rv-funclib-c1c6eec7) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-funclib/src/functions/value/encode.rs:7,16,142 | Replace str_escape_chars with json_escape in the registered function catalog. |
| [RV-FUNCLIB-E6DE0D0F](P2.md#rv-funclib-e6de0d0f) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-funclib/src/functions/value/cast/core.rs:178 | Replace lowercase allocation with borrowed trimmed ASCII-case-insensitive comparisons. |

### shamir-index (36)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-INDEX-002DDDA6](P2.md#rv-index-002ddda6) | `STILL_OPEN` | medium | M/code | no | agree | crates/shamir-index/src/model/query/expr.rs:172-187; crates/shamir-types/src/core/interner/interner.rs:109-112 | Use a lazy or evaluation-scoped conversion context while preserving map-key meaning and reentrancy. |
| [RV-INDEX-2BA3B945](P2.md#rv-index-2ba3b945) | `STILL_OPEN` | medium | L/code | yes | agree | crates/shamir-index/src/backends/backend.rs:100-109; crates/shamir-index/src/model/meta/persistence.rs:132-135 | Preserve typed storage errors and sources through index and engine boundaries. |
| [RV-INDEX-40BCFF22](P2.md#rv-index-40bcff22) | `STILL_OPEN` | medium | M/code | no | agree | crates/shamir-index/src/vector/snapshot/load.rs:46,94; crates/shamir-index/src/vector/tests/quant/quantization_snapshot_tests.rs:385-396 | Add historical v1 disk mirrors and version dispatch, tested against genuine historical bytes. |
| [RV-INDEX-4DC29EFB](P2.md#rv-index-4dc29efb) | `STILL_OPEN` | medium | L/code | yes | agree | crates/shamir-index/src/vector/snapshot/codec.rs:368-369,440-441; crates/shamir-index/src/vector/snapshot/load.rs:120-124,216-220 | Stream bounded snapshot chunks and move sidecar ownership while preserving generation coherence. |
| [RV-INDEX-53CEF405](P2.md#rv-index-53cef405) | `STILL_OPEN` | medium | L/decision | yes | agree | crates/shamir-index/src/fts/tokenizer.rs:447-452; crates/shamir-index/src/backends/fts_ranked_backend.rs:136-158 | Choose collision-resolving token identity with an explicit persisted FTS rebuild/version policy. |
| [RV-INDEX-872016D9](P2.md#rv-index-872016d9) | `STILL_OPEN` | medium | M/code | no | agree | crates/shamir-index/src/backends/fts_ranked_backend.rs:152-154,249-251 | Tokenize the old record once and derive both its token set and document length. |
| [RV-INDEX-95108B6D](P2.md#rv-index-95108b6d) | `STILL_OPEN` | medium | M/code | yes | agree | crates/shamir-index/src/base_index/index_manager/lookup.rs:488-510; crates/shamir-index/src/base_index/index_manager/inner.rs:34 | Reserve retained bytes atomically and account for admission, replacement, removal and invalidation. |
| [RV-INDEX-9E0EC9A5](P2.md#rv-index-9e0ec9a5) | `STILL_OPEN` | medium | M/code | yes | agree | crates/shamir-index/src/vector/snapshot/load.rs:150-160,183-188,303; crates/shamir-index/src/vector/quant/quant_meta.rs:73-74 | Validate persisted parameters fallibly and add versioned sidecar integrity coverage with corruption fixtures. |
| [RV-INDEX-9E472725](P2.md#rv-index-9e472725) | `STILL_OPEN` | medium | L/decision | yes | agree | crates/shamir-index/src/base_index/infra/index_keys.rs:227-258; crates/shamir-index/src/base_index/index_manager_unique/validate.rs:310-345; Cargo.lock:3030 | Choose collision-resolving canonical-value identity and an index migration/rebuild policy. |
| [RV-INDEX-C3FBD7AC](P2.md#rv-index-c3fbd7ac) | `STILL_OPEN` | medium | M/code | no | agree | crates/shamir-index/src/vector/quant/sq8.rs:222; crates/shamir-index/src/vector/simd/quantized.rs:454,623,772; rust-toolchain.toml:15 | Replace typed NEON lane loads with alignment-safe loads and add offset-slice safety coverage in CI. |
| [RV-INDEX-FDC0161A](P2.md#rv-index-fdc0161a) | `STILL_OPEN` | medium | M/code | no | agree | crates/shamir-index/src/vector/snapshot/load.rs:560-573; crates/shamir-index/src/vector/hnsw_adapter/vector_impl.rs:108-116 | Introduce bounded ordered replay batches that amortize blocking dispatch without changing repeated-rid ordering. |
| [RV-INDEX-0256E6B5](P2.md#rv-index-0256e6b5) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-index/src/vector/quant/brute_force.rs:262-330 | Move owned exact-search work into spawn_blocking and verify runtime progress in CI. |
| [RV-INDEX-0695AADA](P2.md#rv-index-0695aada) | `STILL_OPEN` | low | L/decision | yes | — | crates/shamir-index/src/fts/tokenizer.rs:143-158; crates/shamir-index/src/backends/fts_ranked_backend.rs:140-145 | Define explicit token/work budgets and propagate typed over-budget errors without silent truncation. |
| [RV-INDEX-12B88835](P2.md#rv-index-12b88835) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-index/src/base_index/index_manager/drop_rename.rs:260-262; crates/shamir-index/src/base_index/index_manager_unique/create.rs:347-349; crates/shamir-index/src/base_index/sorted_index_manager/ddl.rs:363-365,858-860 | Use consistent non-panicking timestamp conversion and cover pre-epoch input. |
| [RV-INDEX-1443E02B](P2.md#rv-index-1443e02b) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-index/src/base_index/index_manager/lookup.rs:55-65,119-133; crates/shamir-engine/src/table/table_manager/transactions/tx_insert.rs:354,571 | Apply single-row in-flight dirty capture to batch planning and test online-build interleaving in CI. |
| [RV-INDEX-3854F3CC](P2.md#rv-index-3854f3cc) | `STILL_OPEN` | low | M/decision | yes | — | crates/shamir-index/src/model/runtime/registry.rs:631-643,659-680 | Choose multi-match selection semantics and maintain a lifecycle-synchronized reverse lookup. |
| [RV-INDEX-55A6887F](P2.md#rv-index-55a6887f) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-index/src/vector/quant/brute_force.rs:84-92,175-180 | Publish structurally shared immutable chunks instead of deep-cloning the whole snapshot. |
| [RV-INDEX-605EEC5D](P2.md#rv-index-605eec5d) | `STILL_OPEN` | low | L/decision | yes | — | crates/shamir-index/Cargo.toml:36; crates/shamir-index/src/fts/tokenizer.rs:447-452; crates/shamir-index/src/model/meta/persistence.rs:37 | Define portable persisted-hash identity, golden keys and explicit upgrade or rebuild rules. |
| [RV-INDEX-6BE64AFC](P2.md#rv-index-6be64afc) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-index/src/vector/hnsw_adapter/vector_impl.rs:571-595 | Score borrowed or shared vector storage without nested concurrent-map lock acquisition. |
| [RV-INDEX-8533E947](P2.md#rv-index-8533e947) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-index/src/vector/snapshot/load.rs:204,239-247; crates/shamir-index/src/vector/snapshot/quantized_load.rs:15-22; crates/shamir-index/src/model/meta/meta_envelope.rs:14-18,62-78 | Validate single-component basenames and introduce versioned metadata integrity with a legacy-read policy. |
| [RV-INDEX-9733F1F3](P2.md#rv-index-9733f1f3) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-index/src/backends/fts_ranked_backend.rs:181-187 | Keep empty legacy defaults but reject corrupt nonempty postings, with ranked-lookup regression coverage. |
| [RV-INDEX-ECE5EBA5](P2.md#rv-index-ece5eba5) | `STILL_OPEN` | low | M/code | no | agree | crates/shamir-index/src/base_index/index_manager/flags.rs:530-540; crates/shamir-index/src/base_index/sorted_index_manager/ddl.rs:160-168 | Delete bounded resumable batches while preserving scan correctness, drain ordering and durable tombstones. |
| [RV-INDEX-F39434C2](P2.md#rv-index-f39434c2) | `PARTIAL` | low | M/code | no | agree | crates/shamir-index/src/fts/bm25.rs:105-127; crates/shamir-index/src/backends/fts_ranked_backend.rs:184-185,458-460; crates/shamir-index/src/base_index/sorted_index_manager/covering.rs:91 | Make remaining decode skips, defaults, encoding fallbacks and rejected arithmetic observable or explicit errors. |
| [RV-INDEX-FBB72544](P2.md#rv-index-fbb72544) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-index/src/backends/functional_backend.rs:180-187 | Canonicalize recursive Map hashing and provide persisted-index rebuild handling and order-parity tests. |
| [RV-INDEX-20415F00](P2.md#rv-index-20415f00) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-index/src/model/runtime/lifecycle.rs:31-53; crates/shamir-index/src/model/runtime/state.rs:73; crates/shamir-index/src/base_index/infra/index_record_key.rs:62,104-111 | Correct lifecycle documentation and retain public key APIs absent a separate breaking-change decision. |
| [RV-INDEX-32C98E9F](P2.md#rv-index-32c98e9f) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-index/src/model/query/expr.rs:87-88; crates/shamir-index/src/model/runtime/write_ops.rs:165-166; crates/shamir-index/src/fts/tokenizer.rs:448-449 | Hoist ordinary local imports to headers and remove redundant imports without changing cfg scope. |
| [RV-INDEX-54363D3B](P2.md#rv-index-54363d3b) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-index/src/vector/quant/brute_force.rs:59-65,131-133 | Add the required inline teardown-only contention-model explanation beside the join mutex. |
| [RV-INDEX-858BB5E1](P2.md#rv-index-858bb5e1) | `STILL_OPEN` | nit | M/doc | no | — | crates/shamir-index/src/model/runtime/reader_drain_gate.rs:85; crates/shamir-index/src/base_index/index_manager/lookup.rs:411,434-452 | Document permitted leaf-cache locking and the actual drain-order invariant in both locations. |
| [RV-INDEX-97E36480](P2.md#rv-index-97e36480) | `STILL_OPEN` | nit | M/doc | no | agree | crates/shamir-index/src/model/kind.rs:10-40,48-52; crates/shamir-index/src/model/query/expr.rs:19-22 | Document append-only bincode ordinal obligations on IndexKind, TokenizerKind and IndexExpr. |
| [RV-INDEX-ABD60B4B](P2.md#rv-index-abd60b4b) | `STILL_OPEN` | nit (was low) | S/doc | no | — | crates/shamir-index/src/lib.rs:8-11; crates/shamir-index/src/vector/quant/brute_force.rs:64 | Document the scoped hot-path locking policy and sanctioned lifecycle/DDL exceptions. |
| [RV-INDEX-AFA66311](P2.md#rv-index-afa66311) | `STILL_OPEN` | nit | M/test | no | — | crates/shamir-index/src/model/runtime/reader_drain_gate.rs:306-310; crates/shamir-index/build.rs:13-14 | Extract the loom model to a file-backed module while preserving feature gating and CI selection. |
| [RV-INDEX-B23D8FE7](P2.md#rv-index-b23d8fe7) | `STILL_OPEN` | nit | M/test | no | — | crates/shamir-index/src/vector/quant/quant_meta.rs:83-89; crates/shamir-index/src/vector/mod.rs:5,13 | Move QuantMeta tests into the appropriate tests directory and preserve CI discovery. |
| [RV-INDEX-B424A8D5](P2.md#rv-index-b424a8d5) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-index/src/model/runtime/actor.rs:100-103; crates/shamir-index/src/vector/quant/brute_force.rs:129-133 | Report join failures without changing signatures and avoid a poison panic in teardown-slot extraction. |
| [RV-INDEX-B683BA68](P2.md#rv-index-b683ba68) | `STILL_OPEN` | nit | M/doc | no | agree | crates/shamir-index/src/model/runtime/reader_drain_gate.rs:113; crates/shamir-index/src/base_index/meta/index_definition.rs:48 | Correct suffient and replace the stale provenance source reference with the actual type path. |
| [RV-INDEX-DE92095C](P2.md#rv-index-de92095c) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-index/src/model/runtime/write_ops.rs:184-190 | Build one name-to-backend lookup per application call while retaining provenance semantics. |
| [RV-INDEX-7826036A](P2.md#rv-index-7826036a) | `FIXED` | low | -/- | no | agree | crates/shamir-index/src/model/meta/meta_envelope.rs:62-77; commit 313bccf6 | None. |

### shamir-numa (8)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-NUMA-553CB81D](P2.md#rv-numa-553cb81d) | `PARTIAL` | low | M/test | no | agree | bcb21f4e; crates/shamir-numa/src/platform/tests/linux_probe_tests.rs:8-33; crates/shamir-numa/tests/linux_topology.rs:1-18; .github/workflows/numa.yml:40-57 | Add off-Linux detect and direct Linux explicit-node out-of-range assertions. |
| [RV-NUMA-6FDBD35C](P2.md#rv-numa-6fdbd35c) | `STILL_OPEN` | low | L/code | yes | — | crates/shamir-numa/src/platform/detect.rs:24-30; crates/shamir-numa/src/platform/linux.rs:52-53; crates/shamir-index/src/base_index/meta/index_info.rs:146,370 | Introduce shared cached topology for constructors while retaining an explicit fresh-probe path. |
| [RV-NUMA-79E4752D](P2.md#rv-numa-79e4752d) | `STILL_OPEN` | low | M/doc | no | agree | crates/shamir-numa/src/core/node_replicated.rs:35-41,96-97; crates/shamir-numa/src/lib.rs:26-29; crates/shamir-numa/README.md:24-26 | Replace absolute zero-overhead claims with the actual constant-time routing contract. |
| [RV-NUMA-B44795F2](P2.md#rv-numa-b44795f2) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-numa/src/platform/detect.rs:25-30; crates/shamir-numa/src/platform/linux.rs:174-180 | Expose the fallback reason through a once-only operational diagnostic. |
| [RV-NUMA-CA89A9AE](P2.md#rv-numa-ca89a9ae) | `STILL_OPEN` | low | M/doc | no | — | crates/shamir-numa/src/lib.rs:34-40,48-50; crates/shamir-numa/README.md:22,79-81 | Update shipped scope and roadmap while distinguishing smoke coverage from full guest integration. |
| [RV-NUMA-DB772C02](P2.md#rv-numa-db772c02) | `STILL_OPEN` | low | M/test | no | — | scripts/runners/ci-qemu-numa-test.sh:251-252,270-272 | Require distinct configured node IDs and add duplicate-announcement rejection fixtures in CI. |
| [RV-NUMA-81E50CBF](P2.md#rv-numa-81e50cbf) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-numa/README.md:41-42 | Document CI use of the central test wrapper and explicit permission for any local execution. |
| [RV-NUMA-E3044ABE](P2.md#rv-numa-e3044abe) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-numa/src/platform/detect.rs:25-27; crates/shamir-numa/src/platform/linux.rs:69-77; crates/shamir-numa/src/platform/cpulist.rs:66 | Remove the redundant positive-node check and second sorting pass while preserving probe invariants. |

### shamir-query-builder (14)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-QUERY-BUILDER-2A648617](P2.md#rv-query-builder-2a648617) | `STILL_OPEN` | low | S/doc | no | — | crates/shamir-query-builder/src/batch/batch_refs.rs:268-280 | Document quadratic switch lowering and require a separate design decision before changing the wire representation. |
| [RV-QUERY-BUILDER-320AD085](P2.md#rv-query-builder-320ad085) | `STILL_OPEN` | low | S/test | no | — | crates/shamir-query-builder/src/batch/batch_refs.rs:280; crates/shamir-engine-query/src/query/filter/evaluation/filter_node.rs:360 | Add execution-boundary coverage proving zero cases execute exactly the default operation. |
| [RV-QUERY-BUILDER-8BEB53C0](P2.md#rv-query-builder-8beb53c0) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-query-builder/src/batch/batch_refs.rs:558-601 | Add typed Call and scope-aware Subscribe reference collection with planner-parity tests. |
| [RV-QUERY-BUILDER-9E172C69](P2.md#rv-query-builder-9e172c69) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-query-builder/src/batch/batch_refs.rs:54-67,161; crates/shamir-sdk/src/api/db.rs:216-220 | Add a consuming validated build path and use it where callers own the batch, retaining borrowed reuse. |
| [RV-QUERY-BUILDER-FE2003D1](P2.md#rv-query-builder-fe2003d1) | `STILL_OPEN` | low | L/decision | yes | — | crates/shamir-query-builder/src/read/val/filter_value.rs:76; crates/shamir-engine-query/src/query/filter/evaluation/semantics/resolve.rs:197,237 | Specify and test large-integer ordering, range and membership across Big and decimal-string storage while preserving RU7. |
| [RV-QUERY-BUILDER-0E30EB25](P2.md#rv-query-builder-0e30eb25) | `STILL_OPEN` | nit | M/doc | no | agree | crates/shamir-query-builder/src/read/filter/leaf.rs:83; crates/shamir-query-builder/src/write/ops/insert.rs:44 | Replace stale constructor and macro names and update fallible-conversion commentary. |
| [RV-QUERY-BUILDER-202E00CD](P2.md#rv-query-builder-202e00cd) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-query-builder/src/read/select/select_item.rs:8; crates/shamir-query-builder/src/read/select/mod.rs:12; crates/shamir-query-builder/src/write/ops/update.rs:10; crates/shamir-query-builder/src/write/mod.rs:67 | Keep public manifest exports and make redundant implementation sibling imports private. |
| [RV-QUERY-BUILDER-21560B4A](P2.md#rv-query-builder-21560b4a) | `STILL_OPEN` | nit | M/doc | no | — | crates/shamir-query-builder/src/read/val/filter_value.rs:116; crates/shamir-query-builder/src/read/val/expr.rs:15; crates/shamir-query-builder/src/read/select/select_item.rs:42,76 | Document qualified imports or explicit aliases for simultaneous val/select use. |
| [RV-QUERY-BUILDER-7A0F3500](P2.md#rv-query-builder-7a0f3500) | `STILL_OPEN` | nit (was low) | M/code | no | — | crates/shamir-query-builder/src/ddl/schema/schema.rs:36,408,469,517,563; crates/shamir-query-builder/src/ddl/policy/access_control.rs:78 | Split independent builders into owner files while preserving public re-exports and layout limits. |
| [RV-QUERY-BUILDER-A548E006](P2.md#rv-query-builder-a548e006) | `STILL_OPEN` | nit (was low) | M/code | no | — | crates/shamir-query-builder/src/macros/mod.rs:30,56,72,98 | Move macro definitions into sibling implementation files while preserving exports and lexical wiring. |
| [RV-QUERY-BUILDER-C861C539](P2.md#rv-query-builder-c861c539) | `STILL_OPEN` | nit (was low) | M/test | no | — | crates/shamir-query-builder/src/read/cursor.rs:81-82; crates/shamir-query-builder/src/read/cursor/tests.rs:6 | Move cursor tests into a tests directory with a manifest and topical test file. |
| [RV-QUERY-BUILDER-DE20E96B](P2.md#rv-query-builder-de20e96b) | `STILL_OPEN` | nit (was low) | M/dep | yes | — | crates/shamir-query-builder/src/batch/op/build_error.rs:64; crates/shamir-query-builder/src/ddl/index/create_index_build_error.rs:131; crates/shamir-query-builder/Cargo.toml:15 | Add thiserror and migrate the five error implementations while preserving messages and sources. |
| [RV-QUERY-BUILDER-E2602921](P2.md#rv-query-builder-e2602921) | `STILL_OPEN` | nit (was low) | M/code | no | agree | crates/shamir-query-builder/src/batch/batch_refs.rs:330; crates/shamir-query-builder/src/batch/tests/flow/after_tests.rs:24 | Hoist production and test imports into file headers except genuine documented exceptions. |
| [RV-QUERY-BUILDER-CFBEC44F](P2.md#rv-query-builder-cfbec44f) | `FIXED` | nit (was low) | -/- | no | agree | crates/shamir-query-builder/src/wire/mod.rs:21-30; crates/shamir-query-builder/src/wire/to_wire.rs:6-18; 3616e3ba | None. |

### shamir-query-builder-macros (8)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-QUERY-BUILDER-MACROS-02993B77](P2.md#rv-query-builder-macros-02993b77) | `STILL_OPEN` | low (was nit) | M/dep | yes | — | crates/shamir-query-builder-macros/src/filter_lower.rs:62; crates/shamir-query-builder-macros/src/query_parse_lower.rs:29,250-253 | Resolve the consumer dependency alias for all builder expansion paths and add an alias-only consumer fixture. |
| [RV-QUERY-BUILDER-MACROS-13A0D25A](P2.md#rv-query-builder-macros-13a0d25a) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-query-builder-macros/src/filter_lower.rs:327-342; crates/shamir-query-builder-macros/src/query_parse_lower.rs:274-290,303-305 | Emit source-spanned string literals for field names and aliases and add CI diagnostic fixtures. |
| [RV-QUERY-BUILDER-MACROS-1AF72B7B](P2.md#rv-query-builder-macros-1af72b7b) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-query-builder-macros/src/filter_lower.rs:10,20,315; crates/shamir-query-builder-macros/src/query_parse_lower.rs:274,359 | Narrow helper visibility and consolidate equivalent field-path emitters while retaining the useful lowering seam. |
| [RV-QUERY-BUILDER-MACROS-83FE5F55](P2.md#rv-query-builder-macros-83fe5f55) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-query-builder-macros/src/lib.rs:63-64,166-175; crates/shamir-query-builder-macros/src/query_parse_lower.rs:263-269 | Document six forms and distinguish bare DTO results from fallible write terminals. |
| [RV-QUERY-BUILDER-MACROS-A690BEA8](P2.md#rv-query-builder-macros-a690bea8) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-query-builder-macros/src/filter_lower.rs:118,130-135,327-342 | Borrow callee identifiers and use existing argument storage instead of collecting a temporary reference vector. |
| [RV-QUERY-BUILDER-MACROS-E78E1666](P2.md#rv-query-builder-macros-e78e1666) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-query-builder-macros/src/lib.rs:135; crates/shamir-query-builder-macros/src/filter_lower.rs:132-305; crates/shamir-query-builder/src/read/filter/leaf.rs:296,314 | Document 17 supported DSL predicates and direct vector-option users to builder APIs. |
| [RV-QUERY-BUILDER-MACROS-EC1B86B3](P2.md#rv-query-builder-macros-ec1b86b3) | `STILL_OPEN` | nit | M/code | no | agree | crates/shamir-query-builder-macros/src/query_parse.rs:614-669 | Avoid redundant group reconstruction and prefix reparsing while preserving clause-keyword disambiguation. |
| [RV-QUERY-BUILDER-MACROS-9BCA14B1](P2.md#rv-query-builder-macros-9bca14b1) | `UNVERIFIABLE` | low | M/test | no | agree | crates/shamir-query-builder-macros/src/filter_lower.rs:26-38,58-67,100,339 | Establish an isolated pathological-input CI reproducer across parsing, lowering, diagnostics, and destruction before choosing limits. |

### shamir-query-types (18)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-QUERY-TYPES-02448485](P2.md#rv-query-types-02448485) | `STILL_OPEN` | low | S/code | no | agree | crates/shamir-query-types/src/read/core/query_record.rs:192,223,249,264,278 | Borrow Inserted fields directly in scalar accessors, matching the string accessor. |
| [RV-QUERY-TYPES-0A0D9092](P2.md#rv-query-types-0a0d9092) | `PARTIAL` | low | L/code | no | agree | crates/shamir-query-types/src/ops/batch/ops/query_entry.rs:94-122; crates/shamir-query-types/src/ops/batch/ops/batch_op.rs:712-716; crates/shamir-engine/src/query/batch/control/authorized.rs:109-114 | Compute reusable request classification, repository and access metadata once at the execution boundary. |
| [RV-QUERY-TYPES-182C703E](P2.md#rv-query-types-182c703e) | `STILL_OPEN` | low | L/code | yes | — | crates/shamir-query-types/src/ops/batch/core/batch_error.rs:300; crates/shamir-query-types/src/admin/types/db/retention.rs:41; crates/shamir-query-types/src/read/filter/filter_enum.rs:234 | Migrate formatting to thiserror and adopt typed validation errors under an explicit API compatibility plan. |
| [RV-QUERY-TYPES-3D922433](P2.md#rv-query-types-3d922433) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-query-types/src/read/clause/limit.rs:84-88,123,130-132 | Implement allocation-free encoded-value equivalence with numeric, float and map-order regression oracles. |
| [RV-QUERY-TYPES-4E49851D](P2.md#rv-query-types-4e49851d) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-query-types/src/ops/batch/plan/planner.rs:451-471 | Reuse buffered-value deserialization with the authoritative FilterValue decoder, preserving fallback and Cond validation. |
| [RV-QUERY-TYPES-4F767AA5](P2.md#rv-query-types-4f767aa5) | `STILL_OPEN` | low | L/code | yes | agree | crates/shamir-query-types/src/read/filter/filter_value.rs:180-201; crates/shamir-query-types/src/read/filter_value_decoder.rs:95-102 | Introduce fallible QueryValue conversion, migrate callers and reject unsupported maps without Null substitution. |
| [RV-QUERY-TYPES-5F9D5468](P2.md#rv-query-types-5f9d5468) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-query-types/src/read/core/query_record.rs:195,289-296 | Make opaque-versus-nonopaque comparisons false and add a three-value transitivity regression. |
| [RV-QUERY-TYPES-C000A440](P2.md#rv-query-types-c000a440) | `STILL_OPEN` | low (was nit) | M/code | no | — | crates/shamir-query-types/src/read/clause/limit.rs:123,132; rmp-serde registry source | Replace encoding-based equality with allocation-free wire-equivalent comparison. |
| [RV-QUERY-TYPES-D1C4DEAB](P2.md#rv-query-types-d1c4deab) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-query-types/src/ops/batch/plan/planner.rs:185-186,224,248,260-261 | Use the query map for alias membership and remove redundant ownership while preserving provenance and order. |
| [RV-QUERY-TYPES-F05184D8](P2.md#rv-query-types-f05184d8) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-query-types/src/ops/write/inserted_record.rs:70-72,102 | Accept existing non-map wire values in the decoder and add scalar/list round-trip tests. |
| [RV-QUERY-TYPES-0172CFB1](P2.md#rv-query-types-0172cfb1) | `STILL_OPEN` | nit (was low) | M/code | no | — | crates/shamir-query-types/src/ops/batch/plan/planner.rs:433,619,752; crates/shamir-query-types/src/ops/write/inserted_record.rs:96; crates/shamir-query-types/src/read/filter/filter_enum.rs:391 | Hoist function-local implementation imports while retaining genuine module-header and cfg exceptions. |
| [RV-QUERY-TYPES-4B4C84BF](P2.md#rv-query-types-4b4c84bf) | `STILL_OPEN` | nit (was low) | M/test | no | — | crates/shamir-query-types/src/ops/batch/tests/plan/planner_tests.rs:459; crates/shamir-query-types/src/wire/tests/repl_tests.rs:22; crates/shamir-query-types/src/read/tests/core/query_record_tests.rs:78 | Hoist standalone test imports to file headers without changing test logic. |
| [RV-QUERY-TYPES-881BC94F](P2.md#rv-query-types-881bc94f) | `STILL_OPEN` | nit | M/doc | no | — | crates/shamir-query-types/src/admin/types/ddl/validator_ops.rs:63-78; crates/shamir-query-types/src/read/clause/limit.rs:346; crates/shamir-query-types/src/wire/db_message.rs:380-382 | Correct wire examples, stale method names and default documentation against current DTOs and HMAC v2. |
| [RV-QUERY-TYPES-9223F400](P2.md#rv-query-types-9223f400) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-query-types/src/admin/types/db/db_ops.rs:6; crates/shamir-query-types/src/admin/types/db/schema_ops.rs:222; crates/shamir-query-types/src/admin/types/repl/repl_ops.rs:47; crates/shamir-query-types/src/read/core/read… | Consolidate internal helper uses while retaining the public schema helper as a forwarding compatibility wrapper. |
| [RV-QUERY-TYPES-BFFB2B3A](P2.md#rv-query-types-bffb2b3a) | `STILL_OPEN` | nit (was low) | S/doc | no | — | crates/shamir-query-types/src/ops/write/inserted_record.rs:1,32,39-40 | Narrow allocation-free wording to the actual no-intermediate-map guarantee. |
| [RV-QUERY-TYPES-CC015A3C](P2.md#rv-query-types-cc015a3c) | `STILL_OPEN` | nit | L/decision | yes | — | crates/shamir-query-types/src/ops/table_ref.rs:23; crates/shamir-query-types/src/ops/call/call_op.rs:7-8 | Decide whether measured cost justifies changing public String ownership or explicitly accept default-name allocations. |
| [RV-QUERY-TYPES-D319B6E3](P2.md#rv-query-types-d319b6e3) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-query-types/src/ops/write/types.rs:85-105; crates/shamir-engine/src/table/write/insert.rs:420-444 | Document existing values-first then idmsgpack ordering on InsertOp. |
| [RV-QUERY-TYPES-D687DBA3](P2.md#rv-query-types-d687dba3) | `STILL_OPEN` | nit (was low) | M/code | no | — | crates/shamir-query-types/src/read/filter/mod.rs:24 | Move FieldPath into a sibling definition file and preserve its existing public re-export. |

### shamir-sdk (8)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-SDK-17BC120C](P2.md#rv-sdk-17bc120c) | `PARTIAL` | low | M/code | no | agree | crates/shamir-sdk/src/runtime/__rt.rs:118-159,190-196 | Validate packed response ranges centrally before ownership reconstruction and test malformed ranges. |
| [RV-SDK-77261152](P2.md#rv-sdk-77261152) | `STILL_OPEN` | low | L/decision | yes | — | crates/shamir-sdk/src/api/context.rs:14-16,64-65; crates/shamir-sdk-macros/src/lib.rs:583-591 | Choose signature-only purity documentation or host-enforced scalar capability isolation. |
| [RV-SDK-9ABDB893](P2.md#rv-sdk-9abdb893) | `PARTIAL` | low | S/test | no | agree | crates/shamir-sdk/src/api/value.rs:125-134; eea842a6 (#313) | Add NaN bit/classification and infinity MessagePack fixtures. |
| [RV-SDK-E3875787](P2.md#rv-sdk-e3875787) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-sdk/src/runtime/__rt.rs:92-96 | Restrict packed result ABI to wasm32 and use native-width interfaces for native testing. |
| [RV-SDK-723F5FD2](P2.md#rv-sdk-723f5fd2) | `STILL_OPEN` | nit | L/doc | yes | agree | crates/shamir-sdk/src/runtime/__rt.rs:52-58; crates/shamir-wasm-host/src/wasm/wasm_function.rs:366-367,407-409 | Explicitly define the below-2-GiB ABI range and align memory documentation and boundary fixtures. |
| [RV-SDK-8BC846C6](P2.md#rv-sdk-8bc846c6) | `PARTIAL` | nit | S/doc | no | agree | crates/shamir-sdk/src/api/db.rs:104-111; crates/shamir-sdk/src/api/value.rs:9; b2e6383e (#363) | Correct the stale Value test path together with RV-SDK-DDF49297. |
| [RV-SDK-9D3236E4](P2.md#rv-sdk-9d3236e4) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-sdk/src/tests/value/value_tests.rs:146,279-280; crates/shamir-sdk/src/tests/validation_tests.rs:239,305 | Hoist non-exempt test imports to file headers. |
| [RV-SDK-DDF49297](P2.md#rv-sdk-ddf49297) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-sdk/src/api/value.rs:9; crates/shamir-sdk/src/tests/value/value_tests.rs:1 | Replace the stale test path with src/tests/value/value_tests.rs. |

### shamir-sdk-macros (3)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-SDK-MACROS-44A9CC7A](P2.md#rv-sdk-macros-44a9cc7a) | `STILL_OPEN` | low | L/decision | yes | — | crates/shamir-sdk-macros/src/lib.rs:523-532,552-556,604-610; crates/shamir-sdk/src/api/context.rs:64-75 | Define host-enforced scalar capability policy and replace the lexical purity guarantee with accurate documentation and tests. |
| [RV-SDK-MACROS-5D1F61B0](P2.md#rv-sdk-macros-5d1f61b0) | `STILL_OPEN` | low | S/doc | no | — | crates/shamir-sdk-macros/src/lib.rs:355-362; crates/shamir-sdk/src/api/db.rs:112 | Propagate query errors with ? and return Value::List(rows); compile the corrected example in CI. |
| [RV-SDK-MACROS-C8189C06](P2.md#rv-sdk-macros-c8189c06) | `STILL_OPEN` | low | M/code | yes | agree | crates/shamir-sdk-macros/src/lib.rs:107,243,375,565,605-606 | Resolve dependency aliases and reject nonempty unsupported attribute tokens with consumer CI fixtures. |

### shamir-server (9)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-SERVER-A65118CC](P2.md#rv-server-a65118cc) | `CHANGED` | low (was nit) | M/code | yes | agree | crates/shamir-server/src/boot/config.rs:939,991-995; crates/shamir-transport-tcp/src/listener.rs:51-55 | Choose mapped-IPv4 loopback policy and align or contract-test both validation paths. |
| [RV-SERVER-E8E1D11B](P2.md#rv-server-e8e1d11b) | `STILL_OPEN` | low | L/decision | yes | — | crates/shamir-server/src/sync/replication/transport/prod_factory.rs:114-117; crates/shamir-connect/src/client/handshake.rs:263-294 | Define persisted leader-pin enrollment, rotation, mismatch, and reconnect policy and wire it into replication. |
| [RV-SERVER-F60C70B4](P2.md#rv-server-f60c70b4) | `STILL_OPEN` | low | M/code | no | agree | crates/shamir-server/src/security/observability/logging/mod.rs:170-173 | Use a retrying ArcSwap RCU update and add a synchronized concurrent-override preservation regression. |
| [RV-SERVER-4512C85D](P2.md#rv-server-4512c85d) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-server/src/sync/connection/request_loop.rs:514 | Delete the discarded AuthFailed value and its unused import. |
| [RV-SERVER-8999F6B7](P2.md#rv-server-8999f6b7) | `STILL_OPEN` | nit | M/test | no | — | crates/shamir-server/src/sync/connection/session/wire.rs:38-99; crates/shamir-client/src/client/transport.rs:721-745 | Add shared byte-exact handshake compatibility fixtures for base and hinted positional frames. |
| [RV-SERVER-97C240D7](P2.md#rv-server-97c240d7) | `STILL_OPEN` | nit | M/decision | yes | — | crates/shamir-server/src/main.rs:48-56,414 | Choose protected-file/stdin credential input and deprecate or explicitly document plaintext argv input. |
| [RV-SERVER-CEE352CA](P2.md#rv-server-cee352ca) | `UNVERIFIABLE` | low (was unverified) | M/doc | no | agree | crates/shamir-server/src/engine/subscriptions/filter/target_match.rs:44,81-83; crates/shamir-server/src/engine/subscriptions/cache/decode_cache.rs:115-134 | Qualify complexity claims against the exact dependency implementation and support bounds with CI scaling/concurrency evidence. |
| [RV-SERVER-D5F412E2](P2.md#rv-server-d5f412e2) | `UNVERIFIABLE` | low (was unverified) | M/test | no | agree | crates/shamir-server/src/sync/replication/transport/wire_source.rs:71-74; crates/shamir-server/src/sync/replication/supervisor.rs:675,719,742-745 | Audit guard lifetimes and scc cardinality sites, correct shared-source contention rationale, and cover concurrent pulls in CI. |
| [RV-SERVER-9C77C713](P2.md#rv-server-9c77c713) | `FIXED` | nit | -/- | no | agree | crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs:12,468 | None required; production consumption uses the exported browser-binding constant. |

### shamir-storage (24)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-STORAGE-0786A4D6](P2.md#rv-storage-0786a4d6) | `STILL_OPEN` | medium | M/code | no | agree | crates/shamir-storage/src/core/types.rs:578-604 | Terminate the ascending range wrapper at the first above-end key and add a no-further-polls oracle. |
| [RV-STORAGE-0A9E3A41](P2.md#rv-storage-0a9e3a41) | `PARTIAL` | low (was nit) | M/code | yes | agree | crates/shamir-storage/src/core/error.rs:227-234; crates/shamir-storage/src/stores/storage_fjall.rs:121-124; f90cea3d (#361) | Return worker-start failures through DbResult and approve source-preserving error representations. |
| [RV-STORAGE-1A5A9374](P2.md#rv-storage-1a5a9374) | `STILL_OPEN` | low | S/doc | yes | — | crates/shamir-storage/src/stores/storage_fjall.rs:410-414,522-537 | Document per-worker ordering and explicitly decide whether cross-route submission ordering is required. |
| [RV-STORAGE-2F8C6A8C](P2.md#rv-storage-2f8c6a8c) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-storage/src/stores/storage_membuffer/core.rs:297,319-334 | Parameterize Moka cache types with THasher and construct them with build_with_hasher. |
| [RV-STORAGE-38F208C4](P2.md#rv-storage-38f208c4) | `STILL_OPEN` | low | L/decision | yes | — | crates/shamir-storage/src/stores/storage_fjall.rs:178-186; crates/shamir-types/src/types/ids/record_id.rs:44-61 | Approve the actual probabilistic uniqueness contract or implement cross-handle collision-safe insertion. |
| [RV-STORAGE-8FEF1C5A](P2.md#rv-storage-8fef1c5a) | `CHANGED` | low | S/doc | no | agree | crates/shamir-collections/src/lib.rs:21-48; crates/shamir-storage/src/stores/storage_membuffer/core.rs:141; b3cb621d (#334) | Align the older project-policy premise with binding RU8; do not seed individual storage maps. |
| [RV-STORAGE-9478B37E](P2.md#rv-storage-9478b37e) | `STILL_OPEN` | low | L/decision | yes | — | crates/shamir-storage/src/core/types.rs:647-661; crates/shamir-engine/src/repo/instance/repo_instance/catalog.rs:332-338 | Choose explicit partial-copy semantics or a rename-owned staging and recovery protocol. |
| [RV-STORAGE-9794FD53](P2.md#rv-storage-9794fd53) | `STILL_OPEN` | low | S/code | no | — | crates/shamir-storage/src/stores/storage_mirrored.rs:44-48,173-179; crates/shamir-types/src/types/ids/record_id.rs:122-137 | Use existing RecordId length validation and system classification while retaining the tag allowlist. |
| [RV-STORAGE-B7839F16](P2.md#rv-storage-b7839f16) | `STILL_OPEN` | low | M/doc | no | agree | crates/shamir-storage/src/core/types.rs:441,494-500; crates/shamir-storage/src/key_bytes/mod.rs:3-9 | Correct prefetch, supported-backend and completed KeyBytes migration documentation. |
| [RV-STORAGE-BE72410B](P2.md#rv-storage-be72410b) | `STILL_OPEN` | low | M/code | no | agree | crates/shamir-storage/src/stores/storage_fjall.rs:617; crates/shamir-storage/src/key_bytes/core.rs:167-171 | Use a bounded key diagnostic formatter and test its maximum output length. |
| [RV-STORAGE-CC5C79BA](P2.md#rv-storage-cc5c79ba) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-storage/src/stores/storage_fjall.rs:224-230,399-408 | Apply one approved backend-compatible name validator before ordinary and atomic keyspace operations. |
| [RV-STORAGE-DCBCB721](P2.md#rv-storage-dcbcb721) | `STILL_OPEN` | low | S/doc | no | — | crates/shamir-storage/src/key_bytes/mod.rs:3-9,54-56; crates/shamir-storage/src/core/types.rs:11 | Replace pre-migration narration with the live KeyBytes alias and representation obligations. |
| [RV-STORAGE-3D6F974D](P2.md#rv-storage-3d6f974d) | `PARTIAL` | nit | M/test | no | agree | crates/shamir-storage/src/tests/stores/storage_membuffer_tests/clear_race_tests.rs:32,139; crates/shamir-storage/src/tests/stores/storage_membuffer_tests/mod.rs:5 | Split clear-race fixture topics through a topical subdirectory without growing the seven-entry folder. |
| [RV-STORAGE-458CFAD8](P2.md#rv-storage-458cfad8) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-storage/src/stores/storage_fjall.rs:812,872 | Remove the stale numeric location and retain the iter_stream method reference. |
| [RV-STORAGE-6FEF6FDD](P2.md#rv-storage-6fef6fdd) | `PARTIAL` | nit | M/code | no | agree | crates/shamir-storage/src/stores/storage_membuffer/store_impl.rs:5; crates/shamir-storage/src/tests/types_tests.rs:12-13; crates/shamir-storage/src/tests/stores/storage_cached_tests/basic_tests.rs:618 | Import the canonical RecordStream alias in remaining test consumers. |
| [RV-STORAGE-88C4F2CC](P2.md#rv-storage-88c4f2cc) | `STILL_OPEN` | nit | L/decision | yes | — | crates/shamir-storage/src/core/types.rs:13-14,488,506-510,647; crates/shamir-storage/src/core/error.rs:13-26 | Approve a cohesive API-polish scope before changing aliases, bounds, copy semantics or public errors. |
| [RV-STORAGE-B26793CA](P2.md#rv-storage-b26793ca) | `STILL_OPEN` | nit | M/doc | no | — | crates/shamir-storage/src/test_seams/membuffer_clear_race_hook.rs:53-70; crates/shamir-storage/src/tests/stores/storage_membuffer_tests/clear_race_tests.rs:120-137 | Update retired-boolean narration to the current oracle and hoist ordinary body imports. |
| [RV-STORAGE-B6B3CBB6](P2.md#rv-storage-b6b3cbb6) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-storage/src/stores/storage_membuffer/store_impl.rs:519-524; crates/shamir-storage/src/stores/storage_fjall.rs:834,900 | Evaluate ownership-preserving miss handling and bounded batch-aware reservation using allocation evidence. |
| [RV-STORAGE-B7703B8B](P2.md#rv-storage-b7703b8b) | `STILL_OPEN` | nit | M/doc | no | — | crates/shamir-storage/src/stores/storage_in_memory.rs:368-372; crates/shamir-storage/src/stores/storage_fjall.rs:945 | Remove surviving empty Tests banners without changing test registration. |
| [RV-STORAGE-F26D060C](P2.md#rv-storage-f26d060c) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-storage/src/core/types.rs:554,583,648; crates/shamir-storage/src/stores/storage_fjall.rs:827 | Hoist ordinary function and closure imports to file headers and remove duplicates. |
| [RV-STORAGE-FB7C0794](P2.md#rv-storage-fb7c0794) | `STILL_OPEN` | nit | M/code | yes | — | crates/shamir-storage/src/key_bytes/core.rs:242-245 | Approve a key-size limit and enforce it with a bounded deserialization visitor before growth. |
| [RV-STORAGE-8DFF95BF](P2.md#rv-storage-8dff95bf) | `FIXED` | low | -/- | no | agree | crates/shamir-storage/src/stores/storage_cached.rs:321-345,433-448; 973d0a0 (#269) | None for the named persistent stale-refill mechanism. |
| [RV-STORAGE-AE7F22EF](P2.md#rv-storage-ae7f22ef) | `FIXED` | low | -/- | no | agree | crates/shamir-storage/src/stores/storage_cached.rs:366-448; 973d0a0 (#269) | None for the named clear-reset-refill mechanism. |
| [RV-STORAGE-7B098ADB](P2.md#rv-storage-7b098adb) | `FIXED` | nit | -/- | no | agree | crates/shamir-storage/src/stores/storage_cached.rs:143-168,275-280,412-446; 973d0a0 (#269) | None for the named insert-fill and reload counter races. |

### shamir-transport-ipc (3)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-TRANSPORT-IPC-FA481516](P2.md#rv-transport-ipc-fa481516) | `STILL_OPEN` | low | M/decision | yes | — | crates/shamir-transport-ipc/src/unix.rs:66-82,91-116; docs/guide-docs/client-server-protocol-spec/TRANSPORT_UNIX.md:57-59 | Record accepted conditional exposure and offer an owner-only socket parent as structural mitigation. |
| [RV-TRANSPORT-IPC-400D803E](P2.md#rv-transport-ipc-400d803e) | `PARTIAL` | nit | M/code | no | agree | crates/shamir-transport-ipc/src/unix.rs:150-169; crates/shamir-transport-ipc/src/windows.rs:91,160-168; d4981f5e (#360) | Add endpoint and first-instance diagnostic context while preserving OS error identity and ambiguity. |
| [RV-TRANSPORT-IPC-9B233842](P2.md#rv-transport-ipc-9b233842) | `STILL_OPEN` | nit | S/doc | no | agree | crates/shamir-transport-ipc/src/windows.rs:12-14,164 | State that remote-client rejection is explicitly pinned to true. |

### shamir-transport-tcp (15)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-TRANSPORT-TCP-17E11DF1](P2.md#rv-transport-tcp-17e11df1) | `STILL_OPEN` | low | M/doc | no | — | crates/shamir-transport-tcp/src/tls.rs:58; docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md:154 | Describe the project-specific exporter separately from RFC 9266 without changing wire bytes. |
| [RV-TRANSPORT-TCP-31C5E8A6](P2.md#rv-transport-tcp-31c5e8a6) | `CHANGED` | low | M/test | no | agree | crates/shamir-transport-tcp/src/framing.rs:52,98; 6a128476 | Measure plain-TCP syscall counts and latency in CI before changing bounded framing reads. |
| [RV-TRANSPORT-TCP-79910BA5](P2.md#rv-transport-tcp-79910ba5) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-client/src/client/api.rs:571; crates/shamir-transport-tcp/src/framing.rs:198 | Reuse per-connection zeroizing framing storage while preserving cancellation safety. |
| [RV-TRANSPORT-TCP-05C670C2](P2.md#rv-transport-tcp-05c670c2) | `PARTIAL` | nit | S/test | no | agree | crates/shamir-transport-tcp/tests/echo_e2e.rs:209 | Hoist the remaining echo-peer latency import to the file header. |
| [RV-TRANSPORT-TCP-16C543F3](P2.md#rv-transport-tcp-16c543f3) | `STILL_OPEN` | nit | M/test | no | — | crates/shamir-transport-tcp/tests/handshake_e2e.rs:46,89; crates/shamir-transport-tcp/tests/echo_e2e.rs:58,122 | Share handwritten-peer wire fixtures without replacing independent protocol oracles. |
| [RV-TRANSPORT-TCP-4D95A5F8](P2.md#rv-transport-tcp-4d95a5f8) | `STILL_OPEN` | nit | M/code | yes | — | crates/shamir-transport-tcp/src/framing.rs:80,187,230,295 | Add capped writer companions while retaining existing default-cap wrappers. |
| [RV-TRANSPORT-TCP-5CE7BA5B](P2.md#rv-transport-tcp-5ce7ba5b) | `STILL_OPEN` | nit | M/test | no | — | crates/shamir-transport-tcp/tests/handshake_e2e.rs:117,283; crates/shamir-transport-tcp/tests/echo_e2e.rs:377; crates/shamir-connect/src/client/handshake.rs:236 | Offload synchronous challenge derivation from the test runtime worker. |
| [RV-TRANSPORT-TCP-6062E565](P2.md#rv-transport-tcp-6062e565) | `STILL_OPEN` | nit | S/doc | no | agree | crates/shamir-transport-tcp/src/tls.rs:124,128,135,148 | Document the actual local exporter adapter and its supported stream implementations. |
| [RV-TRANSPORT-TCP-9C706A16](P2.md#rv-transport-tcp-9c706a16) | `STILL_OPEN` | nit | S/dep | yes | — | crates/shamir-transport-tcp/Cargo.toml:14 | Narrow Tokio library features and declare test-only runtime capabilities separately. |
| [RV-TRANSPORT-TCP-A13A3534](P2.md#rv-transport-tcp-a13a3534) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-transport-tcp/src/listener.rs:30,76 | Distinguish broader UDS policy from these TCP-only bind and validation APIs. |
| [RV-TRANSPORT-TCP-B48A0EF2](P2.md#rv-transport-tcp-b48a0ef2) | `STILL_OPEN` | nit | M/doc | no | agree | crates/shamir-transport-tcp/src/tls.rs:62,80; crates/shamir-server/src/security/tls.rs:91,115 | Document bootstrap scheduling and require spawn_blocking for active async-runtime callers. |
| [RV-TRANSPORT-TCP-CD312411](P2.md#rv-transport-tcp-cd312411) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-transport-tcp/src/tls.rs:16,18 | Use one consistent pki-types import spelling through the rustls re-export. |
| [RV-TRANSPORT-TCP-EC4F02F5](P2.md#rv-transport-tcp-ec4f02f5) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-transport-tcp/src/tls.rs:58; crates/shamir-connect/src/common/primitives/domain_tags.rs:35 | Alias the dependency exporter-label constant while preserving the public TCP name. |
| [RV-TRANSPORT-TCP-FA40B59D](P2.md#rv-transport-tcp-fa40b59d) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-transport-tcp/src/listener.rs:98,100 | Remove redundant dead_code allowances while retaining the public constants. |
| [RV-TRANSPORT-TCP-6392C63D](P2.md#rv-transport-tcp-6392c63d) | `FIXED` | nit | -/- | no | agree | crates/shamir-transport-tcp/src/tests/mod.rs:1; crates/shamir-transport-tcp/src/tests/framing_tests.rs:12; crates/shamir-transport-tcp/src/tests/tls_tests.rs:15 | None. |

### shamir-transport-ws (9)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-TRANSPORT-WS-67E355C7](P2.md#rv-transport-ws-67e355c7) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-transport-ws/src/browser.rs:22-24; crates/shamir-transport-ws/src/server.rs:223; crates/shamir-server/src/engine/server/server_launcher/accept_loops.rs:443 | Borrow the transport callback policy and share immutable policy storage across connection tasks. |
| [RV-TRANSPORT-WS-F0A54173](P2.md#rv-transport-ws-f0a54173) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-transport-ws/src/browser.rs:37-39,59-70 | Validate origin patterns and serialized origins while preserving explicit one-level wildcard semantics. |
| [RV-TRANSPORT-WS-3D98F208](P2.md#rv-transport-ws-3d98f208) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-transport-ws/src/browser.rs:28-29,99 | Remove the nonexistent accept_no_origin exception and document unconditional empty-policy rejection. |
| [RV-TRANSPORT-WS-460A02D6](P2.md#rv-transport-ws-460a02d6) | `STILL_OPEN` | nit | S/dep | yes | — | crates/shamir-transport-ws/Cargo.toml:31-35; crate-wide Rust reference search | Remove the unused hex, serde, serde_bytes and rmp-serde dev-dependencies. |
| [RV-TRANSPORT-WS-5AF6EE84](P2.md#rv-transport-ws-5af6ee84) | `STILL_OPEN` | nit | M/doc | no | agree | crates/shamir-transport-ws/src/framing.rs:53,239-242; crates/shamir-transport-ws/src/server.rs:66-79 | Document short-prefix actual-length semantics and caller ownership of pre-auth limits. |
| [RV-TRANSPORT-WS-5D3BB859](P2.md#rv-transport-ws-5d3bb859) | `STILL_OPEN` | nit | S/code | yes | — | crates/shamir-transport-ws/src/browser.rs:86,103; crates/shamir-transport-ws/src/server.rs:259-264 | Return a fixed browser origin-rejection body while preserving typed diagnostics internally. |
| [RV-TRANSPORT-WS-B1243564](P2.md#rv-transport-ws-b1243564) | `STILL_OPEN` | nit | S/code | yes | — | crates/shamir-transport-ws/src/server.rs:244-257; crates/shamir-transport-ws/src/browser.rs:99 | Classify present non-text browser origins as NotAllowed rather than Missing. |
| [RV-TRANSPORT-WS-E6697C98](P2.md#rv-transport-ws-e6697c98) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-transport-ws/src/server.rs:66-79; crates/shamir-transport-ws/src/framing.rs:254-258; crates/shamir-server/src/sync/connection/handshake.rs:778 | Separate caller-selected phase limits from accept-time tungstenite buffering ceilings in the documentation. |
| [RV-TRANSPORT-WS-F6AAB91C](P2.md#rv-transport-ws-f6aab91c) | `FIXED` | nit | -/- | no | agree | crates/shamir-transport-ws/tests/framing_round_trip.rs:3-14 | None. |

### shamir-tx (18)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-TX-D1C20D85](P2.md#rv-tx-d1c20d85) | `STILL_OPEN` | medium | M/code | no | agree | crates/shamir-tx/src/mvcc_store/mvcc_locks.rs:76-83,216-235 | Implement registry-coordinated retirement covering holders, requesters, waiters and cancellation. |
| [RV-TX-21B69CC7](P2.md#rv-tx-21b69cc7) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs:905-908; crates/shamir-tx/src/mvcc_store/history/version_entry.rs:648-654 | Batch or bounded-concurrently resolve fallback slots while retaining race semantics and input order. |
| [RV-TX-3B9F4D3B](P2.md#rv-tx-3b9f4d3b) | `PARTIAL` | low | M/code | no | agree | crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs:215-230 | Batch archival timestamp misses while preserving pending stamps, ordering and unknown-age semantics. |
| [RV-TX-4A1FDAAE](P2.md#rv-tx-4a1fdaae) | `STILL_OPEN` | low | L/decision | yes | — | crates/shamir-tx/src/changefeed/feed/events.rs:193-205 | Choose shared in-memory table-name storage while preserving serialized string values. |
| [RV-TX-4D85CE16](P2.md#rv-tx-4d85ce16) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-tx/src/version/versioned_overlay.rs:193-213 | Add version-oriented reclamation indexing with consistent mutation and size accounting. |
| [RV-TX-56651C4A](P2.md#rv-tx-56651c4a) | `STILL_OPEN` | low | L/code | yes | agree | crates/shamir-tx/src/changefeed/feed/core.rs:611-626,683,743 | Add ordered journal batch persistence without crossing gap or barrier boundaries. |
| [RV-TX-782352E1](P2.md#rv-tx-782352e1) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-tx/src/mvcc_store/store.rs:514-515,567-598; crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs:892 | Reject reserved empty payloads before version allocation while preserving an explicit deletion path. |
| [RV-TX-7BB0D16C](P2.md#rv-tx-7bb0d16c) | `STILL_OPEN` | low | L/code | no | — | crates/shamir-tx/src/gate/repo_tx_gate.rs:895-909; crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs:278-285 | Introduce an epoch-qualified opener handoff with a deterministic GC-progress oracle. |
| [RV-TX-D70EC152](P2.md#rv-tx-d70ec152) | `STILL_OPEN` | low | M/code | no | agree | crates/shamir-tx/src/tx/support/id_remap.rs:79-86; crates/shamir-tx/src/tx/tx_context.rs:981 | Track recursive remap changes and retain original bytes for unchanged rows. |
| [RV-TX-ED8562B0](P2.md#rv-tx-ed8562b0) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-tx/src/tx/repo_wal_manager.rs:30-32,53-55; crates/shamir-tx/src/gate/repo_tx_gate.rs:364-370 | Reject or normalize zero seeds consistently in both allocators and pin external-zero semantics. |
| [RV-TX-3FC2FA1D](P2.md#rv-tx-3fc2fa1d) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-tx/src/mvcc_store/history/mvcc_history.rs:919-929 | Diagnose non-absence storage failures while retaining conservative unknown-age behaviour. |
| [RV-TX-43CD8A27](P2.md#rv-tx-43cd8a27) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-tx/src/tx/support/layered_interner.rs:91; crates/shamir-tx/src/mvcc_store/store.rs:373-374; crates/shamir-tx/src/tx/tx_context.rs:570,728 | Hoist non-exempt production imports to file headers. |
| [RV-TX-5508B0C1](P2.md#rv-tx-5508b0c1) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-tx/src/changefeed/feed/core.rs:120-126,178-194 | Document the entered-runtime requirement, panic behaviour and intentional detached lifecycle. |
| [RV-TX-C935EAB9](P2.md#rv-tx-c935eab9) | `STILL_OPEN` | nit | M/test | no | — | crates/shamir-tx/src/lib.rs:33-34; crates/shamir-tx/src/tests/mod.rs:1-6 | Move implementation suites to owning module tests directories while preserving discovery and layout limits. |
| [RV-TX-D2989766](P2.md#rv-tx-d2989766) | `STILL_OPEN` | nit | M/code | no | — | crates/shamir-tx/src/mvcc_store/store.rs:714,735-736,850 | Borrow item keys in compensation and vacuum helpers instead of allocating duplicate key vectors. |
| [RV-TX-FB4FD0FE](P2.md#rv-tx-fb4fd0fe) | `PARTIAL` | nit | M/test | no | agree | crates/shamir-tx/src/tests/version_tests/version_guard_queued_tests.rs:53-55,79,105-106; crates/shamir-tx/src/tx/support/metrics.rs:34-94 | Add registered local coverage for defaults, independent increments, GC totals and snapshots. |
| [XXS-NEW-TX-PREFIX](P2.md#xxs-tx-prefix-vacuum) | `FIXED` | high | -/- | no | agree | crates/shamir-tx/src/mvcc_store/history/mvcc_gc.rs:209-215; 199331cb (#329) | None; exact decoded-key filtering removes neighbouring-key selection. |
| [RV-TX-99487168](P2.md#rv-tx-99487168) | `FIXED` | nit | -/- | no | agree | crates/shamir-collections/src/lib.rs:23-47; b3cb621d (#334) | None; RU8 explicitly documents client-controlled inputs, bounds and the reopen trigger. |

### shamir-types (17)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-TYPES-973FEEFD](P2.md#rv-types-973feefd) | `STILL_OPEN` | medium | L/code | no | agree | crates/shamir-types/src/core/interner/interner.rs:149-154; crates/shamir-engine/src/table/support/storage/interner_manager.rs:248-255,292 | Validate hydrated ID/name bijection fallibly, allow identical recaptures, reject conflicts, and propagate corruption errors. |
| [RV-TYPES-5E4BFC76](P2.md#rv-types-5e4bfc76) | `STILL_OPEN` | low | L/decision | yes | agree | crates/shamir-types/src/codecs/basic/bincode.rs:41-45; crates/shamir-engine/src/table/support/storage/interner_manager.rs:248 | Define trusted-type/input constraints and require explicitly bounded decoding for recursive untrusted roots. |
| [RV-TYPES-71AFA12F](P2.md#rv-types-71afa12f) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-types/src/entries/access/resource_path.rs:198-218; crates/shamir-types/src/entries/access/policy.rs:77-78; crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs:73-78,103-106 | Escape controls at trace and diagnostic boundaries without changing resource identity or URI Display. |
| [RV-TYPES-A7B1FCEC](P2.md#rv-types-a7b1fcec) | `STILL_OPEN` | low | L/code | yes | — | crates/shamir-types/src/entries/access/resource_path.rs:155-190; crates/shamir-db/src/shamir_db/shamir_db/access/authorize.rs:56-67 | Introduce borrowed ancestor traversal and compatible metadata lookup while preserving nearest-first awaited authorization. |
| [RV-TYPES-E82C6BE1](P2.md#rv-types-e82c6be1) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-types/src/codecs/interned/messagepack.rs:657-679 | Serialize replacement and new-entry payloads directly into the output buffer, preserving byte parity. |
| [RV-TYPES-EFA1B8F4](P2.md#rv-types-efa1b8f4) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-types/src/macros/mpack.rs:239-260; crates/shamir-types/src/macros/tests/mpack_tests.rs:33,56 | Add negative-literal object arms and CI cases with and without trailing commas while retaining explicit expression escapes. |
| [RV-TYPES-F070E432](P2.md#rv-types-f070e432) | `PARTIAL` | low | M/code | no | agree | crates/shamir-types/src/codecs/interned/messagepack.rs:133-152 | Use checked range arithmetic in read_bin and add a malformed maximum-length Bin32 CI oracle on 32-bit. |
| [RV-TYPES-FE40CB54](P2.md#rv-types-fe40cb54) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-types/src/entries/secret.rs:21,67-73 | Use safe String zeroization, settle public comparison semantics, and add lifecycle and feature CI coverage. |
| [RV-TYPES-FEB0B5E4](P2.md#rv-types-feb0b5e4) | `STILL_OPEN` | low | S/doc | no | — | crates/shamir-types/src/types/ids/record_id.rs:41-60,64-72,94-102 | Document timestamp disclosure and non-secret identifier semantics, explicitly prohibiting capability-by-possession use. |
| [RV-TYPES-178ED8CB](P2.md#rv-types-178ed8cb) | `STILL_OPEN` | nit | S/test | no | — | crates/shamir-types/src/codecs/interned/tests/bytes/merge_storage_bytes_tests.rs:296,318 | Hoist the shared RecordView import to the file header and remove function-local duplicates. |
| [RV-TYPES-2A360D16](P2.md#rv-types-2a360d16) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-types/src/core/README.md:64-67,76,84; crates/shamir-types/src/core/interner/interner.rs:84-93 | Update interner layout, return types, and locking descriptions to match ArcSwap reverse reads and serialized publication. |
| [RV-TYPES-313627EB](P2.md#rv-types-313627eb) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-types/src/types/value.rs:666,678-682,691,703 | Track traversed byte extent and allocate the error prefix only when returning NotAMap. |
| [RV-TYPES-8AC2CFC5](P2.md#rv-types-8ac2cfc5) | `STILL_OPEN` | nit | S/test | no | — | crates/shamir-types/src/types/tests/record_id_tests.rs:159-163,185-194 | Compare timestamp prefixes against independently calculated expected bytes in the uniqueness test. |
| [RV-TYPES-A31DDF76](P2.md#rv-types-a31ddf76) | `STILL_OPEN` | nit | S/code | no | agree | crates/shamir-types/src/core/sort_codec.rs:152-154 | Remove the empty trailing Tests banner. |
| [RV-TYPES-F9A7C76C](P2.md#rv-types-f9a7c76c) | `FIXED` | low | -/- | no | agree | crates/shamir-types/src/core/interner/interner.rs:270-301; f90cea3d (#361) | None. |
| [RV-TYPES-47A3CB10](P2.md#rv-types-47a3cb10) | `FIXED` | nit | -/- | no | agree | crates/shamir-types/src/macros/mpack.rs:286-290 | None. |
| [RV-TYPES-FC6867C2](P2.md#rv-types-fc6867c2) | `FIXED` | nit | -/- | no | agree | crates/shamir-types/src/types/value.rs:19-28 | None. |

### shamir-wal (16)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-WAL-59E3DBA1](P2.md#rv-wal-59e3dba1) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-wal/src/wal_group_commit/wal_group_commit.rs:218,226,300 | Bound leader tenure with safe follower re-election and sustained-arrival completion coverage. |
| [RV-WAL-67475C86](P2.md#rv-wal-67475c86) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-wal/src/segment_set/segment_set.rs:548,589 | Retain retryable deletion claims and reconcile per-file outcomes with the sealed registry. |
| [RV-WAL-79483DA7](P2.md#rv-wal-79483da7) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-wal/src/segment/wal_segment.rs:299,318,345,373; crates/shamir-wal/benches/segment_set_lock.rs:112 | Serialize offset capture, write, rollback and accounting; add concurrent failure and replay coverage. |
| [RV-WAL-8021EB80](P2.md#rv-wal-8021eb80) | `PARTIAL` | low | M/code | no | agree | crates/shamir-wal/src/segment/wal_sink.rs:308,417; crates/shamir-wal/src/segment_set/segment_set.rs:530,549 | Preserve unversioned-record pins across batching, sealing and reopen with mixed-version Mem/File tests. |
| [RV-WAL-A25EDE9D](P2.md#rv-wal-a25ede9d) | `STILL_OPEN` | low | L/code | yes | — | crates/shamir-wal/src/segment_set/segment_set.rs:518; crates/shamir-engine/src/tx/drain/drainer/drain_step.rs:162 | Introduce bounded incremental replay and migrate startup and live gap recovery consumers. |
| [RV-WAL-CC3131A9](P2.md#rv-wal-cc3131a9) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-wal/src/segment/wal_segment.rs:506,726; crates/shamir-wal/src/entry/segment_meta.rs:135 | Bound sidecar reads and parse segment frames incrementally with allocation limits. |
| [RV-WAL-DD492F5C](P2.md#rv-wal-dd492f5c) | `STILL_OPEN` | low | M/doc | yes | — | crates/shamir-wal/src/segment/wal_segment.rs:313,759; crates/shamir-wal/src/entry/segment_meta.rs:153; docs/guide-docs/security/data-protection.md:27 | Document trusted directory writers, required ACLs and the absence of WAL authenticity guarantees. |
| [RV-WAL-0843B96B](P2.md#rv-wal-0843b96b) | `STILL_OPEN` | nit | M/code | yes | — | crates/shamir-wal/src/lib.rs:53; crates/shamir-wal/src/entry/segment_meta.rs:11,125,143 | Remove the empty public segment_meta namespace and align documentation visibility. |
| [RV-WAL-0CAFAF79](P2.md#rv-wal-0cafaf79) | `STILL_OPEN` | nit | S/code | no | agree | crates/shamir-wal/src/segment/wal_segment.rs:147,186 | Remove broad dead_code allowances and retain only specifically justified exceptions if necessary. |
| [RV-WAL-3735F642](P2.md#rv-wal-3735f642) | `PARTIAL` | nit | M/code | no | agree | crates/shamir-wal/src/segment/wal_sink.rs:391,415,438 | Use immutable snapshots and move corpus processing outside runtime-worker critical sections. |
| [RV-WAL-3F10E8A1](P2.md#rv-wal-3f10e8a1) | `STILL_OPEN` | nit | M/decision | yes | — | crates/shamir-wal/src/segment/wal_segment.rs:212,709 | Define operator diagnostic versus public-error policy and redact externally surfaced paths. |
| [RV-WAL-5DFE355F](P2.md#rv-wal-5dfe355f) | `STILL_OPEN` | nit | M/decision | yes | agree | crates/shamir-wal/src/segment/wal_segment.rs:459,462 | Gate or restrict mark_poisoned after consumer review, or explicitly support it as an administrative API. |
| [RV-WAL-C37B110A](P2.md#rv-wal-c37b110a) | `STILL_OPEN` | nit (was low) | S/doc | no | — | crates/shamir-wal/src/segment/wal_segment.rs:234,251 | Explicitly define returned sequence numbers as nonpersistent per-handle ordinals. |
| [RV-WAL-C66039FC](P2.md#rv-wal-c66039fc) | `STILL_OPEN` | nit | S/test | no | — | crates/shamir-wal/src/tests/wal_group_commit_tests.rs:225,256,273,442 | Hoist Duration imports into the test file header. |
| [RV-WAL-FE36ADFC](P2.md#rv-wal-fe36adfc) | `STILL_OPEN` | nit (was low) | S/doc | no | — | crates/shamir-wal/src/entry/wal_entry_v2.rs:80; crates/shamir-engine/src/tx/recovery/v2.rs:183 | Document the existing reserved and ignored idx_id behavior without changing serialization. |
| [RV-WAL-4BD5E020](P2.md#rv-wal-4bd5e020) | `FIXED` | low | -/- | no | agree | crates/shamir-wal/src/segment/wal_segment.rs:515,742; e27aa271 | None. |

### shamir-wasm-host (18)

| Card | Verdict | Risk | Size/kind | Owner | Pass 2 | Evidence | Recipe |
|---|---|---|---|---|---|---|---|
| [RV-WASM-HOST-C17D12C8](P2.md#rv-wasm-host-c17d12c8) | `STILL_OPEN` | medium | M/decision | yes | agree | crates/shamir-wasm-host/src/compile.rs:641-645,667 | Define a deployment-supported SDK source and configurable runtime resolution with actionable errors. |
| [RV-WASM-HOST-203F2147](P2.md#rv-wasm-host-203f2147) | `STILL_OPEN` | low | M/code | no | — | crates/shamir-wasm-host/src/wasm/bindings/call.rs:96,161; crates/shamir-wasm-host/src/runtime/context.rs:660-661 | Add a crate-private shared-grants setter and reuse the existing immutable Arc. |
| [RV-WASM-HOST-4669FDA6](P2.md#rv-wasm-host-4669fda6) | `STILL_OPEN` | low | L/code | yes | — | crates/shamir-wasm-host/src/builtin/meta.rs:113-141; crates/shamir-db/src/shamir_db/shamir_db/core.rs:484 | Introduce checked version-aware metadata decoding with explicit malformed-field errors and legacy defaults. |
| [RV-WASM-HOST-796ECAE4](P2.md#rv-wasm-host-796ecae4) | `STILL_OPEN` | low | S/code | no | — | crates/shamir-wasm-host/src/compile.rs:701-703 | Pass Cargo paths as native Path or OsStr arguments instead of Unicode strings. |
| [RV-WASM-HOST-98551A44](P2.md#rv-wasm-host-98551a44) | `STILL_OPEN` | low | M/decision | yes | — | crates/shamir-wasm-host/src/wasm/wasm_engine.rs:204; crates/shamir-wasm-host/src/wasm/wasm_function.rs:303-316,526-528 | Define an engine memory ceiling and reconcile accepted function limits with pooling capacity. |
| [RV-WASM-HOST-C5833F4E](P2.md#rv-wasm-host-c5833f4e) | `STILL_OPEN` | low | S/code | no | agree | crates/shamir-wasm-host/src/wasm/wasm_function.rs:471-474 | Retain encoded bytes directly and encode a borrowed parameter map without cloning its tree. |
| [RV-WASM-HOST-EB1654FA](P2.md#rv-wasm-host-eb1654fa) | `STILL_OPEN` | low | M/code | yes | — | crates/shamir-wasm-host/src/compile.rs:718,723-726; crates/shamir-proc-tree/src/supervise.rs:68-77 | Drain both pipes continuously while retaining capped diagnostics with explicit truncation markers. |
| [RV-WASM-HOST-FE02BB1D](P2.md#rv-wasm-host-fe02bb1d) | `STILL_OPEN` | low | S/code | yes | — | crates/shamir-wasm-host/src/builtin/argon2id.rs:78-80 | Distinguish blocking-task panic from cancellation and preserve a diagnostic panic cause. |
| [RV-WASM-HOST-741D48B5](P2.md#rv-wasm-host-741d48b5) | `STILL_OPEN` | nit | S/doc | no | agree | crates/shamir-wasm-host/src/wasm/mod.rs:3-5,16 | Include verify_wasm_module and SANCTIONED_HOST_IMPORTS in the public-surface documentation list. |
| [RV-WASM-HOST-8FA4CADD](P2.md#rv-wasm-host-8fa4cadd) | `STILL_OPEN` | nit | S/code | no | — | crates/shamir-wasm-host/src/compile.rs:744,754,761,794,798 | Pass the owned artifact Vec into maybe_wasm_opt and return it directly on fallback. |
| [RV-WASM-HOST-92EA3A88](P2.md#rv-wasm-host-92ea3a88) | `PARTIAL` | nit | S/code | no | agree | crates/shamir-wasm-host/src/compile.rs:715-719; crates/shamir-wasm-host/src/tests/runtime/compile_tests.rs:112,129,141,154,189,553 | Hoist the remaining test-body imports into the file header. |
| [RV-WASM-HOST-C43E9092](P2.md#rv-wasm-host-c43e9092) | `STILL_OPEN` | nit | S/doc | no | — | crates/shamir-wasm-host/src/gateway/net_gateway.rs:148,161,174,267 | Remove internal finding labels while retaining the substantive DNS-pinning explanation. |
| [RV-WASM-HOST-EBD32CA5](P2.md#rv-wasm-host-ebd32ca5) | `STILL_OPEN` | nit | S/dep | yes | — | crates/shamir-wasm-host/Cargo.toml:15; crates/shamir-wasm-host/src/compile.rs:61,574; crate-wide Rust reference search | Remove the unused direct serde dependency. |
| [RV-WASM-HOST-FF7C2D6E](P2.md#rv-wasm-host-ff7c2d6e) | `PARTIAL` | nit | M/code | yes | agree | crates/shamir-wasm-host/src/wasm/wasm_engine.rs:164-171; crates/shamir-wasm-host/src/gateway/net_gateway.rs:20,107; crates/shamir-wasm-host/src/compile.rs:701-703; crates/shamir-wasm-host/src/builtin/meta.rs:124-141 | Complete the remaining path and metadata components through cards 796ECAE4 and 4669FDA6. |
| [RV-WASM-HOST-AAAE2B68](P2.md#rv-wasm-host-aaae2b68) | `FIXED` | low | -/- | no | agree | crates/shamir-wasm-host/src/compile.rs:334-367; be062ea0 (#271) | None. |
| [RV-WASM-HOST-D808C5EB](P2.md#rv-wasm-host-d808c5eb) | `FIXED` | low | -/- | no | agree | crates/shamir-wasm-host/src/wasm/wasm_function.rs:756-770; be062ea0 (#271) | None. |
| [RV-WASM-HOST-8062B59A](P2.md#rv-wasm-host-8062b59a) | `FIXED` | nit | -/- | no | agree | crates/shamir-wasm-host/src/wasm/bindings/batch.rs:45-46; crates/shamir-wasm-host/src/wasm/bindings/globals.rs:101-102; 70f8ee60 (#343) | None. |
| [RV-WASM-HOST-B3A8300E](P2.md#rv-wasm-host-b3a8300e) | `FIXED` | nit | -/- | no | agree | crates/shamir-wasm-host/src/wasm/bindings/call.rs:30-49 | None. |

## Notes and limits

- The title of `RV-STORAGE-0786A4D6` ("scans past the upper bound forever") overstates the mechanism: the default range filter keeps polling the underlying stream after the first key above the end, so a finite store is scanned to its end; "forever" holds only for an unbounded source. The finding itself stands.
- `UNVERIFIABLE` is a statement about static evidence, not about the code: those cards need a measurement, a published artifact or a crate-wide absence proof.
- Pass 2 saw the pass-1 claim in its prompt (it was instructed to decide from the code first); full agreement on 135 cards can still share a blind spot. The 204 `low`/`nit` verdicts outside the sample rest on one pass plus the 0/40 calibration result.
- Overlapping umbrella cards (for example `RV-FUNCLIB-37BE26F9`) must not be counted as separate runtime defects; the per-crate counts are cards, not unique bugs.
- This document does not schedule any work. Planning the fix waves is a separate step that waits for the owner.
