<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-db — api-wire-protocol independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Validator identity/binding destruction and silent isolation downgrade are concrete defects. The legacy shim and error-conversion sites retain hygiene debt; privileged wrappers and optional error fields are documented contracts.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 12 | 10 | 0 | 0 | 0 | 0 | 2 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — `replace=true` on a WASM validator destroys persisted binding bookkeeping and can silently re-key its identity

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Replace saves bound_in=[] and removes the live entry, erasing its reverse binding set. Drop then succeeds despite persisted table-side bindings; those writes fail closed after removal. The discriminating regression must replace an already-bound validator, preserve its ID/bookkeeping, and still refuse drop.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:249](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L249); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:284](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L284); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:305](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L305); [crates/shamir-engine/src/validator/registry.rs:156](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/validator/registry.rs#L156); [crates/shamir-db/tests/validators_e2e.rs:488](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/tests/validators_e2e.rs#L488).

<a id="review-2"></a>

### Claim 2 — Dead, exported `api::{Command, Request, Response}` wire shim that no server speaks

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The DTOs remain public with client/server prose, but their README identifies them as vestigial and no workspace transport consumes them. No active protocol incompatibility is demonstrated. Removing them changes an external Rust API.

Evidence: [crates/shamir-db/src/lib.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/lib.rs#L26); [crates/shamir-db/src/api/types.rs:24](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/api/types.rs#L24); [crates/shamir-db/src/api/README.md:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/api/README.md#L17).

<a id="review-3"></a>

### Claim 3 — Convenience `execute` / `tx_begin` / `tx_execute` / `tx_commit` default to `Actor::System` (admin bypass)

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

These methods intentionally delegate with System and actor-aware counterparts exist. The embedded trust model accepts privileged direct APIs; inspected server paths call *_as. Better warnings are optional, not a source-proven authentication defect.

Evidence: [crates/shamir-db/src/shamir_db/execute/db_execute.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_execute.rs#L19); [crates/shamir-db/src/shamir_db/execute/db_tx.rs:41](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_tx.rs#L41); [crates/shamir-db/src/shamir_db/execute/db_tx.rs:106](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_tx.rs#L106); [crates/shamir-db/src/shamir_db/execute/db_tx.rs:159](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_tx.rs#L159); [docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md:111](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/SECURITY_MODEL.md#L111).

<a id="review-4"></a>

### Claim 4 — Builder-only query-construction rule bypassed across the facade (30+ hand-assembled wire ops, no exception comments; builder is dev-dep only)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

SystemStore, replication and gateway construct typed DTOs while the builder is dev-only and lacks the requested exception explanation. This is convention debt; no malformed request is established. Moving construction must preserve explicit defaults, field paths and batch semantics.

Evidence: [CLAUDE.md:516](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L516); [crates/shamir-db/src/shamir_db/system_store.rs:199](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/system_store.rs#L199); [crates/shamir-db/src/shamir_db/execute/admin_replication.rs:87](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_replication.rs#L87); [crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs#L130); [crates/shamir-db/Cargo.toml:102](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/Cargo.toml#L102).

<a id="review-5"></a>

### Claim 5 — Wire error-`code` contract populated unevenly across handler families; `TransactionInfo::aborted` reason mixes stable codes with free text

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The observed fields conform to their documented types: code is optional and reason is human-readable. Specific promised codes remain testable, but a universally closed vocabulary is a proposed API redesign.

Evidence: [crates/shamir-query-types/src/batch/batch_error.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/batch_error.rs#L36); [crates/shamir-query-types/src/batch/transaction_info.rs:14](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/transaction_info.rs#L14); [crates/shamir-db/src/shamir_db/execute/db_tx.rs:217](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_tx.rs#L217).

<a id="review-6"></a>

### Claim 6 — `tx_begin` accepts any isolation string and silently falls back to Snapshot

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Only exact serializable selects Serializable; a typo succeeds as Snapshot and TxOpened echoes the typo. The oracle must inspect selected isolation or reject invalid input, not merely assert BEGIN succeeded.

Evidence: [crates/shamir-db/src/shamir_db/execute/db_tx.rs:80](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_tx.rs#L80); [crates/shamir-server/src/db_handler/tx_handlers.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tx_handlers.rs#L26); [crates/shamir-server/src/db_handler/tx_handlers.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tx_handlers.rs#L64).

<a id="review-7"></a>

### Claim 7 — `to_qv` converts serialization failure into `QueryValue::Null` inside Ok responses

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The generic helper discards either Result error; ChangesSince applies it to each event. Exact rmp-serde 1.3.1 archive source confirms the fallible conversion path. A currently failing event was not shown, so projection corruption remains a hypothetical consequence.

Evidence: [crates/shamir-db/src/shamir_db/execute/helpers.rs:73](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/helpers.rs#L73); [crates/shamir-db/src/shamir_db/execute/admin_retention.rs:206](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_retention.rs#L206); [Cargo.lock:2949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2949).

<a id="review-8"></a>

### Claim 8 — Dead TLS/network dependencies and stale `net` doc kept "so the obsolete code doesn't bit-rot" — but the code is gone

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The direct declarations and stale lib documentation remain; inspected facade source has no corresponding module or consumers. Dependency cleanup does not automatically remove shared lockfile packages.

Evidence: [crates/shamir-db/Cargo.toml:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/Cargo.toml#L64); [crates/shamir-db/src/lib.rs:8](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/lib.rs#L8).

Grouping/duplicate: [security-crypto.md#5](security-crypto.md#review-5). This is not an additional independent defect.

<a id="review-9"></a>

### Claim 9 — Malformed client input mapped to `DbError::Internal` in `get_ddl_op_status`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

RecordId parse failure and every underlying table-resolution error are wrapped as Internal. Parse failure should remain distinguishable from missing tables and real storage failure; converting every resolution error to NotFound is also wrong.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/core.rs:764](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L764); [crates/shamir-db/src/shamir_db/shamir_db/core.rs:772](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/core.rs#L772).

Grouping/duplicate: [error-handling-lifecycle.md#10](error-handling-lifecycle.md#review-10). This is not an additional independent defect.

<a id="review-10"></a>

### Claim 10 — Catalogue `wasm_hash` and `version` fields are dead, and the hash is not integrity-grade

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

WASM rows write noncryptographic FxHash metadata without verification; function replacement writes version 1. Validators write no version and native rows omit WASM hashes. Neither field currently establishes an integrity/version promise.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:187](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L187); [crates/shamir-db/src/shamir_db/shamir_db/function_management.rs:205](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/function_management.rs#L205); [crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:267](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs#L267); [Cargo.lock:3007](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3007).

<a id="review-11"></a>

### Claim 11 — `create_db_as` reports success even when the catalogue write fails

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

create_db_as returns DbInstance after warn-only save failure; the handler reports created=true. A pre-commit failure can leave no catalogue record; a post-commit flush/persist error may instead leave committed state, so the outcome must distinguish those cases.

Evidence: [crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:58](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_management.rs#L58); [crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_db_repo.rs#L67).

Grouping/duplicate: [error-handling-lifecycle.md#1](error-handling-lifecycle.md#review-1). This is not an additional independent defect.

<a id="review-12"></a>

### Claim 12 — Nits

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

All four sites remain. Derived Debug contains QueryError.code text but loses structured identity; Display alone does not guarantee a code-preserving string contract. The codec-demo binary is real, not the production server.

Evidence: [crates/shamir-db/src/shamir_db/execute/helpers.rs:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/helpers.rs#L61); [crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs:88](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_gateway.rs#L88); [crates/shamir-db/src/main.rs:7](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/main.rs#L7); [crates/shamir-db/src/shamir_db/ports.rs:6](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/ports.rs#L6).

## Evidence and recipe corrections

- System convenience methods and optional error metadata are deliberate contracts, not currently violated authentication or wire-schema guarantees.
- Native validator replacement is only an identity-fallback reference; it also destroys binding bookkeeping.
- A safe replacement recipe must handle a catalogue-only ID when replace_artifact returns false, preserve existing ownership unless explicitly changed, and reconcile table-side bindings.
- Malformed status input can map to Validation, but preserve the actual table-resolution error rather than manufacturing NotFound.
- Metadata removal or introducing digest/version enforcement needs a compatibility decision; no current verified integrity control has failed.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-db -- API & wire-protocol design

## Summary

The crate's real wire surface (`ShamirDb::execute`/`tx_*` over `BatchRequest`/`BatchOp`) is mature: typed DTOs, e2e serde round-trip and error-code suites that build through `shamir-query-builder`, catalog fields with documented stable-string contracts (`ArtifactKind`, `SCHEMA_*_FIELD`), and exemplary API-honesty fixes (F-43's loud rejection of `CreateRepo.path`). The findings below are mostly contract-shape issues at the edges: a dead, exported, never-implemented `api::Request/Response` wire shim; a replace path that silently destroys a validator's persisted binding bookkeeping (re-enabling an unsafe drop); the convenience `execute`/`tx_*` entry points defaulting to `Actor::System`; systematic hand-assembly of query/wire ops that bypasses the project's builder-only rule without the mandated exception comments; and an error-`code` contract populated in some handler families but not others.

## Findings

### 1. `replace=true` on a WASM validator destroys persisted binding bookkeeping and can silently re-key its identity
- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/validator_management.rs:248-252` (id fallback), `:282-285` (`bound_in` reset), `:302-313` (remove+register instead of `replace_artifact`)
- **Severity:** high
- **Issue:** `create_validator_inner` with `replace=true` (a) rebuilds the catalogue row from scratch with `bound_in: []`, clobbering the old record's persisted binding list (unlike `rename_validator_as`, which preserves the record), and (b) routes through `validators.remove(&old_id)` + `register(...)`, and `ValidatorRegistry::remove` (`crates/shamir-engine/src/validator/registry.rs:143`) wipes the live `bound_in` set; `register` does not restore it. The registry even provides `replace_artifact(id, ...)` ("preserving its name and table bindings", registry.rs:93) for exactly this purpose, but the facade does not use it. Additionally, when the registry has already lost the name (e.g. boot skipped a row after a compile failure), `id_for_name(name).unwrap_or_default()` (line 249) mints `RecordId::default()` — the native path (lines 89-107) explicitly falls back to the catalogue `_id` to prevent this, the WASM path does not.
- **Failure scenario:** operator replaces a table-bound validator → `bound_in` is emptied in both the registry and the catalogue → `drop_validator`'s `is_bound` guard now passes → the validator is dropped and its catalogue row removed while tables still carry `ValidatorBinding { validator_id }` on their info-twins → after restart (and even before it) every write to those tables fails closed ("Missing"), with no live validator to rebind. In the `unwrap_or_default()` variant the replacement is additionally registered under a different id, so the surviving table bindings point at a validator that no longer exists under the old id.
- **Suggested fix:** on `replace=true`, reuse `ValidatorRegistry::replace_artifact` for the same id; carry the old record's `bound_in` (and `_id`) into the new catalogue row exactly as the native path and `rename_validator_as` do; mirror the native path's catalogue `_id` fallback instead of `RecordId::default()`. Add a regression test for "replace a bound validator, then attempt drop".

### 2. Dead, exported `api::{Command, Request, Response}` wire shim that no server speaks
- **File:line:** `crates/shamir-db/src/api/types.rs:7-41` (types), `src/api/mod.rs:3`, `src/lib.rs:26` (export)
- **Severity:** high
- **Issue:** the crate exports a plausible-looking client/server envelope (`Request { request_id, command }`, `Response { request_id, result: Result<Option<UserValue>, String> }`, `Command::{Put,Get,Del,Execute}`) that has zero consumers anywhere in the workspace — the only match for `Command::` / `api::Request` outside the module is its own test (`src/api/tests/api_tests.rs`). It is not the real protocol (`BatchRequest`/`BatchOp`/`DbRequest`), carries no version field, flattens errors to a bare `String`, and uses an externally-tagged msgpack enum whose shapes match nothing on the wire today.
- **Failure scenario:** an SDK/FFI author greps the facade for a request/response type, adopts `shamir_db::api::Request`, and ships a client that no `shamir-server` build will ever answer — or, worse, a future contributor "completes" this protocol in parallel to the real one, splitting the wire format.
- **Suggested fix:** delete `src/api/` (its round-trip test adds no value over `tests/ddl_wire_e2e/serde_roundtrip.rs`), or at minimum mark the module `#[doc(hidden)]` + `#[deprecated]` with a pointer to `shamir-query-builder` before the 0.1.0 window closes.

### 3. Convenience `execute` / `tx_begin` / `tx_execute` / `tx_commit` default to `Actor::System` (admin bypass)
- **File:line:** `crates/shamir-db/src/shamir_db/execute/db_execute.rs:16-22`; `src/shamir_db/execute/db_tx.rs:31-45, 102-110, 204-212`
- **Severity:** medium
- **Issue:** the `_as`-suffixed variants take the authenticated actor; the bare public variants stamp `Actor::System`, which bypasses every ACL check (`authorize_access` short-circuits for System). Task #606 already recognized this class of footgun and applied `#[doc(hidden)]` + a SAFETY comment to `create_db`/`add_repo` (`db_management.rs:10-33, 325-346`), but `execute`/`tx_*` — far more attractive to an embedder — remain first-class documented public methods whose one-line doc only says "for backward compatibility".
- **Failure scenario:** an embedding application picks the obvious `db.execute("prod", &batch)` and silently gets superuser semantics for every op in the batch; the bug is invisible because everything succeeds.
- **Suggested fix:** apply the #606 treatment (`#[doc(hidden)]` + wire-reachability SAFETY comment) to the System-actor convenience wrappers, or deprecate them in favor of `*_as` now that all internal callers go through `execute_as`.

### 4. Builder-only query-construction rule bypassed across the facade (30+ hand-assembled wire ops, no exception comments; builder is dev-dep only)
- **File:line:** `src/shamir_db/system_store.rs` (20 struct-literal sites: lines 199, 212, 270, 293, 400, 420, 473, 551, 566, 660, 789, 902, 926, 964, 990, 1005, 1058, 1085, 1100, 1157); `src/shamir_db/execute/admin_replication.rs` (8 sites: 87, 125, 168, 206, 278, 316, 393, 596); `src/shamir_db/shamir_db/db_gateway.rs` (107-142, 168-197, 230-265 — `ReadQuery`/`BatchRequest`/`InsertOp`/`Filter` literals); `Cargo.toml:102` (`shamir-query-builder` under `[dev-dependencies]` only)
- **Severity:** medium
- **Issue:** CLAUDE.md's "Query construction — builder only" section says queries/filters/wire ops are *always* built through `shamir-query-builder` in engine/server code, and that where the builder genuinely does not apply a one-line "why" comment is mandatory. The facade hand-assembles `SetOp`/`DeleteOp`/`Filter::Eq`/`ReadQuery`/`InsertOp`/`BatchRequest` struct literals at ~31 production sites with no such comments anywhere, and the crate does not even depend on the builder outside tests, so compliance is currently impossible without a dependency change. Credit: construction is fully typed (no raw `serde_json`/`json!` anywhere in `src/` — that letter of the rule is honored), and struct literals fail to compile when a field is added; the cost is convention drift and duplicated 15-field literals, not corruption.
- **Failure scenario:** `BatchRequest`/`ReadQuery` grow semantics beyond their fields (defaults change, new invariants like "id must be unique", limits validation) and hand-built call sites silently diverge from what every builder-produced request guarantees; the "why no builder" rationale is unrecorded so the next reviewer cannot distinguish an oversight from a decision.
- **Suggested fix:** either promote `shamir-query-builder` to a real dependency and route `db_gateway.rs` (the clear client-shaped case) through `Batch`/`Query` builders, or add the mandated one-line exception comment at each of the three files explaining that the facade sits *below* the builder (it is the builder's execution target) — one `mod.rs`-level comment per file suffices.

### 5. Wire error-`code` contract populated unevenly across handler families; `TransactionInfo::aborted` reason mixes stable codes with free text
- **File:line:** coded examples: `admin_db_repo.rs:62-65 ("exists"), 117-123 ("still_referenced"), 177-183 ("unsupported_field")`, all `access_denied` sites; uncoded: ~68 `code: None` sites — e.g. `db_execute.rs:44-48` and `db_tx.rs:70-81, 169-173, 231-242` ("Database/Repository not found"), `helpers.rs:84-113` (name validation), and every non-access error in `admin_replication.rs`, `admin_access.rs`, `admin_function.rs`, `admin_validator.rs`, `admin_migration.rs`, `admin_buffer.rs`, `admin_interner.rs`; reason mixing: `db_tx.rs:252-272`
- **Severity:** medium
- **Issue:** `BatchError.code` is a real client-facing contract (there is a dedicated `tests/ddl_wire_e2e/error_codes.rs` suite and coded retry logic such as `version_conflict`), but whether a given failure carries a code depends on which handler family it hits: db/repo DDL codes its errors, while the structurally identical "not found"/validation failures in tx-begin, replication, access, function, and migration arms send `code: None`. Separately, `tx_commit_as` maps four `CommitError` variants to stable codes but `Storage`/`Expired` to human-readable strings inside the same `reason` field.
- **Failure scenario:** a client implements `match code { "exists" => ..., "access_denied" => ... }` per the DDL contract, then gets `None` for `Repository not found` from `tx_begin` and for every replication DDL error, and cannot distinguish retryable from permanent without parsing prose; commit-reason switch statements misroute `storage: ...`/`tx expired: ...` to the default arm.
- **Suggested fix:** define a small closed code vocabulary (not_found / validation / exists / unsupported_field / storage / tx_expired, alongside access_denied) in one place, seed the shared `err` closures in `helpers.rs` with it, and make `TransactionInfo::aborted` take only stable codes (move detail into a separate message field if the DTO allows).

### 6. `tx_begin` accepts any isolation string and silently falls back to Snapshot
- **File:line:** `crates/shamir-db/src/shamir_db/execute/db_tx.rs:82-85`
- **Severity:** medium
- **Issue:** `isolation: &str` is matched with `"serializable" => Serializable, _ => Snapshot`. There is no `TryFrom`-style validation and the typed `crate::engine::tx::IsolationLevel` exists but is not exposed at the facade boundary; any typo or future level ("repeatable_read") is silently downgraded.
- **Failure scenario:** a client requests `"serializable"` with a typo (or a newer SDK asks for an isolation this server doesn't know) and receives a Snapshot transaction — weaker guarantees — with an unqualified success response; the correctness bug surfaces only as an anomaly in production data.
- **Suggested fix:** return a typed validation error (`unsupported_isolation_level`, naming the accepted values) for unrecognized strings, or accept `IsolationLevel` directly and parse the wire string in the transport layer.

### 7. `to_qv` converts serialization failure into `QueryValue::Null` inside Ok responses
- **File:line:** `crates/shamir-db/src/shamir_db/execute/helpers.rs:73-78`; consumed at `admin_retention.rs:206` (ChangesSince events), `admin_buffer.rs`, `admin_access.rs`
- **Severity:** medium-low
- **Issue:** the msgpack round-trip helper chains `.ok().and_then(..).ok()` and ends in `.unwrap_or(QueryValue::Null)`, so a struct that fails to encode/decode (a bug, or a DTO/QueryValue shape mismatch) is silently replaced by `Null` in an otherwise successful admin response. This contradicts the project's error-handling rules (`Result<T, E>`, don't swallow) on the response-construction path.
- **Failure scenario:** `ChangesSince` returns `events: [Null, {...}, Null]` after a `ChangelogEvent` field stops round-tripping; a changefeed consumer treats the Nulls as deletions/unknown events and corrupts its projection, with no error anywhere.
- **Suggested fix:** make `to_qv` return `Result<QueryValue, BatchError>` (or log `error!` + fail the op); at minimum log loudly at the collapse point as `parse_one_rule_default` (`schema_management.rs:93-104`) already does.

### 8. Dead TLS/network dependencies and stale `net` doc kept "so the obsolete code doesn't bit-rot" — but the code is gone
- **File:line:** `Cargo.toml:64-68` (`rustls`, `tokio-rustls`, `rcgen`); `src/lib.rs:5-9` (doc claims the crate exposes `net`)
- **Severity:** low
- **Issue:** three crypto/TLS dependencies are declared for a legacy `db/net/*` module that no longer exists anywhere under `src/` (zero `rustls`/`rcgen`/`tokio_rustls` references), and the crate-level doc still advertises a `net` module at the root that isn't there. Every consumer build compiles the TLS stack for nothing, and the doc misstates the public surface.
- **Failure scenario:** a security reviewer auditing the dependency tree (a database binary) flags unexplained TLS stacks; a new contributor follows the crate doc hunting for `shamir_db::net` and finds nothing.
- **Suggested fix:** drop the three dependencies (git history preserves the module if it is ever revived) and correct the lib.rs doc line.

### 9. Malformed client input mapped to `DbError::Internal` in `get_ddl_op_status`
- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/core.rs:764-777`
- **Severity:** low
- **Issue:** an unparsable `op_id_str` (caller-supplied, e.g. from `DdlOpStatus.op_id` handed back through a lossy client) and an unresolvable table are both wrapped as `DbError::Internal(...)`, which by the project's convention signals programmer bug / repository corruption rather than bad request input (`DbError::Validation`/`NotFound` exist and are used for exactly this in `db_management.rs`).
- **Failure scenario:** a client that mangles an op id gets an "Internal error" the operator treats as a corruption event and pages on, instead of a clean 4xx-class validation error.
- **Suggested fix:** return `DbError::Validation(format!("Invalid op_id format: {e}"))` for the parse failure and `NotFound` for table resolution; reserve `Internal` for genuine invariant violations.

### 10. Catalogue `wasm_hash` and `version` fields are dead, and the hash is not integrity-grade
- **File:line:** `src/shamir_db/shamir_db/function_management.rs:186-189, 205, 216-218, 230-233`; `src/shamir_db/shamir_db/validator_management.rs:236-238, 267-270`
- **Severity:** low
- **Issue:** every function/validator row persists `wasm_hash` (FxHash over the bytes) and `version: 1`, but nothing in the workspace ever reads either, and `replace=true` overwrites keep `version: 1`, so the field can never mean what it says. FxHash is non-cryptographic and its digest is not stable across `rustc-hash` major releases — fine per pillar 4 for in-process keyed structures, but wrong as a persisted content-integrity/version identity.
- **Failure scenario:** a later task "verifies" a function against its persisted `wasm_hash` (the name invites it) and either accepts a spoofed module or rejects every row after a dependency upgrade.
- **Suggested fix:** either delete both fields until a consumer exists, or switch to a stable digest (e.g. SHA-256) and actually increment `version` on `replace`; document the on-disk contract next to `ArtifactKind::as_str`'s "stable — do not rename" note.

### 11. `create_db_as` reports success even when the catalogue write fails
- **File:line:** `crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:37-68`
- **Severity:** low
- **Issue:** the method returns `DbInstance` (not `DbResult`) and demotes a failed `save_database` to `log::warn!`, so the caller cannot distinguish a durably registered database from an in-memory-only one; the sibling `add_repo_as` propagates (`DbResult<()>`), making the contract incoherent across the same file. The wire path (`handle_create_db`) therefore answers `{"created": true}` for a database that vanishes on restart.
- **Failure scenario:** transient system-store failure during `CREATE DATABASE` → success response → client writes data → restart → database (and its catalogue row) gone; recovery tooling has no error to correlate.
- **Suggested fix:** return `DbResult<DbInstance>` from `create_db_as` and propagate (the bare `create_db` wrapper can keep its signature only if it keeps `#[doc(hidden)]`).

### 12. Nits
- **File:line / Severity:** various / nit
- `helpers.rs:59-63` — `.unwrap()` on `SystemTime::now().duration_since(UNIX_EPOCH)` in `admin_result_with_op_id`; a pre-epoch clock panics the wire handler. `db_management.rs:41-44` already shows the project's `unwrap_or_default` pattern — copy it.
- `db_gateway.rs:87-89` — `batch_err_to_string` formats with `{e:?}` (Debug), so the error text crossing the WASM `DbGateway` boundary is an unstable internal enum dump that also drops the `code` field; the `String` trait signature is engine-owned, but `Display` + `code` suffix would keep the guest-visible text stable.
- `src/main.rs` — the binary target is a Hello-World codec demo in what is otherwise a library facade; it ships a useless (and `#![allow(deprecated)]`) bin in the packaged crate.
- `ports.rs:7` — doc typo: "the narrow injected surface lets" → "that lets".

## Coverage notes (test organization conformance)

Test layout follows CLAUDE.md: every unit-test group lives in a `tests/` directory with a manifest-only `mod.rs` (`src/api/tests/`, `src/shamir_db/tests/`, `src/shamir_db/shamir_db/tests/`, `src/shamir_db/execute/tests/`), no inline `#[cfg(test)] mod tests` blocks exist in `src/`, and `tests/ddl_wire_e2e/serde_roundtrip.rs` correctly builds ops through `shamir-query-builder` (a compliant round-trip exception). Wire-surface coverage is broad (30+ integration files: `error_codes`, `idempotency_cascade`, `builder_execute_e2e`, `native_parity_e2e`, `cas_sequenced_e2e`, …). The one themed gap: no test exercises `replace=true` on a table-bound validator (finding 1) — the exact flow where the catalogue round-trip contract breaks.

</details>
