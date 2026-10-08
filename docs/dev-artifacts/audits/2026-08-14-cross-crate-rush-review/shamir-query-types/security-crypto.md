<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-query-types — security-crypto revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

Constant-time MAC comparison and domain separation are source-supported for the pinned dependencies; SecretString redaction is present and zeroization is crypto-feature-dependent. Nested HMAC omission remains. The FilterValue depth-walk hole is fixed, and iteration limits now have an engine ceiling. Decode-abort assertions remain unverified.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 9 | 6 | 1 | 1 | 1 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

## Parent acceptance refinements

- Parent inspection resolves the previously unavailable codec source: decoding is depth-bounded; universal stack-safety is still not claimed.

<a id="review-1"></a>

### Claim 1 — No parse-time depth bound on recursive DTO deserialization — remote stack-overflow abort

Status: `refuted`. Current risk: —.

Parent source check of the checksummed rmp-serde 1.3.1 archive finds a default depth counter of 1024, decremented for arrays/maps and rejected with DepthLimitExceeded before unbounded descent. The categorical missing-bound / arbitrary-depth decode mechanism is refuted, not fixed. No project override increases that limit. This does not prove 1024 recursive containers are stack-safe on every target; a lower project limit is separate, unmeasured hardening.

Evidence: [crates/shamir-query-types/src/filter/filter_enum.rs:12](../../../../../crates/shamir-query-types/src/filter/filter_enum.rs#L12); [crates/shamir-query-types/src/batch/batch_op.rs:262](../../../../../crates/shamir-query-types/src/batch/batch_op.rs#L262); [crates/shamir-server/src/db_handler/handler.rs:344](../../../../../crates/shamir-server/src/db_handler/handler.rs#L344); [crates/shamir-connect/src/server/dispatch.rs:132](../../../../../crates/shamir-connect/src/server/dispatch.rs#L132); [crates/shamir-transport-tcp/src/framing.rs:56](../../../../../crates/shamir-transport-tcp/src/framing.rs#L56); [Cargo.lock:2949](../../../../../Cargo.lock#L2949).

Pinned dependency evidence: [rmp-serde 1.3.1, src/decode.rs:294](https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs); [rmp-serde 1.3.1, src/decode.rs:566](https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs).

<a id="review-2"></a>

### Claim 2 — Unbounded recursive walks over already-parsed attacker trees

Status: `confirmed-open`. Current risk: `medium`.

Dependency DFS/depth and value/filter walkers remain recursive without a hard traversal cap. Graph depth is checked after recursive calculation; planner traversal precedes engine filter validation. Default server query caps restrict graph risk, but higher operator caps and nested bodies expand it. Exact crash thresholds are unmeasured.

Evidence: [crates/shamir-query-types/src/batch/planner.rs:367](../../../../../crates/shamir-query-types/src/batch/planner.rs#L367); [crates/shamir-query-types/src/batch/planner.rs:618](../../../../../crates/shamir-query-types/src/batch/planner.rs#L618); [crates/shamir-query-types/src/batch/planner.rs:671](../../../../../crates/shamir-query-types/src/batch/planner.rs#L671); [crates/shamir-query-types/src/batch/planner.rs:721](../../../../../crates/shamir-query-types/src/batch/planner.rs#L721); [crates/shamir-engine/src/query/batch/batch_execute.rs:167](../../../../../crates/shamir-engine/src/query/batch/batch_execute.rs#L167); [crates/shamir-server/src/config.rs:424](../../../../../crates/shamir-server/src/config.rs#L424).

<a id="review-3"></a>

### Claim 3 — `check_filter_depth` does not descend into `FilterValue` — contradicts its own `$cond` claim

Status: `fixed`. Current risk: —.

Commit 66ddbf48 introduced an iterative combined Filter/FilterValue stack with exhaustive operand handling, including Cond condition/branches, Array, Expr, and FnCall arguments. Registered Array/Cond tests would fail with the old leaf-only checker. Engine validation also now includes when and HAVING.

Evidence: [crates/shamir-query-types/src/filter/filter_enum.rs:239](../../../../../crates/shamir-query-types/src/filter/filter_enum.rs#L239); [crates/shamir-query-types/src/filter/filter_enum.rs:271](../../../../../crates/shamir-query-types/src/filter/filter_enum.rs#L271); [crates/shamir-query-types/src/filter/filter_enum.rs:321](../../../../../crates/shamir-query-types/src/filter/filter_enum.rs#L321); [crates/shamir-query-types/src/filter/tests/filter_enum_tests.rs:236](../../../../../crates/shamir-query-types/src/filter/tests/filter_enum_tests.rs#L236); [crates/shamir-query-types/src/filter/tests/filter_enum_tests.rs:256](../../../../../crates/shamir-query-types/src/filter/tests/filter_enum_tests.rs#L256); [crates/shamir-query-types/src/filter/tests/mod.rs:1](../../../../../crates/shamir-query-types/src/filter/tests/mod.rs#L1); [crates/shamir-engine/src/query/batch/batch_validate.rs:85](../../../../../crates/shamir-engine/src/query/batch/batch_validate.rs#L85).

<a id="review-4"></a>

### Claim 4 — Destructive-op HMAC gate never reaches ops nested in `Batch`/`ForEach`; `is_admin()` misclassifies `ForEach` via non-exhaustive `matches!`

Status: `confirmed-open`. Current risk: `medium`.

check_destructive_hmacs still walks only batch.queries and skips containers via wildcard continue. Nested bodies execute with the same admin executor but no recursive confirmation check. ForEach remains excluded from is_admin; actor-aware DAC prevents interpreting this as unrestricted authorization bypass.

Evidence: [crates/shamir-server/src/db_handler/admin.rs:637](../../../../../crates/shamir-server/src/db_handler/admin.rs#L637); [crates/shamir-server/src/db_handler/admin.rs:656](../../../../../crates/shamir-server/src/db_handler/admin.rs#L656); [crates/shamir-server/src/db_handler/admin.rs:758](../../../../../crates/shamir-server/src/db_handler/admin.rs#L758); [crates/shamir-query-types/src/batch/batch_op.rs:577](../../../../../crates/shamir-query-types/src/batch/batch_op.rs#L577); [crates/shamir-engine/src/query/batch/query_runner.rs:686](../../../../../crates/shamir-engine/src/query/batch/query_runner.rs#L686).

<a id="review-5"></a>

### Claim 5 — Canonical HMAC inputs omit semantically destructive request fields (`cascade`, `dst_path`)

Status: `confirmed-open`. Current risk: `medium`.

Drop canonicalizers still omit cascade and migration canonicalization omits dst_path; server recomputation uses those same signatures. This weakens intent binding for authorized sessions, not TLS/SCRAM authentication.

Evidence: [crates/shamir-query-types/src/hmac.rs:101](../../../../../crates/shamir-query-types/src/hmac.rs#L101); [crates/shamir-query-types/src/hmac.rs:134](../../../../../crates/shamir-query-types/src/hmac.rs#L134); [crates/shamir-query-types/src/admin/types/migration_ops.rs:21](../../../../../crates/shamir-query-types/src/admin/types/migration_ops.rs#L21); [crates/shamir-server/src/db_handler/admin.rs:657](../../../../../crates/shamir-server/src/db_handler/admin.rs#L657).

<a id="review-6"></a>

### Claim 6 — Canonical-input encoding ambiguities: interior NULs and empty identifiers

Status: `confirmed-open`. Current risk: `low`.

Unescaped separators still permit distinct input tuples to share canonical bytes. Empty Function aliases FunctionNamespace, and slash-containing resource components alias other resource shapes. DTOs accept these strings; persistence/reachability varies by resource validation, so this is an intent-binding concern.

Evidence: [crates/shamir-query-types/src/hmac.rs:89](../../../../../crates/shamir-query-types/src/hmac.rs#L89); [crates/shamir-query-types/src/hmac.rs:184](../../../../../crates/shamir-query-types/src/hmac.rs#L184); [crates/shamir-query-types/src/hmac.rs:402](../../../../../crates/shamir-query-types/src/hmac.rs#L402); [crates/shamir-query-types/src/admin/access.rs:8](../../../../../crates/shamir-query-types/src/admin/access.rs#L8).

<a id="review-7"></a>

### Claim 7 — Destructive-op HMAC coverage has drifted: whole op families are ungated

Status: `confirmed-open`. Current risk: `low`.

The cited rename, function/validator-drop, and replication-DDL families still lack tags/canonicalizers and fall through the server confirmation gate. Authorization remains separate and present. Expanding confirmation to every family requires an explicit policy decision, not an assumed security requirement.

Evidence: [crates/shamir-query-types/src/admin/types/repo_ops.rs:61](../../../../../crates/shamir-query-types/src/admin/types/repo_ops.rs#L61); [crates/shamir-query-types/src/admin/types/function_ops.rs:72](../../../../../crates/shamir-query-types/src/admin/types/function_ops.rs#L72); [crates/shamir-query-types/src/admin/types/validator_ops.rs:34](../../../../../crates/shamir-query-types/src/admin/types/validator_ops.rs#L34); [crates/shamir-query-types/src/admin/types/repl_ops.rs:139](../../../../../crates/shamir-query-types/src/admin/types/repl_ops.rs#L139); [crates/shamir-server/src/db_handler/admin.rs:758](../../../../../crates/shamir-server/src/db_handler/admin.rs#L758).

<a id="review-8"></a>

### Claim 8 — `BatchLimits` are fully client-supplied; only 3 of 6 fields are server-clamped, and the crate offers no clamping helper

Status: `partially-fixed`. Current risk: `low`.

DTO defaults/helper and server clamps remain unchanged, leaving dependency/nesting limits client-controlled. Engine effective_max_iterations now clamps the body's requested iteration count to 100000 before iteration zero; usize::MAX no longer disables that gate.

Evidence: [crates/shamir-query-types/src/batch/batch_limits.rs:31](../../../../../crates/shamir-query-types/src/batch/batch_limits.rs#L31); [crates/shamir-server/src/db_handler/handler.rs:489](../../../../../crates/shamir-server/src/db_handler/handler.rs#L489); [crates/shamir-server/src/db_handler/tx_handlers.rs:87](../../../../../crates/shamir-server/src/db_handler/tx_handlers.rs#L87); [crates/shamir-engine/src/query/batch/query_runner.rs:36](../../../../../crates/shamir-engine/src/query/batch/query_runner.rs#L36); [crates/shamir-engine/src/query/batch/query_runner.rs:846](../../../../../crates/shamir-engine/src/query/batch/query_runner.rs#L846); [crates/shamir-engine/src/query/batch/tests/dos_gate_tests.rs:214](../../../../../crates/shamir-engine/src/query/batch/tests/dos_gate_tests.rs#L214); [crates/shamir-engine/src/query/batch/tests/mod.rs:4](../../../../../crates/shamir-engine/src/query/batch/tests/mod.rs#L4).

<a id="review-9"></a>

### Claim 9 — Derived `Debug` on `DbRequest::ChangePasswordVerify` prints long-term SCRAM credential material

Status: `confirmed-open`. Current risk: `low`.

DbRequest still derives Debug over plain credential/proof Vec fields. Exposure is conditional on logging/debug formatting; no production request-Debug logger was established. Stored/server keys alone do not supply the client_key required for a fresh client proof, so immediate client impersonation was overstated.

Evidence: [crates/shamir-query-types/src/wire/db_message.rs:29](../../../../../crates/shamir-query-types/src/wire/db_message.rs#L29); [crates/shamir-query-types/src/wire/db_message.rs:159](../../../../../crates/shamir-query-types/src/wire/db_message.rs#L159); [crates/shamir-connect/src/common/scram.rs:72](../../../../../crates/shamir-connect/src/common/scram.rs#L72); [crates/shamir-connect/src/common/scram.rs:96](../../../../../crates/shamir-connect/src/common/scram.rs#L96).

## Corrections and qualified non-findings

- Remove categorical remote-abort and 100000-frame-depth claims until pinned decoder behavior and stack reachability are proven. Existing TCP/WS frame limits are not missing.
- The combined post-parse depth checker is fixed; this does not prove decode-time safety or bound planner recursion that occurs before validation.
- Current engine filter validation includes when and HAVING; the original top-level-WHERE-only statement is stale.
- max_iterations now has an absolute engine ceiling of 100000, although the six-field clamping proposal remains incomplete.
- HMAC collisions/coverage are confirmation-of-intent weaknesses for authorized session holders, not independent authentication breaks.
- Correct the credential-exposure scenario: client SCRAM proof creation additionally requires client_key, not merely stored_key/server_key.
- SecretString Debug redaction is unconditional; zeroization is enabled only with the forwarded crypto feature.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-query-types -- Security & crypto boundary

## Summary

This crate is the shared client/server DTO layer for the post-handshake wire protocol plus the canonical HMAC-confirmation scheme for destructive ops. The crypto core is sound: no `unsafe` anywhere, constant-time tag verification (`Hmac::verify_slice`), domain-separated session-key derivation, `SecretString` hygiene on passwords, and hardened exhaustive `match` classifiers (`required_access`, `is_write`) with no wildcard arms. The real boundary weaknesses are on the untrusted-input side: every recursive DTO (`Filter`, `FilterValue`, `QueryValue`, `BatchOp` via nested `Batch`/`ForEach`) decodes with no parse-time depth bound (stack-overflow abort reachable post-handshake), several recursive tree-walks over those same attacker-supplied trees are unbounded, and the destructive-op HMAC scheme has both coverage holes (ops nested in `Batch`/`ForEach` are never gate-checked; `cascade`/`dst_path` are absent from the canonical inputs) and encoding ambiguities (interior NULs, empty identifiers).

## Findings

### 1. No parse-time depth bound on recursive DTO deserialization — remote stack-overflow abort
- **File:** `crates/shamir-query-types/src/filter/filter_enum.rs:9,127` (`Filter::Not(Box<Filter>>)`), `src/filter/filter_value.rs:46,70-74` (`Array(Vec<FilterValue>)`, `Cond(Box<Cond>)`), `src/batch/batch_op.rs:262` (`QueryValue::deserialize` + nested `Batch`/`ForEach` re-entry), `src/batch/sub_batch_op.rs:13`, `src/batch/for_each_op.rs:37`
- **Severity:** high
- **Issue:** `Filter`, `FilterValue`, `Cond`, `SelectExpr`, `QueryValue` and `BatchOp` (via `SubBatchOp`/`ForEachOp` → `BatchRequest` → `QueryEntry` → `BatchOp`) are all self-referential, and their serde `Deserialize` impls recurse once per nesting level. `rmp-serde` has no depth limiter, and `MAX_FILTER_DEPTH`/`check_filter_depth` (filter_enum.rs:7-9, 219) can only run *after* deserialization completes — the doc's stated purpose ("rejected to prevent stack overflow post-handshake") is unachievable for the decode itself, which is exactly where the overflow happens. A ~1 MB msgpack frame of nested arrays/maps (≈100k depth) inside an `Execute` envelope blows the thread stack before any DTO-level guard can run; in Rust that aborts the whole server process, taking down every session.
- **Failure scenario:** Any authenticated client sends `{"op":"execute","db":"x","batch":{"queries":{"a":{"for_each":{"over":[],"bind_row":"r","queries":{"b":{"for_each":{...}}}}}}}}` (or nested `[[[[...]]]]` as an insert value). Deserialization recurses ~100k frames deep on a standard 2 MB tokio worker stack → `thread has overflowed its stack` → process abort → full-server DoS from one frame.
- **Suggested fix:** Enforce depth at decode time: a depth-counting `Deserializer` wrapper (fails at e.g. 128 nested containers) applied wherever untrusted frames enter these types, or manual bounded `Deserialize` visitors for the recursive spine (`Filter::Not`, `FilterValue::Array/Cond`, `QueryValue::List/Map`, `BatchOp::Batch/ForEach`). Pair it with a transport-level max-frame check. Update the `MAX_FILTER_DEPTH` doc to state it bounds *post-parse* walks only, not decode.

### 2. Unbounded recursive walks over already-parsed attacker trees
- **File:** `src/batch/batch_op.rs:764,771` (`is_write` recursion), `src/batch/query_entry.rs:102-113,140-155` (`collect_repos` / `collect_required_access_into`), `src/batch/planner.rs:367-442` (`extract_deps_from_value` over `QueryValue`), `planner.rs:445-505,535-569,584-606,618-648` (`extract_deps_from_filter`, `contains_field_based_comparison`, `filter_value_contains_field_based_comparison`, `extract_deps_from_filter_value`), `planner.rs:671-703,721-754` (`detect_cycle` DFS, `calculate_max_depth`)
- **Severity:** medium
- **Issue:** The crate already solves this class iteratively where it noticed it — `max_nesting_depth_of_ops` (planner.rs:763-796, `NESTING_WALK_LIMIT = 64`) exists precisely "so a malicious deeply-nested payload cannot blow the call stack", and `check_filter_depth` is likewise iterative. But every other walk over the same untrusted trees recurses without any bound: write-value `QueryValue` trees have no depth check anywhere (unlike `Filter`, whose depth is checked engine-side), and the `when`/`$cond` walks run at plan time on trees `validate_filter_depth` never inspects. Any depth the deserializer survives (finding 1) is revisited here with additional frames per level.
- **Failure scenario:** A client sends a moderately deep (few-thousand-level) nested map as an `InsertOp.values[0]` or inside a `$cond` chain — deep enough to decode on a large-stack thread, deep enough that `extract_deps_from_value`'s recursion overflows during planning. Same process-abort outcome as finding 1, second bite at the apple.
- **Suggested fix:** Give the recursive walkers an explicit depth parameter capped at a small constant (nesting is already capped at 4 by default), returning `BatchError::NestingTooDeep`/`TooDeep` on exceed; convert `extract_deps_from_value` to the same worklist shape as `max_nesting_depth_of_ops`. One shared bounded-walk helper for `QueryValue`/`FilterValue` trees would prevent the next drift.

### 3. `check_filter_depth` does not descend into `FilterValue` — contradicts its own `$cond` claim
- **File:** `crates/shamir-query-types/src/filter/filter_enum.rs:7-8,219-238`
- **Severity:** medium
- **Issue:** The `MAX_FILTER_DEPTH` doc says "Deeply-nested `$cond`/`not`/`and`/`or` beyond this cap will be rejected", but `check_filter_depth`'s match only recurses through `And`/`Or`/`Not`; the `_ => {}` arm ignores every comparison variant's `FilterValue` operand. `FilterValue::Cond` embeds `Cond.condition: Box<Filter>` (cond.rs:44), so a 100k-deep `and/or/not` chain hidden inside a `$cond` (e.g. as `Eq.value`) passes the check untouched. Downstream, the engine's `validate_filter_depth` (shamir-engine `batch_validate.rs:78-97`) only inspects top-level `Read`/`Delete`/`Update` WHERE clauses — not `when`, not `GroupBy.having`, not nested bodies — so nothing else catches it either.
- **Failure scenario:** Client sends `{"op":"eq","field":"x","value":{"$cond":{"if":<64+-deep and/or/not tree>,"then":1,"else":2}}}`. Depth check passes; the planner's `extract_deps_from_filter_value` then recurses the full hidden depth (finding 2) and/or the evaluator recurses at run time.
- **Suggested fix:** Extend `check_filter_depth` to walk `FilterValue` operands (mirroring `extract_deps_from_filter_value`'s coverage of `Cond`/`Expr`/`FnCall`/`Array`), or fix the doc to drop the `$cond` claim. Add a test with a `$cond`-embedded over-deep tree.

### 4. Destructive-op HMAC gate never reaches ops nested in `Batch`/`ForEach`; `is_admin()` misclassifies `ForEach` via non-exhaustive `matches!`
- **File:** `crates/shamir-query-types/src/batch/batch_op.rs:577-646` (`is_admin` — includes `BatchOp::Batch(_)` at :634 but not `ForEach(_)`, `Call(_)`, `Subscribe`/`Unsubscribe`), cf. `is_write`'s deliberately exhaustive match at :660-773 and `required_access`'s no-wildcard rationale at :449-456
- **Severity:** medium
- **Issue:** (a) `is_admin` is a `matches!` list that silently defaults new/unlisted variants to `false` — the exact "wildcard silently swallows the decision" weakness the crate's own `is_write`/`required_access` doc comments call out and fixed by going exhaustive. `ForEach` wraps a nested `BatchRequest` exactly like `Batch` does, yet `Batch` is admin-classified and `ForEach` is not. (b) Verified downstream consequence: the server's two top-level-only gates inherit this shape — `check_destructive_hmacs` (shamir-server `db_handler/admin.rs:637-777`) iterates only `batch.queries` and `continue`s on `Batch`/`ForEach`, so a `drop_db`/`grant_role`/`purge_history` nested one level inside either wrapper executes with **no** "did-you-mean-it" tag at all; and the superuser coarse gate (`handler.rs:512-521`) skips `ForEach`-wrapped admin ops for non-superusers (containment verified: each admin handler still runs its own `authorize_access` DAC check, so this is gate inconsistency, not privilege escalation).
- **Failure scenario:** A superuser client with a buggy confirm-dialog sends `{"batch":{"queries":{"x":{"drop_db":"prod"}}}}` — no `hmac` field anywhere; the gate sees only the `Batch` wrapper, `continue`s, and `prod` is dropped without the confirmation the protocol promises. Same payload via `for_each` additionally skips the coarse-gate error path.
- **Suggested fix:** In this crate: make `is_admin` an exhaustive `match` (no `matches!`) and decide `ForEach`/`Call` explicitly, mirroring `is_write`; provide a recursive `for_each_op`/`collect_destructive_ops` helper (the `collect_required_access` shape, query_entry.rs:127-155, is the precedent) so gates can walk nested bodies. Server side: drive `check_destructive_hmacs` through that helper. Add regression tests asserting a nested `DropDb` requires its tag.

### 5. Canonical HMAC inputs omit semantically destructive request fields (`cascade`, `dst_path`)
- **File:** `crates/shamir-query-types/src/hmac.rs:101-163` (`canonical_drop_db/repo/table`, `canonical_start_migration`), vs `src/admin/types/db_ops.rs:35`, `repo_ops.rs:34`, `table_ops.rs:54` (`cascade: bool`), `migration_ops.rs:21` (`dst_path: Option<String>`)
- **Severity:** medium
- **Issue:** The tag confirms "drop this repo/table/db" but not "recursively destroy everything inside it": `cascade` is absent from all three canonical inputs, and `start_migration`'s `dst_path` (a filesystem path for the destination store) is absent from its canonical input. A tag legitimately computed for the non-cascade / path-less op verifies byte-identically for the maximally destructive variant, because the server (verified: `admin.rs:657-680`) recomputes from the same under-specified canonical form.
- **Failure scenario:** A confirmation UI signs `b"drop_repo\0db\0cold"` (plain drop). The request is then sent — or mutated by a buggy client/proxy — with `cascade: true`; the tag still verifies and every table in the repo is destroyed, an action the user never confirmed. Analogously, a migration can be steered to an attacker-chosen `dst_path` under a tag computed without it.
- **Suggested fix:** Include the discriminating fields in the canonical inputs (e.g. `b"drop_repo\0db\0repo\0cascade=0|1"`, append `dst_path` or a `"none"` sentinel). This is a wire-format change — bump the `hmac key v1` domain-separation string (e.g. `v2`) so old tags cannot validate against the new inputs.

### 6. Canonical-input encoding ambiguities: interior NULs and empty identifiers
- **File:** `crates/shamir-query-types/src/hmac.rs:89-99` (`join_null`), `hmac.rs:184-196` (`canonical_resource_ref`), `hmac.rs:402-408` (`canonical_create_scram_user`)
- **Severity:** low
- **Issue:** `join_null` performs no component validation, so `("a\0b", "c")` and `("a", "b\0c")` canonicalize identically — two *different* grant/drop/user tuples share one tag. `canonical_resource_ref` has collisions of its own: `Function { function: "" }` renders `"fn://"`, identical to the `FunctionNamespace` singleton; `FunctionFolder { [] }` renders `"fn:///"`; empty db/store/table names also collide across variants (`Database { "" }` → `"db://"`). The module itself demonstrates the right rigor for `GroupRef` ("can never collide between variants", hmac.rs:70-75) but does not extend it to the other encoders, and no test covers NUL-containing or empty components.
- **Failure scenario:** If names with interior NULs survive anywhere into stored principals/resources (only the SCRAM username path is documented as normalised via SASLprep — role/db/repo/table/index names are unvalidated `String`s in this crate), a tag obtained to confirm op A also confirms the distinct op B, silently defeating the intent-confirmation the tag exists to provide.
- **Suggested fix:** Reject `\0` and empty components in every `canonical_*` helper (return `Option`/`Result`), or length-prefix each part instead of NUL-separating. Add ambiguity tests to `hmac_tests.rs`.

### 7. Destructive-op HMAC coverage has drifted: whole op families are ungated
- **File:** `crates/shamir-query-types/src/admin/types/repo_ops.rs:61-65,87-91` (`RenameRepoOp`, `RenameDbOp` — no `hmac`), `table_ops.rs:84-90` (`RenameTableOp`), `index_ops.rs:99-126` (`RenameIndexOp`), `function_ops.rs:72-90` (`DropFunctionOp`, `RenameFunctionOp`), `validator_ops.rs:34-41` (`DropValidatorOp`), `repl_ops.rs:138-161,182-186` (`DropReplicationProfileOp`, `DropPublicationOp`, `DropSubscriptionOp` — cluster-topology mutations); no `canonical_*` counterparts in `src/hmac.rs`
- **Severity:** low
- **Issue:** The confirmation scheme gates `rename_group` and `create_group` but not `rename_db`/`rename_repo`/`rename_table`/`rename_index`; it gates `drop_index` but not `drop_function` (which destroys code plus its `secret_grants`/`net_grants` bindings); it gates nothing in the replication-DDL family or validator DDL. There is no single classifier enumerating "ops requiring a tag", so each new op family silently ships ungated — the same drift pattern `required_access`/`is_write` eliminated by exhaustive matching.
- **Failure scenario:** An operator fat-fingers `rename_db` on the production console, or a scripted client drops the wrong publication — the classes of accident the HMAC exists to catch, with no confirmation gate to catch them.
- **Suggested fix:** Add a `BatchOp::requires_hmac(&self) -> bool` exhaustive match (no wildcard) as the single source of truth, generate the `hmac: Option<String>` field and canonical helper per gated op, and make the server gate iterate that classifier (recursively, per finding 4).

### 8. `BatchLimits` are fully client-supplied; only 3 of 6 fields are server-clamped, and the crate offers no clamping helper
- **File:** `crates/shamir-query-types/src/batch/batch_limits.rs:31-86`, consumed via `batch_request.rs:97-99`
- **Severity:** low
- **Issue:** The DTO is documented as "Execution limits for security. Prevents DoS attacks", but every field arrives from the request. Verified server-side (shamir-server `db_handler/handler.rs:489-505`): only `max_result_size`, `max_execution_time_secs`, `max_queries` are `min()`-clamped against operator caps — `max_nesting_depth`, `max_dependency_depth`, and `max_iterations` are taken verbatim from the client. A client can send `max_nesting_depth: usize::MAX`, disabling the nesting gate the planner enforces (`BatchPlanner::plan`, planner.rs:127-133) and un-binding recursive execution depth; `max_iterations: usize::MAX` similarly neutralises the `for_each` runtime cap.
- **Failure scenario:** Authenticated client sends a batch with inflated limits and a deeply nested `for_each`/`batch` tree; every depth/iteration backstop that assumed a default of 4/1000 is switched off by the attacker themselves.
- **Suggested fix:** Add a `BatchLimits::clamped_against(server_caps: &BatchLimits) -> BatchLimits` helper in this crate (taking per-field `min`) so the server clamps all six fields through one auditable call site, and document that these fields are *requests*, never authorities.

### 9. Derived `Debug` on `DbRequest::ChangePasswordVerify` prints long-term SCRAM credential material
- **File:** `crates/shamir-query-types/src/wire/db_message.rs:29` (`#[derive(Debug, ...)] enum DbRequest`), fields at `:159-173` (`new_stored_key`, `new_server_key`, `client_proof_old` as plain `Vec<u8>`)
- **Severity:** low
- **Issue:** `ChangePasswordVerify` carries `new_stored_key`/`new_server_key` — the new long-term server-side credential. Possessing the `(stored_key, server_key)` pair is sufficient to complete future SCRAM exchanges as that user without the password, which is precisely why `User::password_hash` and `CreateScramUser::password` are wrapped in `SecretString` (redacted `Debug`, zeroize-on-drop) one file over (auth/types.rs:145-170, db_message.rs:57-81). The change-password fields get no such treatment: any `tracing::debug!`/log of the deserialized `DbRequest` prints the raw key bytes.
- **Failure scenario:** Verbose request logging on a server (or client SDK debug dump) writes `new_stored_key`/`new_server_key` into logs; anyone with log access can impersonate the user in subsequent handshakes.
- **Suggested fix:** Implement a manual `Debug` for `ChangePasswordVerify` that redacts the three byte fields (or wrap them in a redacting newtype from `shamir_types::secret`), matching the established `SecretString` precedent.


</details>
