<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-engine — security-crypto independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The designated filter repairs and default authorization seam are present. Trusted replication is intentional. Exact dependencies refute universally unbounded transport decoding, but normal executable-value surfaces still bypass the collector.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 15 | 0 | 4 | 3 | 1 | 1 | 6 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1"></a>

### Claim 1 — Filter-depth DoS guard misses when, having, and all FilterValue nesting

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

The specified when/HAVING collector omissions and embedded-value walker omissions are removed. This does not cover every other executable FilterValue surface.

Evidence: [crates/shamir-engine/src/query/batch/batch_validate.rs:85](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/batch_validate.rs#L85); [crates/shamir-query-types/src/filter/filter_enum.rs:239](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_enum.rs#L239); [crates/shamir-query-types/src/filter/filter_enum.rs:342](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_enum.rs#L342).

<a id="review-1-transport-recursion-assertion"></a>

### Claim 1/transport recursion assertion — Transport-layer serde recursion is equally unbounded

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `unverified`.

Exact rmp-serde 1.3.1 defaults depth to 1024 and counts any_inner map/array/ext recursion. The server decodes DbRequest with it and BatchOp first reads QueryValue through deserialize_any. Enum/option/newtype dispatch is not itself counted, so no universal codec safety claim follows. Published source: https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs.

Evidence: [Cargo.lock:2949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2949); [crates/shamir-server/src/db_handler/handler.rs:344](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/handler.rs#L344); [crates/shamir-query-types/src/batch/batch_op.rs:256](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/batch_op.rs#L256); [crates/shamir-types/src/types/value.rs:283](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/value.rs#L283).

<a id="review-2"></a>

### Claim 2 — Per-row recompile of cond conditions on the WHERE path

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

The fallback cache returns the stored node on a sequential content hit. Registered Arc-identity assertions supplement the actual hit branch; formatting work remains.

Evidence: [crates/shamir-engine/src/query/filter/resolve.rs:415](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/resolve.rs#L415); [crates/shamir-engine/src/query/filter/cond_cache.rs:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/cond_cache.rs#L99); [crates/shamir-engine/src/query/filter/tests/local_cond_cache_tests.rs:214](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/tests/local_cond_cache_tests.rs#L214).

<a id="review-2-regex-budget-and-timing-assertions"></a>

### Claim 2/regex budget and timing assertions — Linear regex matching, default 10 MB budget and compile-time estimates

Status: `unverified`. Current risk: `—`.

Prior-cycle decision: `unverified`.

Exact regex 1.12.3 supports O(m*n) single-search complexity and a default 10 MiB NFA budget, not a 10 MB input-pattern or total-memory cap. Historical compile-time estimates remain unmeasured. Published source: https://docs.rs/crate/regex/1.12.3/source/src/builders.rs.

Evidence: [Cargo.lock:2844](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2844); [Cargo.lock:2856](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2856); [crates/shamir-engine/src/query/filter/compile.rs:108](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/compile.rs#L108).

<a id="review-3"></a>

### Claim 3 — Engine boundary performs no authorization — enforcement is a single upstream wrapper

Status: `partially-fixed`. Current risk: `low`.

Prior-cycle decision: `partially-fixed`.

Normal executors require Authorized and the live facade uses DAC. Raw actor-less APIs, custom AccessGate implementations and test-util remain explicit trusted-embedding surfaces; no current server bypass is demonstrated.

Evidence: [crates/shamir-engine/src/query/batch/authorized.rs:92](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/authorized.rs#L92); [crates/shamir-engine/src/query/batch/authorized.rs:127](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/authorized.rs#L127); [crates/shamir-db/src/shamir_db/execute/db_execute.rs:40](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/db_execute.rs#L40); [crates/shamir-engine/src/db_instance/db_instance.rs:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/db_instance/db_instance.rs#L61).

<a id="review-4"></a>

### Claim 4 — Replication apply is a trusted raw write — no re-validation of leader events

Status: `partially-fixed`. Current risk: `low`.

Prior-cycle decision: `partially-fixed`.

Trust documentation and an optional policy exist, but the server deliberately selects Trusted. Rerunning schema/DAC is not the replay contract; structural validation under the opt-in policy is separately defective.

Evidence: [crates/shamir-engine/src/tx/apply_replicated.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/apply_replicated.rs#L3); [crates/shamir-engine/src/tx/apply_replicated.rs:198](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/tx/apply_replicated.rs#L198); [crates/shamir-server/src/replication/follower_loop.rs:343](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L343); [docs/dev-artifacts/roadmap/REPLICATION.md:272](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/dev-artifacts/roadmap/REPLICATION.md#L272).

<a id="review-5"></a>

### Claim 5 — Pointer-keyed caches expose documentation-only lifetime invariants and stale-hit hazards

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Keys derive from content rather than allocation addresses. Interner and resolved-reference freshness still require the documented per-context/per-scan scope.

Evidence: [crates/shamir-engine/src/query/filter/cond_cache.rs:53](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/cond_cache.rs#L53); [crates/shamir-engine/src/query/filter/field_path_cache.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/field_path_cache.rs#L43); [crates/shamir-engine/src/query/filter/query_ref_cache.rs:86](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/query_ref_cache.rs#L86).

<a id="review-6"></a>

### Claim 6 — Regex/Like patterns have no size cap and invalid patterns silently compile to False

Status: `partially-fixed`. Current risk: `low`.

Prior-cycle decision: `partially-fixed`.

Collected batch/interactive filters reject invalid or over-64-KiB patterns. Direct compilation and uncollected value-condition surfaces still bypass this gate; NOT can invert False folding.

Evidence: [crates/shamir-engine/src/query/filter/pattern_guard.rs:35](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/pattern_guard.rs#L35); [crates/shamir-engine/src/query/batch/batch_validate.rs:85](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/batch_validate.rs#L85); [crates/shamir-engine/src/query/filter/compile.rs:108](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/compile.rs#L108); [crates/shamir-engine/src/query/batch/query_runner.rs:789](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/query_runner.rs#L789).

<a id="review-7"></a>

### Claim 7 — SessionPermissions RBAC remains publicly exported while being test-only scaffolding

Status: `fixed`. Current risk: `—`.

Prior-cycle decision: `fixed`.

Default production excludes SessionPermissions; the benchmark explicitly requires test-util. The empty loop is removed.

Evidence: [crates/shamir-engine/src/query/auth/mod.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/auth/mod.rs#L19); [crates/shamir-engine/src/query/auth/session.rs:166](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/auth/session.rs#L166); [crates/shamir-engine/Cargo.toml:185](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/Cargo.toml#L185).

<a id="review-positive-observations-no-unsafe"></a>

### Claim Positive observations/No unsafe — No unsafe library implementation or local cryptographic primitives

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Scoped library inventory supports this non-finding; sibling authentication and cryptography are outside this audit.

Evidence: [crates/shamir-engine/src/lib.rs:12](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/lib.rs#L12); [crates/shamir-engine/src/query/auth/mod.rs:27](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/auth/mod.rs#L27).

<a id="review-positive-observations-fail-closed-wasm-validator-bridge"></a>

### Claim Positive observations/Fail-closed WASM validator bridge — Invocation and result-decode failures reject with stop=true

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Both failure branches return an error-bearing Validation with stop=true; FnCtx receives the actor.

Evidence: [crates/shamir-engine/src/validator/wasm_record_validator.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/validator/wasm_record_validator.rs#L55); [crates/shamir-engine/src/validator/wasm_record_validator.rs:84](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/validator/wasm_record_validator.rs#L84); [crates/shamir-engine/src/validator/wasm_record_validator.rs:89](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/validator/wasm_record_validator.rs#L89).

<a id="review-positive-observations-corrupt-record-hygiene"></a>

### Claim Positive observations/Corrupt-record hygiene — Corrupt-record reporting exposes references rather than raw bytes

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The reporting interface carries references. Detection is not exhaustive because RecordView construction validates only the header.

Evidence: [crates/shamir-engine/src/table/read_index_scan.rs:336](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/table/read_index_scan.rs#L336); [crates/shamir-types/src/record_view/lens.rs:767](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/lens.rs#L767).

<a id="review-positive-observations-dos-hardening-that-does-exist"></a>

### Claim Positive observations/DoS hardening that does exist — Absolute ForEach cap and cooperative execution deadline

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Iterations clamp at 100000 and deadline checks occur at safe work boundaries. Interactive execution is explicitly unbounded; a stuck individual operation is not interrupted.

Evidence: [crates/shamir-engine/src/query/batch/query_runner.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/query_runner.rs#L36); [crates/shamir-engine/src/query/batch/query_runner.rs:45](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/query_runner.rs#L45); [crates/shamir-engine/src/query/batch/execution_deadline.rs:242](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/execution_deadline.rs#L242).

<a id="review-positive-observations-like-conversion"></a>

### Claim Positive observations/LIKE conversion — LIKE conversion escapes regex metacharacters

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The escape arm handles regex metacharacters while retaining intentional percent/underscore wildcard conversion.

Evidence: [crates/shamir-engine/src/query/filter/fts.rs:16](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/filter/fts.rs#L16).

<a id="review-positive-observations-secret-comparisons"></a>

### Claim Positive observations/Secret comparisons — No local secret or timing-sensitive comparison implementation

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Auth/secret types are delegated; no local password/token comparator was identified in the reviewed engine surface.

Evidence: [crates/shamir-engine/src/query/auth/mod.rs:27](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/auth/mod.rs#L27).

## Evidence and recipe corrections

- The normal transport path is not equally unbounded. Container depth guards are source-proven, while stack safety up to that limit is not measured.
- The filter collector is not a universal client-input guard: ForEach.over, sub-batch bind and SELECT function arguments can contain executable conditions.
- Returning False/None on depth exhaustion is not a safe hardening recipe under NOT or conditional evaluation. Reject malformed executable trees explicitly.
- A pattern-size cap measures UTF-8 input bytes; the regex NFA budget measures compiled-program size. Neither establishes an exact CPU or RSS threshold.
- Authorized protects against accidentally omitting the check, not against a deliberately permissive AccessGate or pairing the token with an unrelated custom resolver.
- Rerunning nondeterministic validators on followers could reject a legitimately decided leader event. Structural screening and transport integrity are different obligations.
- test-util is an explicit feature, not an impossible production configuration. Default production exclusion is the supported statement.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-engine -- Security & crypto boundary

## Summary

shamir-engine contains **no crypto primitives of its own** — HMAC (stored-function
definer tags), SCRAM/Argon2 auth, and TLS all live in sibling crates
(`shamir-query-types::hmac`, `shamir-types::secret`, `shamir-connect`, transports).
This crate's security surface is: (a) the batch/query execution engine that consumes
**client-supplied filter/expr trees** (depth/recursion/regex DoS), (b) actor
(shamir_types::access::Actor) threading toward the real enforcement gate
(`ShamirDb::execute_as`, outside this crate), (c) the replication apply path, which
is an explicitly **trusted** raw-write boundary, and (d) the WASM validator bridge.
No `unsafe` blocks exist in library code (only a `GlobalAlloc` counter in an example),
corrupt-record reporting leaks only `(table, id)` — never raw bytes — and WASM
validator failures fail closed (`stop = true`). The main gaps are incompleteness of
the designated untrusted-input DoS guards and a per-row recompile amplification in
the `$cond` evaluation path.

Scope reviewed: all of `src/query/` (auth, batch, filter, read, admin, common),
`src/validator/`, `src/tx/apply_replicated.rs`, `src/meta/`, `src/repo/`,
`src/db_instance/`, `src/table/` (security-relevant paths), `Cargo.toml`, plus test
dirs for coverage claims.

## Findings

### 1. Filter-depth DoS guard misses `when`, `having`, and all `FilterValue` nesting — designated guard is incomplete
- **File:line:** `crates/shamir-engine/src/query/batch/batch_validate.rs:78-97` (guard),
  `query_runner.rs:136-157` (`when` compiled, never depth-checked),
  `query/read/aggregate.rs:1304-1311` (`having` compiled, never depth-checked),
  `shamir-query-types/src/filter/filter_enum.rs:219-238` (`check_filter_depth` walks
  only `And`/`Or`/`Not` — never descends into `FilterValue` operands).
- **Severity:** medium
- **Issue:** `validate_filter_depth` collects filters from exactly three places:
  `Read(q.r#where)`, `Delete(d.where_clause)`, `Update(u.where_clause)`. Three classes
  of client-supplied filter trees reach recursive compilation/evaluation **without any
  depth check**:
  1. `QueryEntry::when` (Epic03/B) — compiled by `compile_filter` in `resolve_skip`
     (`query_runner.rs:156`); the planner only rejects *field-based comparisons*
     inside `when`, not depth.
  2. `GroupBy::having` — compiled at `aggregate.rs:1306`.
  3. `FilterValue` trees (`$cond`/`$expr`/`$fn` args/`Array`) nested inside a WHERE
     *value* — `check_filter_depth` treats `Filter::Eq{..}` as a leaf, so a depth-1
     `Eq` whose value is a 100k-deep `$cond`/`Array` chain passes the guard, then
     `resolve_filter_query` (`resolve.rs:272-431`) and `compile_filter` recurse
     unbounded at eval time — per row.
- **Failure scenario:** a client sends a Read whose `where` embeds a deeply nested
  `{"$cond": ...}` chain (or a deep `when`). The batch passes `validate_filter_depth`,
  then the recursive walk overflows the tokio worker stack → process abort, not a
  catchable `Err`. (The transport-layer serde recursion is the first line of defense,
  but it is equally unbounded and lives in sibling crates; this engine guard exists
  precisely as the second line — #670 even extended it to the interactive-tx path —
  and it silently covers only 3 of the reachable filter surfaces.)
- **Suggested fix:** (a) extend the collector in `validate_filter_depth` to include
  `entry.when` and `Read(q.group_by.and_then(having))`; (b) add an iterative
  `FilterValue`-tree depth walk (mirroring `prescan_filter`'s dispatch shape in
  `cond_cache.rs:104-150`) to `check_filter_depth` so value-nesting counts toward
  `MAX_FILTER_DEPTH`; (c) optionally make `compile_filter`/`resolve_filter_query`
  depth-bounded (return `FilterNode::False` / `None` past a cap) as a final backstop.

### 2. Per-row recompile of `$cond` conditions on the WHERE path — client-driven CPU amplification (incl. `Regex::new` per row)
- **File:line:** `crates/shamir-engine/src/query/filter/resolve.rs:397-403`;
  `cond_cache.rs:1-16` (module doc admits WHERE/`when`/write-value callers do not
  populate the cache); `compile.rs:101-108` (`Regex::new` inside `compile_filter`).
- **Severity:** medium
- **Issue:** when a WHERE clause's comparison value is a `$cond`
  (`FilterValue::Cond`), `resolve_filter_query`'s Cond arm calls
  `compile_filter(&cond.condition, ctx.interner)` **on every evaluation** — i.e. once
  per record scanned — because the #643 `CondCache` is only wired into
  `SelectProjection::new`. If the `$cond`'s condition contains a `Filter::Regex` or
  `Like` node, that is a full `Regex::new` compile per row. The Rust `regex` crate
  is linear-time at match (no ReDoS), but *compilation* is not free (tens of µs to
  ms for large patterns, default 10 MB compiled-program budget per pattern) and the
  #666 cooperative deadline only checkpoints **between ops**, never inside one.
- **Failure scenario:** one Read op over a large table with
  `where: {"op":"eq","field":"x","value":{"$cond":{"if":{"op":"regex",...},"then":1,"else":0}}}`
  recompiles the regex once per row — minutes of single-op CPU with no deadline
  trip; the op watchdog (`op_watchdog.rs`) only logs it afterwards. Repeat across
  connections for sustained amplification.
- **Suggested fix:** thread a `CondCache` through the WHERE compile path the same way
  `SelectProjection::new` does (prescan the compiled `FilterNode`'s embedded
  `FilterValue`s once per query), or cache the compiled `FilterNode` inside the
  `FilterNode::Eq/…` arm keyed by the (static-per-query) `&FilterValue` pointer —
  the same identity argument `CondCache` already documents.

### 3. Engine boundary performs no authorization — enforcement is a single upstream wrapper (`execute_as`), `trace_access` is observability only
- **File:line:** `crates/shamir-engine/src/query/batch/query_runner.rs:563-578`
  (explicit doc: `trace_access` "always `Ok`, NOT the enforcement gate");
  `batch_execute.rs:79-100` (public `execute_batch` takes an `Actor` but never checks
  it); `db_instance/db_instance.rs` (raw facade, no actor parameter at all).
- **Severity:** low (documented architecture; flagged as a boundary fragility)
- **Issue:** every public engine entry point (`execute_batch`, `execute_in_open_tx`,
  `DbInstance` methods) is a full-power API; DAC enforcement happens only if the
  embedding calls `ShamirDb::execute_as` first. The code comments this honestly and
  even warn future readers not to mistake `trace_access` for enforcement — but
  nothing structural prevents a new call path (a new server route, a WASM host
  bridge, an internal job) from skipping the wrapper and silently running as
  `Actor::System`. The only `Actor::System` hardcode in non-test engine code is
  inside the `#[cfg(test)]` `execute_batch_with_permissions`.
- **Suggested fix:** consider a type-level seam (e.g. engine executors take a
  `Authorized<BatchRequest>` token minted by the enforcement layer, or `trace_access`
  gains an enforcing sibling behind a feature flag), so "forgot the wrapper" becomes
  a compile error rather than a silent bypass.

### 4. Replication apply is a trusted raw write — no re-validation of leader events
- **File:line:** `crates/shamir-engine/src/tx/apply_replicated.rs:124-271` (raw
  `(key, value)` straight into `apply_committed_ops` / `base.transact`);
  module doc lines 4-9 state the trust model.
- **Severity:** low (explicitly documented design; the residual risk sits on sibling
  crates)
- **Issue:** the follower applies leader `ChangelogEvent`s with **no validators, no
  schema check, no DAC, no integrity check** on the payload — raw bytes go directly
  into the version-log of any table named in the event. The entire security of this
  path therefore rests on the replication transport being authenticated/integrity-
  protected (outside this crate). A compromised or spoofed upstream can plant
  arbitrary/corrupt record bytes that the follower then serves to its own clients.
- **Failure scenario:** unauthenticated replication endpoint (or a compromised peer
  in a chain — events are re-emitted downstream via `reproject_for_downstream`
  without any re-check) writes garbage or forged records; follower-side reads
  surface them (at best as `corrupt_records` refs) and downstream replicas chain-
  replicate the same bytes.
- **Suggested fix:** at minimum document this as a hard precondition on the
  transport crates in REPLICATION.md's threat model; consider an opt-in
  "validate-on-apply" mode (run record decode + schema/validator gates on follower
  ingest) for deployments that cannot fully trust the wire.

### 5. Pointer-keyed caches (`CondCache`, `FieldPathCache`, `QueryRefCache`) expose a public type alias whose safety invariant is documentation-only — stale *hit* hazard unaddressed
- **File:line:** `crates/shamir-engine/src/query/filter/cond_cache.rs:27-49`
  (`pub type CondCache = TMap<usize, Arc<FilterNode>>` keyed on
  `&*cond.condition as *const Filter as usize`); same pattern in
  `field_path_cache.rs` and `query_ref_cache.rs`.
- **Severity:** low
- **Issue:** the doc's safety analysis covers only the clone case (a cloned tree's
  nodes live at new addresses → cache *miss* → benign recompile). It does not cover
  **address reuse**: if the owning `Filter`/`FilterValue` tree is dropped while a
  cache built from it survives, a freshly allocated tree can land on the same
  addresses and the cache returns a **stale `FilterNode` for a different predicate**
  — a silent wrong-results failure (wrong rows returned/filtered), not a soft miss.
  Nothing in the type system ties cache lifetime to tree lifetime; the invariant is
  enforced only by a comment at current call sites.
- **Failure scenario:** a future caller caches across requests (natural temptation
  for a "compiled query cache") while request trees are dropped between uses;
  allocator reuse serves another query's compiled predicate → wrong data.
- **Suggested fix:** wrap the key in a newtype (`CondKey<'a>(&'a Filter)`) that
  borrows the tree, making "cache outlives tree" a compile error; or key on a hash
  of the filter tree instead of the address.

### 6. Client-supplied `Regex`/`Like` patterns: no size/length cap, and invalid patterns silently compile to `False`
- **File:line:** `crates/shamir-engine/src/query/filter/compile.rs:81-110`
  (`Regex::new(pattern)`; `Err(_) => FilterNode::False`; `None => FilterNode::False`),
  `fts.rs:6-25` (`like_pattern_to_regex`, `.ok()`).
- **Severity:** low
- **Issue:** (a) pattern length is unbounded — a repeated batch of ops each carrying
  a near-10 MB pattern (the regex crate's default compiled-size limit) burns
  seconds of compile CPU per op, again inside the no-checkpoint window of a single
  op (compounds finding 2). (b) An **invalid** pattern folds to `FilterNode::False`
  — fail-closed for a bare predicate, but `Not(<invalid regex>)` compiles to
  `True`, i.e. *matches everything*: a `DELETE ... WHERE NOT (regex typo)` deletes
  all rows with no error surfaced. The engine's convention elsewhere is that
  malformed client input is a hard `Err` (e.g. `WriteValueError::MalformedMarker`),
  so this silent fold is inconsistent.
- **Suggested fix:** reject invalid regex/like patterns at batch validation with a
  coded `BatchError` instead of folding to `False`; cap pattern length in
  `validate_filter_depth`'s pass (e.g. 64 KiB) like `MAX_FILTER_DEPTH` caps depth.

### 7. `SessionPermissions` RBAC remains publicly exported while being test-only scaffolding — plus a dead authorization loop inside it
- **File:line:** `crates/shamir-engine/src/query/auth/mod.rs:10` (unconditional
  `pub use session::SessionPermissions`), `session.rs:26-34` (doc: "test-only
  scaffolding … NOT wired into the server's live request path"),
  `session.rs:162-170` (first loop body is empty — dead code with an inline "we need
  a different approach" TODO), `batch/mod.rs:168-170` (only
  `execute_batch_with_permissions` is `#[cfg(test)]`-gated).
- **Severity:** low
- **Issue:** the non-enforcing permission type is part of the crate's public API, so
  a downstream embedder can reasonably construct `SessionPermissions` and believe it
  is the access model; its companion consumer is test-gated. The retained
  implementation also carries an unfinished half of `row_filter()` (the dead first
  loop), which invites "fixes" to the wrong loop. `SecretString` is likewise
  re-exported (`auth/mod.rs:11-14`) but never used in this crate (harmless —
  redaction/zeroize live in `shamir-types::secret`).
- **Suggested fix:** gate `SessionPermissions` behind `#[cfg(test)]` alongside
  `execute_batch_with_permissions` (breaking only for engine-internal tests), or
  move it to a `test-support` module; delete the dead loop.

## Positive observations (no action needed)

- **No `unsafe`** anywhere in `src/` (the only `unsafe impl` is the allocation
  counter in `examples/count_allocs_read_pipeline.rs`).
- **Fail-closed WASM validator bridge:** invocation/decode errors return
  `stop = true` with a `__wasm_err:` sentinel (`wasm_record_validator.rs:69-98`);
  actor identity is threaded into the guest `FnCtx`.
- **Corrupt-record hygiene:** undecodable rows are skipped and reported as
  `(table, id)` refs only — raw bytes never reach `QueryResult` (verified across
  `read_exec.rs`, `read_index_scan.rs`, `read_temporal.rs`).
- **DoS hardening that does exist:** `ABSOLUTE_MAX_FOR_EACH_ITERATIONS` server-side
  clamp over client-supplied `max_iterations` (#666/#653), `max_execution_time_secs: 0`
  clamped to a 1 s minimum rather than "no timeout", cooperative deadline replaces
  the cancel-unsafe `tokio::time::timeout`, #670 extended the depth guard to
  interactive tx, and `subscribe` filters are validated against an operator
  allow-list (`find_unsupported_subscription_filter`).
- **LIKE conversion escapes all regex metacharacters** (`fts.rs:16-19`) — no
  pattern-injection into the regex engine from LIKE patterns.
- **No secret/timing-sensitive comparisons** in this crate: password/PHC-string
  handling (`SecretString`, redacted `Debug`, zeroize) lives in `shamir-types` /
  `shamir-query-types`; `rand` is declared in `Cargo.toml` but unused in `src/`
  (no token/id generation here).

</details>
