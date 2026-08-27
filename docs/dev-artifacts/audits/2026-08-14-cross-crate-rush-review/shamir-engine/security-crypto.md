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
