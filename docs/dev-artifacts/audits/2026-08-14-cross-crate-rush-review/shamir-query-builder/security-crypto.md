<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-query-builder — security-crypto independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The feature-disabled zeroization claim remains refuted by the actual transitive edge. Abandoned CreateUser builders retain an unprotected String lifetime. Missing local HMAC feedback is distinct from session authentication and server-side confirmation.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 3 | 2 | 0 | 0 | 1 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Plaintext password's zeroize-on-drop is silently disabled by this crate's own dependency profile

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

query-types unconditionally enables shamir-types defaults, including crypto, despite the builder's direct default-features=false edge. SecretString's cfg-gated Drop therefore remains enabled in this graph. This is pre-existing counter-evidence, not a source fix.

Evidence: [crates/shamir-query-builder/Cargo.toml:17](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/Cargo.toml#L17); [crates/shamir-query-types/Cargo.toml:11](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/Cargo.toml#L11); [crates/shamir-types/Cargo.toml:53](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/Cargo.toml#L53); [crates/shamir-types/src/secret.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/secret.rs#L67).

<a id="review-2"></a>

### Claim 2 — Builder holds the plaintext password in a bare `String` until `build()`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Dropping create_user(name,password) before build drops an ordinary String, bypassing SecretString's wipe. Private fields and absent Debug/Clone prevent the alleged direct downstream field logging. Exposure requires memory access; retention duration is unmeasured.

Evidence: [crates/shamir-query-builder/src/ddl/auth.rs:8](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/auth.rs#L8); [crates/shamir-query-builder/src/ddl/auth.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/auth.rs#L19); [crates/shamir-query-builder/src/ddl/auth.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/auth.rs#L52); [crates/shamir-types/src/secret.rs:68](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/secret.rs#L68).

<a id="review-3"></a>

### Claim 3 — "HMAC-gated" builders all build successfully without a tag -- gating is advisory-only client-side

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

CreateUser, Chmod and conditional CreateFunction construction leave hmac optional without local rejection. Inspected top-level Execute/TxExecute handlers reject missing or invalid required tags. The gate is documented as a 'did you mean it' confirmation, not authentication; no remote bypass follows from builder permissiveness.

Evidence: [crates/shamir-query-builder/src/ddl/auth.rs:49](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/auth.rs#L49); [crates/shamir-query-builder/src/ddl/access_control.rs:92](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/access_control.rs#L92); [crates/shamir-query-builder/src/ddl/function.rs:86](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/ddl/function.rs#L86); [crates/shamir-server/src/db_handler/admin.rs:637](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L637); [crates/shamir-server/src/db_handler/handler.rs:549](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/handler.rs#L549); [crates/shamir-server/src/db_handler/tx_handlers.rs:119](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tx_handlers.rs#L119).

Grouping/duplicate: [error-handling-lifecycle.md#3](error-handling-lifecycle.md#review-3). This is not an additional independent defect.

## Evidence and recipe corrections

- Adding crypto merely to restore zeroization is unnecessary in the frozen dependency graph. Wrap the password immediately while preserving transparent wire serialization and ownership transfer.
- Exact zeroize 1.8.2 archive confirms volatile slice clearing plus a compiler fence; SecretString invokes it on current string bytes. This does not establish removal of every caller copy, spare-capacity residue, serialized request buffer, or microarchitectural exposure.
- Typed DTO construction eliminates textual query interpolation, not every downstream security defect. HMAC coverage inspected here is top-level; nested and embedded authorization are not comprehensively proven.
- The assertion that remaining codec expects are input-unreachable is contradicted by the published decoder's deep-container failure mechanism.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-builder -- Security & crypto boundary

## Summary

This crate is a pure client-side builder: it constructs typed wire DTOs and never implements crypto itself (no `unsafe`, no HMAC computation, no secret comparison, no TLS), and its all-typed construction means there is no string-interpolation/injection surface -- it upholds CLAUDE.md's "query construction -- builder only" rule by construction. The crypto boundary lives in `ddl/auth.rs` / `ddl/access_control.rs` / `ddl/function.rs`, where plaintext passwords and admin HMAC tags are attached to ops; findings there are about the strength of the secret-handling guarantees this crate actually delivers (zeroize-on-drop silently feature-disabled; plaintext held in bare `String`; "HMAC-gated" is advisory-only). Enforcement of all tags/permissions is correctly deferred to the server; nothing here weakens it, but the client-side affordances overstate what they guarantee.

## Findings

### 1. Plaintext password's zeroize-on-drop is silently disabled by this crate's own dependency profile

- File:line: `crates/shamir-query-builder/src/ddl/auth.rs:52` (with `crates/shamir-query-builder/Cargo.toml:17,20`; `crates/shamir-types/src/secret.rs:67-75`)
- Severity: medium
- Issue: `CreateUser::build()` wraps the plaintext password into `SecretString`, whose whole point per its doc is "Drop that zeroizes the heap buffer before freeing it". That `Drop` impl is `#[cfg(feature = "crypto")]`, and this crate declares `shamir-types = { path = "../shamir-types", default-features = false }` (and `shamir-query-types` likewise), which turns `crypto` off. `shamir-types`' Cargo.toml confirms guest/WASM builds consume it exactly this way and "skip zeroize-on-drop". So in the crate's stated headline deployment (lib.rs: "compiles to WASM for browser clients") the password buffer is freed WITHOUT zeroization; Debug-redaction remains the only protection. In a native workspace build, feature unification re-enables `crypto` via other crates -- the guarantee's presence is an accident of the surrounding dependency graph, invisible at this call site.
- Failure scenario: a browser/WASM client built from this crate calls `create_user(...)`; the password String's heap buffer is deallocated un-wiped and persists in JS-heap/`memory` dumps (browser heap snapshots, devtools memory inspection) indefinitely after the request is sent -- a class of exposure zeroize exists to shrink.
- Suggested fix: depend on `shamir-types` with `features = ["crypto"]` (or add a minimal `secret` feature on shamir-types that pulls only `zeroize` and use it). Zeroize is a tiny, no_std-friendly dependency; the WASM-lean argument does not justify dropping the guarantee precisely where the crate handles credentials. Alternatively gate `SecretString`'s definition (not just its `Drop`) so a build without `crypto` cannot silently construct one and believe it is protected.

### 2. Builder holds the plaintext password in a bare `String` until `build()`

- File:line: `crates/shamir-query-builder/src/ddl/auth.rs:8-25, 49-57`
- Severity: low
- Issue: `CreateUser.password: String` keeps the cleartext in an unprotected `String` for the entire builder lifetime; only at `build()` is it moved into `SecretString`. If the builder is dropped without `build()` (error path, early return), the plaintext is never zeroized even when the `crypto` feature IS enabled; and until the move, the value is an ordinary `String` any intermediate code can `clone()`/log. (Credit where due: no `#[derive(Debug)]` on any auth builder, so no accidental `{:?}` leak from this crate itself, and `SecretString::from(String)` takes ownership without copying.)
- Failure scenario: `let b = create_user("alice", pw); if !ready { return Err(..) }` -- `b` drops with the password un-wiped even in a fully-featured native build.
- Suggested fix: wrap at the boundary: `create_user(name, password: impl Into<SecretString>)` (or store `SecretString` in the constructor immediately), shrinking the plaintext-as-bare-`String` window to the caller's own expression.

### 3. "HMAC-gated" builders all build successfully without a tag -- gating is advisory-only client-side

- File:line: `crates/shamir-query-builder/src/ddl/auth.rs:49-57,103-109,151-157,198-205`; `crates/shamir-query-builder/src/ddl/access_control.rs` (all builders); `crates/shamir-query-builder/src/ddl/function.rs:95-107`; also `ddl/drop_*.rs` / `ddl/replication.rs` (same pattern)
- Severity: low
- Issue: every HMAC-gated op stores `hmac: Option<String>` initialized to `None`, and both `build()` and `try_build()` finalize happily without `.hmac(...)` -- including `CreateFunction`, whose own doc says the tag is "Required IFF `security == "definer"` or `secret_grants` is non-empty", and `GrantRole`, documented as "the single most dangerous op in the system". There is no typestate, no `debug_assert!`, no `try_build` check. Doc language like "HMAC-gated (see CreateUser::hmac)" overstates what the builder enforces; the actual gate is 100% server-side rejection after a network round-trip.
- Failure scenario: a caller forgets `.hmac(...)`; the op ships and is rejected (or, worse, accepted if a server deployment misconfigures enforcement -- nothing client-side would have flagged it). The developer gets a wire error far from the construction site instead of a local compile/build-time signal.
- Suggested fix: for ops where the doc already says the tag is conditionally required (`CreateFunction` with `definer`/non-empty `secret_grants`), enforce in `build()`/`try_build()` with a typed error, or use a two-type state machine (`GrantRole` vs `GrantRoleTagged`) so only the tagged form converts to `BatchOp`. At minimum, reword the rustdocs from "HMAC-gated" to "HMAC-authorized server-side; optional here" so callers do not infer client enforcement. (The `canonical_*` doc pointers -- e.g. auth.rs:42 "never the password" -- are good hygiene; keep those.)

## Explicitly checked, no issue found

- **unsafe**: zero `unsafe` blocks in the crate (`rg "unsafe"` over `src/`: no hits). The only `unsafe` adjacent to this boundary is inside `shamir-types::secret`'s feature-gated `Drop`.
- **Timing side-channels**: the crate never compares secrets or tags (it only transports them as opaque strings), so there is no comparison-timing surface here. Tag verification happens server-side (sibling-review territory).
- **Injection**: no string interpolation into any query/command text anywhere (`format!` appears once, at `batch/batch.rs:943`, only to echo a user alias into a `BuildError` message). The one byte-slicing site (`batch/batch.rs:1361`, `split_path_tail`) is boundary-safe (`find(['[', '.'])` on ASCII chars). All construction is typed enum/map building -- the builder-only rule is the injection defense, and this crate honors it.
- **Untrusted input**: response extraction (`response/batch_response_ext.rs`) is fully `Result`-based -- missing alias, out-of-range row, and deserialize failures are typed errors, no panics/indexing on malformed server data. `try_build`'s `$query`-ref validation reuses typed walks plus a conservative msgpack fallback (`collect_op_query_refs`), correctly preferring over-validation for the ~75 unaudited op variants.
- **Panics on caller input**: the remaining `unwrap`/`expect` sites (`ddl/create_index.rs:772-862`, `batch/batch.rs:879-880`, `write/doc.rs:48-50`) are all guarded by immediately-preceding checks or documented-infallible codec round-trips with inline justifications -- invariants, not input-reachable.
- **Test coverage of this surface**: `ddl/tests/access_ddl_tests.rs` covers create_user wire shape (incl. `password` field), hmac attach/detach for chmod/chown/chgrp/groups/grant/revoke; `ddl/tests/schema_ddl_tests.rs` covers hmac on drop_db/drop_table/drop_index; macros/filter/subscribe tests confirm typed-only construction. No test asserts SecretString redaction here, but that invariant is tested at its definition site (`shamir-query-types` wire tests).

</details>
