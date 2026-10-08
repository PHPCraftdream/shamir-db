<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-client — correctness-tdd revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The principal lifecycle, reference-detection, dump, and multi-repo defects remain open. The atomic debug-overflow allegation is false; several coverage claims need qualification.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 14 | 13 | 0 | 0 | 1 | 0 | 0 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

<a id="review-1"></a>

### Claim 1 — Connection death never closes subscription channels -- `SubscriptionHandle::next()` hangs forever

Status: `confirmed-open`. Current risk: `high`.

Reader exit drains only pending requests. Handles retain the sender registry; after queued pushes drain, recv remains pending.

Evidence: [crates/shamir-client/src/client.rs:407](../../../../../crates/shamir-client/src/client.rs#L407); [crates/shamir-client/src/subscription.rs:36](../../../../../crates/shamir-client/src/subscription.rs#L36); [crates/shamir-client/src/subscription.rs:54](../../../../../crates/shamir-client/src/subscription.rs#L54).

Grouping/duplicate: `SUMMARY.md#1.1`. This row is not another independent defect.

<a id="review-2"></a>

### Claim 2 — `batch_has_refs` misses two ref carriers: `QueryEntry.when` guards and `BatchOp::ForEach`

Status: `confirmed-open`. Current risk: `high`.

The walk ignores entry.when and ForEach. IdBytes intermediates remain opaque to engine reference resolution; existing tests do not cover these combinations.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:513](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L513); [crates/shamir-engine/src/query/batch/query_runner.rs:136](../../../../../crates/shamir-engine/src/query/batch/query_runner.rs#L136); [crates/shamir-engine/src/query/batch/query_runner.rs:758](../../../../../crates/shamir-engine/src/query/batch/query_runner.rs#L758); [crates/shamir-query-types/src/read/query_record.rs:195](../../../../../crates/shamir-query-types/src/read/query_record.rs#L195).

Grouping/duplicate: `SUMMARY.md#1.2`. This row is not another independent defect.

<a id="review-3"></a>

### Claim 3 — `resume()` never verifies server identity -- `server_pub_key_pin()` returns unverified caller input

Status: `confirmed-open`. Current risk: `high`.

Accept-any-cert TLS precedes ticket disclosure; resume validates only session-id length and stores the supplied pin without authentication.

Evidence: [crates/shamir-client/src/client.rs:866](../../../../../crates/shamir-client/src/client.rs#L866); [crates/shamir-client/src/client.rs:890](../../../../../crates/shamir-client/src/client.rs#L890); [crates/shamir-client/src/client.rs:937](../../../../../crates/shamir-client/src/client.rs#L937); [crates/shamir-client/src/wire_frames.rs:51](../../../../../crates/shamir-client/src/wire_frames.rs#L51).

Grouping/duplicate: `SUMMARY.md#3.1`. This row is not another independent defect.

<a id="review-4"></a>

### Claim 4 — `subscribe_push` / early-buffer routing race strands pushes

Status: `confirmed-open`. Current risk: `medium`.

Registry lookup/insertion and early-buffer append/removal remain separate critical sections, permitting stranded pushes and flush-versus-direct-send reordering.

Evidence: [crates/shamir-client/src/client.rs:330](../../../../../crates/shamir-client/src/client.rs#L330); [crates/shamir-client/src/client.rs:349](../../../../../crates/shamir-client/src/client.rs#L349); [crates/shamir-client/src/client.rs:998](../../../../../crates/shamir-client/src/client.rs#L998).

Grouping/duplicate: `SUMMARY.md#1.3`. This row is not another independent defect.

<a id="review-5"></a>

### Claim 5 — `dump_repo`: failed dump poisons the OnceCell as "populated" and later calls never re-dump

Status: `confirmed-open`. Current risk: `medium`.

Both error branches resolve unit; get_or_init therefore initializes populated. Subsequent calls skip the closure and return success.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:191](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L191); [crates/shamir-client/src/interner_cache_ops.rs:211](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L211); [crates/shamir-client/src/interner_cache.rs:175](../../../../../crates/shamir-client/src/interner_cache.rs#L175).

Grouping/duplicate: `SUMMARY.md#1.4`. This row is not another independent defect.

<a id="review-6"></a>

### Claim 6 — Multi-repo v2 de-intern: first-match FieldMap can silently attach wrong field names

Status: `confirmed-open`. Current risk: `medium`.

Results discard alias-to-repo provenance and probe every candidate map. Overlapping repo-local IDs can resolve successfully using another repo's names.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:638](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L638); [crates/shamir-client/src/interner_cache_ops.rs:702](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L702); [crates/shamir-client/src/interner_cache_ops.rs:728](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L728).

Grouping/duplicate: `SUMMARY.md#1.5`. This row is not another independent defect.

<a id="review-7"></a>

### Claim 7 — Vacuous test: `atomic_u8_plumbing_stores_and_reads_correctly` tests std, not the client

Status: `confirmed-open`. Current risk: `low`.

The registered test constructs standalone atomics, never Client. V2 integration tests can early-return when version plumbing regresses.

Evidence: [crates/shamir-client/src/tests/wire_version_tests.rs:135](../../../../../crates/shamir-client/src/tests/wire_version_tests.rs#L135); [crates/shamir-client/src/tests/v2_passthrough_tests.rs:154](../../../../../crates/shamir-client/src/tests/v2_passthrough_tests.rs#L154); [crates/shamir-client/src/tests/mod.rs:9](../../../../../crates/shamir-client/src/tests/mod.rs#L9).

Grouping/duplicate: `SUMMARY.md#1.6`. This row is not another independent defect.

<a id="review-8"></a>

### Claim 8 — `get_ddl_op_status`: comment contradicts behavior; method has zero test coverage

Status: `confirmed-open`. Current risk: `low`.

The special-case arm is unreachable: roundtrip converts DbResponse::Error into Err(Db) before the method matches. No client test calls the method.

Evidence: [crates/shamir-client/src/client.rs:1207](../../../../../crates/shamir-client/src/client.rs#L1207); [crates/shamir-client/src/client.rs:1293](../../../../../crates/shamir-client/src/client.rs#L1293); [crates/shamir-client/src/tests/mod.rs:1](../../../../../crates/shamir-client/src/tests/mod.rs#L1).

Grouping/duplicate: `SUMMARY.md#1.7`. This row is not another independent defect.

<a id="review-9"></a>

### Claim 9 — Duplicate `subscribe_push(sub_id)`: dropping the first handle closes the second

Status: `confirmed-open`. Current risk: `low`.

Registration overwrites by sub_id; the stale handle's unconditional Drop removal deletes the replacement sender.

Evidence: [crates/shamir-client/src/client.rs:1000](../../../../../crates/shamir-client/src/client.rs#L1000); [crates/shamir-client/src/subscription.rs:59](../../../../../crates/shamir-client/src/subscription.rs#L59).

Grouping/duplicate: `SUMMARY.md#1.8`. This row is not another independent defect.

<a id="review-10"></a>

### Claim 10 — `collect_map_keys` skips `Value::Set` (asymmetric with `qv_has_fn_marker`)

Status: `confirmed-open`. Current risk: `low`.

Collection omits sets, while storage encoding recursively interns maps inside sets. Cold nested keys can fail despite pre-touch.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:468](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L468); [crates/shamir-client/src/interner_cache_ops.rs:499](../../../../../crates/shamir-client/src/interner_cache_ops.rs#L499); [crates/shamir-types/src/codecs/interned/messagepack.rs:965](../../../../../crates/shamir-types/src/codecs/interned/messagepack.rs#L965).

Grouping/duplicate: `SUMMARY.md#1.9`. This row is not another independent defect.

<a id="review-11"></a>

### Claim 11 — TDD-coverage gaps: untested public paths

Status: `confirmed-open`. Current risk: `low`.

Client-local tests still lack full resume, subscribe_push flushing, Client close/Drop reader-liveness, and when/ForEach smart-path coverage.

Evidence: [crates/shamir-client/src/tests/mod.rs:1](../../../../../crates/shamir-client/src/tests/mod.rs#L1); [crates/shamir-client/src/tests/resume_wire_tests.rs:1](../../../../../crates/shamir-client/src/tests/resume_wire_tests.rs#L1); [crates/shamir-client/src/tests/demux_tests.rs:314](../../../../../crates/shamir-client/src/tests/demux_tests.rs#L314); [crates/shamir-client/tests/batch_when_e2e.rs:219](../../../../../crates/shamir-client/tests/batch_when_e2e.rs#L219).

Grouping/duplicate: `SUMMARY.md#1.10`. This row is not another independent defect.

<a id="review-12"></a>

### Claim 12 — `connect_timeout_fires` depends on 10.255.255.1:9 being a silent black hole

Status: `confirmed-open`. Current risk: `nit`.

Both TCP timeout tests assume externally dependent SYN behavior. Fast refusal/unreachability invalidates their expected timeout outcomes.

Evidence: [crates/shamir-client/src/tests/timeout_tests.rs:26](../../../../../crates/shamir-client/src/tests/timeout_tests.rs#L26); [crates/shamir-client/src/tests/timeout_tests.rs:37](../../../../../crates/shamir-client/src/tests/timeout_tests.rs#L37); [crates/shamir-client/src/tests/timeout_tests.rs:67](../../../../../crates/shamir-client/src/tests/timeout_tests.rs#L67).

Grouping/duplicate: `SUMMARY.md#1.11`. This row is not another independent defect.

<a id="review-13"></a>

### Claim 13 — `close()` never sets the `closed` flag though `ConnectionClosed`'s doc lists "explicit close()" as a trigger

Status: `confirmed-open`. Current risk: `nit`.

Close aborts the reader and shuts down writing without updating closed. Consuming self makes this mainly bookkeeping/documentation inconsistency.

Evidence: [crates/shamir-client/src/client.rs:1306](../../../../../crates/shamir-client/src/client.rs#L1306); [crates/shamir-client/src/error.rs:41](../../../../../crates/shamir-client/src/error.rs#L41).

Grouping/duplicate: `SUMMARY.md#1.12`. This row is not another independent defect.

<a id="review-14"></a>

### Claim 14 — `next_request_id` u32 `fetch_add` overflow panics in debug builds

Status: `refuted`. Current risk: —.

AtomicU32::fetch_add has wrapping arithmetic in debug and release. The separate outstanding-rid collision risk remains, but this panic mechanism does not.

Evidence: [crates/shamir-client/src/client.rs:1259](../../../../../crates/shamir-client/src/client.rs#L1259); [crates/shamir-client/src/client.rs:1271](../../../../../crates/shamir-client/src/client.rs#L1271).

Grouping/duplicate: `SUMMARY.md#1.13`. This row is not another independent defect.

## Corrections and qualified non-findings

- No standalone Fix Plan or TASK_GROUPS is present; suggested fixes are evaluated through the finding rows and SUMMARY Fix Plan.
- Correctness #7 is a coverage weakness, not a demonstrated medium-severity runtime defect.
- For correctness #2, scanning entry.when is insufficient unless filter_has_refs also handles Filter::ValueCompare.left/right.
- An accepted-but-silent loopback listener cannot test connect_tcp's TCP-establishment timeout; use a controllable pending connect abstraction.
- The dump test starts with an already warm cache and cannot prove an actual dump populated it: crates/shamir-client/src/tests/interner_cache_tests.rs:194.
- The alleged cold-cache refresh test is masked by ambient delta merging before de-interning: crates/shamir-client/src/client.rs:1077 and crates/shamir-db/src/shamir_db/execute/ambient_interner.rs:30.
- A signed resume_ok received after ticket transmission would not prevent ticket theft; authenticate before disclosing the ticket.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-client -- Correctness & TDD-coverage

## Summary

The demux/timeout/wire-version unit tests and the live-server e2e suite (cursor, ambient
interner sync, v2 pass-through, batch cond/when/for-each) are genuinely strong and mostly
non-vacuous -- they drive `reader_task` and the timeout primitives directly and assert
real server-side effects. The correctness problems concentrate at the seams the tests do
not cover: subscription-channel lifecycle on connection death (a permanent hang, which
CLAUDE.md explicitly classifies as a bug, never a tolerance), the `batch_has_refs` guard
scanning only part of the ref-carrier surface it was written to protect, a
subscribe/early-buffer routing race that silently loses pushes, and `dump_repo`'s
OnceCell contract contradicting both its own docs and tokio's semantics. TDD-coverage
gaps are localized: one fully vacuous test, plus several untested public paths
(`resume` end-to-end, `get_ddl_op_status`, the `subscribe_push` flush path, reader-exit
with active subscriptions).

## Findings

### 1. Connection death never closes subscription channels -- `SubscriptionHandle::next()` hangs forever
- File:line: `crates/shamir-client/src/client.rs:318-327` (reader exit path), `crates/shamir-client/src/subscription.rs:54-65`
- Severity: high
- Issue: `reader_task`'s exit path marks `closed` and drains only the `pending` map. It
  never touches `subscriptions`. The registry entry is the SOLE holder of the mpsc
  `tx` (the local clone in `subscribe_push` is dropped on return; the handle holds only
  `rx`), and every live `SubscriptionHandle` keeps the registry `Arc` (and therefore the
  `tx`) alive. On server EOF / I/O error / TCP reset, a consumer blocked in
  `handle.next().await` pends forever; the documented contract "`None` if the
  subscription was closed" never fires.
- Failure scenario: server restarts mid-subscription; client's consumer loop
  `while let Some(env) = handle.next().await { .. }` hangs indefinitely. CLAUDE.md is
  explicit: "Hangs and test-locks are BUGS -- hunt and fix them, never tolerate."
- Suggested fix: on reader exit, `subscriptions.lock().clear()` (dropping all senders so
  every `rx.recv()` resolves to `None`), or select on the `closed` flag inside
  `SubscriptionHandle::next`. Add a Red test: reader_task EOF with a registered
  subscription must deliver `None` to `next()`.

### 2. `batch_has_refs` misses two ref carriers: `QueryEntry.when` guards and `BatchOp::ForEach`
- File:line: `crates/shamir-client/src/interner_cache_ops.rs:513-532` (`op_has_refs`), cross-ref `shamir-query-types/src/batch/query_entry.rs:55-62` and `shamir-query-types/src/batch/for_each_op.rs:20-38`
- Severity: high
- Issue: `op_has_refs` scans only the op's own filter, treats `Batch(_) => true`
  unconditionally, and falls to `_ => false` for everything else. Two documented
  ref-bearing shapes escape it: (a) `QueryEntry.when: Option<Filter>` -- the guard whose
  own doc says "only `$query`/`$fn`/`$param`/literals are meaningful" (its canonical use
  is "run this op iff `$query_ref_A >= $query_ref_B`"); (b) `BatchOp::ForEach`, whose
  `over: FilterValue` is canonically `over: $query @a[].id` and whose nested
  `batch.queries` body is never recursed -- despite ForEach being documented as
  "structurally a sibling of SubBatchOp" and despite the #660 precedent in
  `query_entry.rs` that a flat walk silently skips what nested bodies touch.
- Failure scenario: on a v2 server, `execute_with_touch` of a batch whose ops are
  when-guarded on earlier aliases, or that loops `ForEach` over a query result, passes
  `batch_has_refs == false` -> `result_encoding = Id` -> intermediates become opaque
  `QueryRecord::IdBytes` (`as_value() == Null`) -> `$query` path resolution silently
  breaks -- the exact finding-1.4 failure mode this guard was built to prevent (proven
  at engine level by `query_ref_does_not_resolve_under_id_encoding`). Note the existing
  e2e when/for-each tests all call `execute`, never `execute_with_touch`, so the broken
  combination is untested.
- Suggested fix: `batch_has_refs` should take `&QueryEntry` (not `&BatchOp`) and scan
  `entry.when` with `filter_has_refs`; add `BatchOp::ForEach(fe) => fv_has_refs(&fe.over)
  || fe.batch.queries.values().any(..)` (recurse). Add Red tests for both shapes in
  `batch_has_refs_tests.rs`, plus one e2e `execute_with_touch` when/for-each test.

### 3. `resume()` never verifies server identity -- `server_pub_key_pin()` returns unverified caller input
- File:line: `crates/shamir-client/src/client.rs:588-677` (esp. line 663 `pinned_hash: opts.pinned_hash`), `crates/shamir-client/src/wire_frames.rs:51-64`
- Severity: high (correctness-of-invariants; overlaps the security reviewer's theme)
- Issue: TLS is configured with `make_client_config_no_ca()` (no server cert
  verification); on `connect`, server identity is established solely at the app layer by
  `process_auth_ok` validating the pin/identity signature. `WireResumeOk` carries no
  `server_pub_key` / `identity_sig`, and `resume()` performs no verification at all: it
  stores the caller's `pinned_hash` verbatim. The `pinned_hash` field's documented
  invariant ("SHA256(server_ed25519_pub_key)... validated", `ConnectOptions` doc:
  "refuses on mismatch (spec ServerIdentityChanged)") is silently violated on the resume
  path, and `Client::server_pub_key_pin()` afterwards reports a pin that was never
  checked this session -- callers that persist the getter's output create a tautology.
- Failure scenario: active MITM (any self-signed cert is accepted at TLS layer)
  terminates the resumed connection, fabricates a `WireResumeOk` (arbitrary 32-byte
  `session_id`), and the client proceeds to send queries, with no refusal.
- Suggested fix: either carry and verify the server key/signature in `WireResumeOk`
  against `ResumeOptions::pinned_hash` (refuse on mismatch), or explicitly document that
  resume performs no server-identity validation and stop advertising
  `server_pub_key_pin()` as verified for resumed sessions. Add a Red test asserting a
  wrong-identity resume is rejected (or, if by design, that the getter is documented as
  pass-through).

### 4. `subscribe_push` / early-buffer routing race strands pushes
- File:line: `crates/shamir-client/src/client.rs:240-270` (reader routing) vs `722-750` (`subscribe_push`)
- Severity: medium
- Issue: the reader decides routing on the `subscriptions` map lookup, then (after
  releasing that lock) pushes to `early_buffer` on miss. `subscribe_push` inserts into
  the map and, in a SEPARATE critical section, drains the early buffer. Interleaving:
  reader map-lookup miss -> `subscribe_push` inserts -> `subscribe_push` buffer-remove
  (empty) -> reader buffer-push. The envelope is stranded in `early_buffer` forever --
  silently lost (only a later `subscribe_push` for the same id would flush it). This
  contradicts the module's own "no loss" claim (`demux_tests.rs:312`).
- Suggested fix: route under one lock acquisition (reader holds the `subscriptions` lock
  across the miss -> early-buffer append, with `subscribe_push` using the same lock
  order), or move the flush inside the map-insert critical section. Red test: concurrent
  push-then-subscribe loop asserting no envelope is stranded.

### 5. `dump_repo`: failed dump poisons the OnceCell as "populated" and later calls never re-dump
- File:line: `crates/shamir-client/src/interner_cache_ops.rs:180-223`, `crates/shamir-client/src/interner_cache.rs:35-38,161-176`
- Severity: medium
- Issue: the `ensure_populated` closure swallows all errors and always resolves `()`, so
  tokio's `OnceCell::get_or_init` initializes the cell even when the dump roundtrip or
  parse FAILED -- contradicting the `FieldMap` doc ("`populated` is set once the first
  full interner_dump succeeds") and `is_populated()`. Worse, because the cell is now
  initialized, `get_or_init` never runs the closure again: `dump_repo`'s own doc
  ("Subsequent calls re-dump unconditionally") is false -- every subsequent call is a
  silent `Ok(())` no-op. The inline comment "the cell stays uninitialized and a later
  call retries" describes behavior the code cannot have (the closure "cannot" return
  early precisely because it swallows the errors that would keep the cell clean).
- Failure scenario: transient network error on the first `dump_repo` -> cache empty,
  `is_populated() == true`, and `dump_repo` can never populate it; callers trusting the
  bool or the doc operate on a permanently cold map with a success return value.
- Suggested fix: use `get_or_try_init` (or an explicit flag set only after a successful
  merge) so failures leave the cell clean and retryable; return the error instead of
  swallowing it into `Ok(())` (CLAUDE.md error-handling: return `Result`, don't
  launder failures). Red tests: dump_repo failure then retry succeeds; second
  dump_repo after success actually re-dumps (or fix the doc).

### 6. Multi-repo v2 de-intern: first-match FieldMap can silently attach wrong field names
- File:line: `crates/shamir-client/src/interner_cache_ops.rs:695-736`
- Severity: medium
- Issue: `try_deintern_repos` returns the FIRST repo whose map resolves every id. Interner
  ids are small monotonic integers per repo, so cross-repo id overlap is the common case,
  not the edge: in a multi-repo batch, repo B's row can be de-interned with repo A's names
  and returned to the caller as if correct. The comment acknowledges the assumption
  ("best-effort first-match") but the disambiguating data is available and unused --
  `execute_with_touch` walks `entry.op.table_ref()` already and could pass a per-alias
  (or per-result) repo mapping instead of a flat repo list.
- Failure scenario: batch touching `repo_a` and `repo_b`, both with id 1 = different
  names; `repo_b`'s returned rows carry `repo_a`'s field names -- silent data mislabel.
- Suggested fix: resolve each result alias to its op's `table_ref().repo` and de-intern
  against that single map (falling back to the current probe only when unknown); keep the
  refresh-retry path.

### 7. Vacuous test: `atomic_u8_plumbing_stores_and_reads_correctly` tests std, not the client
- File:line: `crates/shamir-client/src/tests/wire_version_tests.rs:135-142`
- Severity: medium (this theme)
- Issue: the test's doc claims to verify "a Client whose WireAuthOk carries
  server_query_version=2 should expose that via server_query_version()", but the body
  constructs bare `std::sync::atomic::AtomicU8` values and asserts the standard library
  stores/loads them. It cannot fail due to any client bug -- the exact vacuous-test
  anti-pattern CLAUDE.md's Red/Green/Refactor protocol exists to prevent. The real
  plumbing (`connect`/`resume` -> `WireAuthOk.server_query_version` -> `AtomicU8` ->
  getter) is covered only incidentally by e2e guards (`server_query_version() < 2`
  early-returns), never asserted positively.
- Suggested fix: delete the test or replace it with one that drives `Client::connect`
  against a live server and asserts `server_query_version() == 2` (the harness already
  exists in `v2_passthrough_tests.rs`).

### 8. `get_ddl_op_status`: comment contradicts behavior; method has zero test coverage
- File:line: `crates/shamir-client/src/client.rs:913-953`
- Severity: low
- Issue: the inline comment says `not_supported` is treated "as 'feature unavailable'
  rather than a hard error", but the code returns `Err(ClientError::Protocol(..))` for it
  -- indistinguishable at the call site from a hard failure except by string matching the
  variant's payload; the method doc ("`None` if the operation is unknown (GC'd, never
  existed, or a pre-RFC op...)" suggests an old server should surface as `Ok(None)`. No
  unit or e2e test exercises any branch of this public method.
- Suggested fix: pick one contract -- `Ok(None)` for `not_supported` (matches the doc) or
  fix the comment -- and add a test for the found / unknown / not_supported branches.

### 9. Duplicate `subscribe_push(sub_id)`: dropping the first handle closes the second
- File:line: `crates/shamir-client/src/client.rs:722-750`, `crates/shamir-client/src/subscription.rs:59-65`
- Severity: low
- Issue: a second `subscribe_push` for the same id overwrites the registry entry,
  dropping the map's last clone of the first `tx` (first handle cleanly sees `None`).
  When the FIRST handle is later dropped, its `Drop` removes the registry key -- which
  now holds the SECOND handle's live sender. The second handle's channel closes
  prematurely (`next()` -> `None`) and all subsequent pushes strand in the early buffer.
- Suggested fix: guard against double registration (return an error / reuse the existing
  sender), or make removal conditional on identity (store an `Arc` token per handle and
  remove only if it is still the registered one).

### 10. `collect_map_keys` skips `Value::Set` (asymmetric with `qv_has_fn_marker`)
- File:line: `crates/shamir-client/src/interner_cache_ops.rs:468-480` vs `490-503`
- Severity: low
- Issue: the touch-collector recurses through `Value::Map` and `Value::List` only, while
  the `$fn` detector also recurses `Value::Set`. An INSERT record containing a set of
  maps registers the nested maps' keys neither for touch nor (therefore) for the v2
  id-keyed encode.
- Failure scenario: `execute_with_touch` of a record with a set-of-maps value on a v2
  server fails loudly with `Protocol("field '...' not in FieldMap -- touch_fields must be
  called first")` despite the pre-touch pass having run.
- Suggested fix: add a `Value::Set(items)` arm to `collect_map_keys` mirroring
  `qv_has_fn_marker`, and a unit test for the shape.

### 11. TDD-coverage gaps: untested public paths
- File:line: `crates/shamir-client/src/client.rs:588-677` (`resume`), `client.rs:722-750` (`subscribe_push` flush), `client.rs:1032-1050` (close/Drop), `crates/shamir-client/src/tests/resume_wire_tests.rs:1-89`
- Severity: low
- Issue: (a) `Client::resume` has only frame serde round-trip tests -- no live-server
  test in this crate drives the full TLS+resume+reader path (the happy-path claim of
  `ResumeOptions` is untested end-to-end); (b) the `subscribe_push` early-buffer flush
  path is never tested (demux tests insert senders into the map by hand); (c) the SS21
  contract "reader aborted on close()/Drop" has no test (e.g. leak/liveness assertion
  after dropping a Client with an open connection); (d) the `when`-guard x
  `execute_with_touch` combination (finding 2) is untested.
- Suggested fix: add Red tests for each before fixing the corresponding behavior.

### 12. `connect_timeout_fires` depends on 10.255.255.1:9 being a silent black hole
- File:line: `crates/shamir-client/src/tests/timeout_tests.rs:26,37-59`
- Severity: nit
- Issue: on networks/VPNs that answer the black-hole SYN with fast EHOSTUNREACH /
  ENETUNREACH, `connect_tcp` returns an io error well before the 250 ms budget and the
  `elapsed >= budget` assertion fails -- an environment-flaky test.
- Suggested fix: bind a local listener, accept nothing, and drop/never-read the socket
  (deterministic "accepted but silent" endpoint), or accept-and-hold the socket.

### 13. `close()` never sets the `closed` flag though `ConnectionClosed`'s doc lists "explicit close()" as a trigger
- File:line: `crates/shamir-client/src/client.rs:1028-1039`, `crates/shamir-client/src/error.rs:41-44`
- Severity: nit
- Issue: harmless today (`close` consumes `self`, so no caller can observe the flag
  afterward; waiters are released when the Client's `pending` map drops), but the doc and
  the flag disagree.
- Suggested fix: set `closed` in `close()` before shutdown (cheap, keeps the flag
  truthful for any future shared-handle design), or align the doc.

### 14. `next_request_id` u32 `fetch_add` overflow panics in debug builds
- File:line: `crates/shamir-client/src/client.rs:349,985`
- Severity: nit
- Issue: after 2^32-1 requests on one client, `fetch_add(1, Relaxed)` wraps (release) or
  panics ("attempt to add with overflow", debug). Practically unreachable; wrapping to
  rid 0 is harmless (rid is echoed opaquely and the pending map is keyed by it).
- Suggested fix: `fetch_update` with a documented wrap, or `Wrapping` semantics comment.


</details>
