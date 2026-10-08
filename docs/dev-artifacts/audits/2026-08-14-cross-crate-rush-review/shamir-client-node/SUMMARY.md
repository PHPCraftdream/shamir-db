<!-- revalidation:2026-10-08 source:92ad58266bf57ddea1fa3c8a47affba1a3a9a096 -->
# shamir-client-node — SUMMARY revalidation

Source snapshot: `92ad58266bf57ddea1fa3c8a47affba1a3a9a096`. Revalidated 2026-10-08 by read-only XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run; no source fix is part of this update. Test registration/assertions are evidence of an oracle, not proof of a passing run.

This section is authoritative for current status. Original titles/IDs are retained for traceability; a refuted title is not a current assertion. The collapsed historical report below is superseded, including its counts, severity, scenarios and fix instructions. Plan IDs preserve historical numbering, not a current release mandate. [Workspace methodology and status definitions](../SUMMARY.md#status-definitions).

The source-proven address, timeout, locking and API/documentation gaps remain. Parent exact-version dependency inspection refutes the alleged dead wrapper, and raw repl errors are intentional. Publication/FFI panic behavior remains unverified; registered coverage was inspected but not run.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 32 | 18 | 0 | 1 | 7 | 2 | 4 |

These are decisions on report claims, including repeated roots, bundled observations and non-findings—not a unique-bug census. Closed/N/A rows have no current risk; unverified risk is provisional. Pure style and unmeasured optimization claims do not establish runtime impact.

## Parent acceptance refinements

- The exact N-API factory source is available and supports receiver-sensitive construction; inheriting connect does not bypass the wrapper prototype.

<a id="review-1-1"></a>

### Claim 1.1 — Wrapper's `execute`/`repl`/typed-error overrides are unreachable: `connect()` returns native-class instances

Status: `refuted`. Current risk: —.

Parent inspection resolves the exact dependency mechanism. napi-derive 3.5.10's 5.1.2 backend retains CallbackInfo.this for an async factory and calls cb.factory; napi 3.10.5's _factory restores that receiver and passes it to napi_new_instance. Calling the inherited static method as the exported wrapper subclass therefore uses that subclass constructor, not a hardcoded native-class constructor. The alleged dead-wrapper/critical mechanism is refuted by source. No native artifact or e2e execution was used.

Evidence: [crates/shamir-client-node/wrapper.js:93](../../../../../crates/shamir-client-node/wrapper.js#L93); [crates/shamir-client-node/src/lib.rs:96](../../../../../crates/shamir-client-node/src/lib.rs#L96); [crates/shamir-client-node/Cargo.toml:39](../../../../../crates/shamir-client-node/Cargo.toml#L39); [tests/e2e/e2e.test.js:35](../../../../../tests/e2e/e2e.test.js#L35); [tests/e2e/tests/02-basic-crud.test.js:19](../../../../../tests/e2e/tests/02-basic-crud.test.js#L19).

Pinned dependency evidence: [napi 3.10.5, src/bindgen_runtime/callback_info.rs:27](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/callback_info.rs); [napi 3.10.5, src/bindgen_runtime/callback_info.rs:199](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/callback_info.rs); [napi-derive-backend 5.1.2, src/codegen/fn.rs:275](https://docs.rs/crate/napi-derive-backend/5.1.2/source/src/codegen/fn.rs); [napi-derive-backend 5.1.2, src/codegen/fn.rs:820](https://docs.rs/crate/napi-derive-backend/5.1.2/source/src/codegen/fn.rs).

<a id="review-1-2"></a>

### Claim 1.2 — Documented hostname usage can never connect: `host` is parsed as an IP literal

Status: `confirmed-open`. Current risk: `high`.

Host and port are still concatenated and parsed as SocketAddr without DNS resolution. Documented DNS names and unbracketed IPv6 such as ::1 fail before connection establishment.

Evidence: [crates/shamir-client-node/src/lib.rs:53](../../../../../crates/shamir-client-node/src/lib.rs#L53); [crates/shamir-client-node/src/lib.rs:100](../../../../../crates/shamir-client-node/src/lib.rs#L100); [crates/shamir-client/src/client.rs:60](../../../../../crates/shamir-client/src/client.rs#L60).

<a id="review-1-3"></a>

### Claim 1.3 — `ReplResponse::Error` is returned as success: wrapper marker checks `kind`, repl errors use `repl_kind`

Status: `refuted`. Current risk: —.

The tag distinction is real, but the existing API intentionally returns raw ReplResponse buffers, including its Error variant and fencing epoch. Typed exceptions are promised for DbResponse::Error, not every ReplResponse::Error. The registered bad_role test explicitly requires successful resolution with repl_kind:error and leader_epoch. Changing this would change the contract, not repair a proven violation.

Evidence: [crates/shamir-client-node/wrapper.d.ts:19](../../../../../crates/shamir-client-node/wrapper.d.ts#L19); [crates/shamir-client-node/wrapper.js:104](../../../../../crates/shamir-client-node/wrapper.js#L104); [crates/shamir-client/src/client.rs:1154](../../../../../crates/shamir-client/src/client.rs#L1154); [crates/shamir-query-types/src/wire/repl.rs:66](../../../../../crates/shamir-query-types/src/wire/repl.rs#L66); [tests/e2e/tests/16-replication.test.js:229](../../../../../tests/e2e/tests/16-replication.test.js#L229).

<a id="review-1-4"></a>

### Claim 1.4 — No automated tests anywhere; the "proof" script re-implements the logic it claims to prove

Status: `partially-fixed`. Current risk: `medium`.

Current automated coverage includes a wired nightly connect/CRUD/error suite, contradicting 'none anywhere'; this coverage predates the review, so it is not a newly discovered fix. The proof still copies marker detection, and the e2e error tests inspect message text rather than asserting actual ShamirDbError.code/retryable. Direct marker-byte compatibility coverage remains absent.

Evidence: [.github/workflows/ts-e2e-nightly.yml:162](../../../../../.github/workflows/ts-e2e-nightly.yml#L162); [tests/e2e/package.json:14](../../../../../tests/e2e/package.json#L14); [tests/e2e/helpers/runner.js:106](../../../../../tests/e2e/helpers/runner.js#L106); [tests/e2e/tests/09-errors.test.js:33](../../../../../tests/e2e/tests/09-errors.test.js#L33); [crates/shamir-client-node/proof-typed-errors.js:67](../../../../../crates/shamir-client-node/proof-typed-errors.js#L67).

<a id="review-1-5"></a>

### Claim 1.5 — `create_scram_user` wrapper msgpack-decodes 16 raw user_id bytes and discards the result

Status: `confirmed-open`. Current risk: `low`.

The success buffer contains raw user-id bytes, yet the override copies and attempts MessagePack decoding solely to detect an error marker, discarding successful decoded values and swallowing decode failures. This is structural overhead when the override is reached, not demonstrated latency or a current functional failure. Arbitrary bytes need not decode successfully.

Evidence: [crates/shamir-client-node/wrapper.js:123](../../../../../crates/shamir-client-node/wrapper.js#L123); [crates/shamir-client-node/wrapper.js:82](../../../../../crates/shamir-client-node/wrapper.js#L82); [crates/shamir-client-node/src/lib.rs:268](../../../../../crates/shamir-client-node/src/lib.rs#L268).

<a id="review-1-6"></a>

### Claim 1.6 — `execute` decodes the payload before the closed-check; decode errors mask "client closed"

Status: `confirmed-open`. Current risk: `low`.

Execute and repl still deserialize before checking the Option<Client>. Malformed input therefore wins over the closed-client diagnostic. This is an error-precedence choice; no documented precedence guarantee establishes a stronger correctness defect.

Evidence: [crates/shamir-client-node/src/lib.rs:205](../../../../../crates/shamir-client-node/src/lib.rs#L205); [crates/shamir-client-node/src/lib.rs:207](../../../../../crates/shamir-client-node/src/lib.rs#L207); [crates/shamir-client-node/src/lib.rs:233](../../../../../crates/shamir-client-node/src/lib.rs#L233); [crates/shamir-client-node/src/lib.rs:235](../../../../../crates/shamir-client-node/src/lib.rs#L235).

<a id="review-1-7"></a>

### Claim 1.7 — Version drift: Cargo `0.1.0-alpha.1` vs npm/generated-loader `0.1.0`

Status: `confirmed-open`. Current risk: `nit`.

The metadata divergence remains undocumented locally. The alleged loader failure is not caused by Cargo's version: enforcement compares the npm platform package's version against the generated npm version, and local native-file loads bypass that comparison.

Evidence: [crates/shamir-client-node/Cargo.toml:3](../../../../../crates/shamir-client-node/Cargo.toml#L3); [crates/shamir-client-node/package.json:3](../../../../../crates/shamir-client-node/package.json#L3); [crates/shamir-client-node/index.js:126](../../../../../crates/shamir-client-node/index.js#L126); [crates/shamir-client-node/index.js:132](../../../../../crates/shamir-client-node/index.js#L132).

<a id="review-2-1"></a>

### Claim 2.1 — One `tokio::sync::Mutex` held across every request serializes the demultiplexed client

Status: `confirmed-open`. Current risk: `medium`.

All request methods retain the binding mutex throughout their core roundtrip, structurally preventing concurrent requests on one binding instance despite core rid multiplexing. This queues that client's promises; it does not establish a Node event-loop freeze or measured throughput loss.

Evidence: [crates/shamir-client-node/src/lib.rs:82](../../../../../crates/shamir-client-node/src/lib.rs#L82); [crates/shamir-client-node/src/lib.rs:193](../../../../../crates/shamir-client-node/src/lib.rs#L193); [crates/shamir-client-node/src/lib.rs:207](../../../../../crates/shamir-client-node/src/lib.rs#L207); [crates/shamir-client-node/src/lib.rs:285](../../../../../crates/shamir-client-node/src/lib.rs#L285); [crates/shamir-client/src/client.rs:1233](../../../../../crates/shamir-client/src/client.rs#L1233).

<a id="review-2-2"></a>

### Claim 2.2 — Concurrent JS callers see stale pin/ticket snapshots only (correct), but `close()`'s take-then-await ordering can strand in-flight callers behind a permanent lock (see 4.1/6.1)

Status: `confirmed-open`. Current risk: `medium`.

Cached getters are immutable connect-time snapshots. Close must first acquire the same mutex held by an unbounded request and then retains it through core shutdown. A stalled preceding request can prevent close indefinitely. Moving shutdown outside the guard alone cannot bypass that preceding holder.

Evidence: [crates/shamir-client-node/src/lib.rs:140](../../../../../crates/shamir-client-node/src/lib.rs#L140); [crates/shamir-client-node/src/lib.rs:178](../../../../../crates/shamir-client-node/src/lib.rs#L178); [crates/shamir-client-node/src/lib.rs:207](../../../../../crates/shamir-client-node/src/lib.rs#L207); [crates/shamir-client-node/src/lib.rs:311](../../../../../crates/shamir-client-node/src/lib.rs#L311).

<a id="review-3-1"></a>

### Claim 3.1 — No timeouts surfaced and no cancellation: JS callers get the core SDK's known unbounded-hang class as the *only* behavior

Status: `confirmed-open`. Current risk: `high`.

Both core timeout options remain hardcoded None, with no binding cancellation API. An unresponsive configured peer can stall TLS/SCRAM or requests. The closed-check/register/drain race also remains structurally possible. Importantly, merely exposing existing knobs would not bound the full handshake, mutex acquisition, or request write: core connect_timeout covers TCP only and request_timeout starts after writing.

Evidence: [crates/shamir-client-node/src/lib.rs:130](../../../../../crates/shamir-client-node/src/lib.rs#L130); [crates/shamir-client-node/wrapper.d.ts:35](../../../../../crates/shamir-client-node/wrapper.d.ts#L35); [crates/shamir-client/src/client.rs:218](../../../../../crates/shamir-client/src/client.rs#L218); [crates/shamir-client/src/client.rs:479](../../../../../crates/shamir-client/src/client.rs#L479); [crates/shamir-client/src/client.rs:509](../../../../../crates/shamir-client/src/client.rs#L509); [crates/shamir-client/src/client.rs:1240](../../../../../crates/shamir-client/src/client.rs#L1240); [crates/shamir-client/src/client.rs:1267](../../../../../crates/shamir-client/src/client.rs#L1267); [crates/shamir-client/src/client.rs:1290](../../../../../crates/shamir-client/src/client.rs#L1290).

<a id="review-3-2"></a>

### Claim 3.2 — No `catch_unwind` discipline at the FFI boundary

Status: `unverified`. Current risk: `medium` (provisional; not a confirmed defect).

No catch_unwind attributes are present, but no concrete reachable panic or version-specific generated-boundary behavior was established. The cited core expect is protected: successful authentication either starts with a supplied pin or invokes the callback that stores one. Malformed MessagePack is handled through Result; attacker-triggered process abort and historical hung-promise claims remain unsupported.

Evidence: [crates/shamir-client-node/src/lib.rs:96](../../../../../crates/shamir-client-node/src/lib.rs#L96); [crates/shamir-client-node/src/lib.rs:205](../../../../../crates/shamir-client-node/src/lib.rs#L205); [crates/shamir-client/src/client.rs:610](../../../../../crates/shamir-client/src/client.rs#L610); [crates/shamir-client/src/client.rs:614](../../../../../crates/shamir-client/src/client.rs#L614); [crates/shamir-client/src/client.rs:631](../../../../../crates/shamir-client/src/client.rs#L631); [crates/shamir-connect/src/client/handshake.rs:266](../../../../../crates/shamir-connect/src/client/handshake.rs#L266); [crates/shamir-client-node/Cargo.toml:39](../../../../../crates/shamir-client-node/Cargo.toml#L39).

<a id="review-3-3"></a>

### Claim 3.3 — Secret copies in the napi struct are not zeroized (`resumption_ticket`, `session_id`, `pin`)

Status: `confirmed-open`. Current risk: `low`.

Ticket and session-id snapshots remain plain owned storage, retained even after close; the core ticket is separately Zeroizing. This concerns local memory disclosure/core dumps, not a demonstrated remote extraction path. The pin is a public-key hash, not a secret, and caller-owned JS Buffers can be overwritten although complete copy erasure cannot be guaranteed.

Evidence: [crates/shamir-client-node/src/lib.rs:85](../../../../../crates/shamir-client-node/src/lib.rs#L85); [crates/shamir-client-node/src/lib.rs:143](../../../../../crates/shamir-client-node/src/lib.rs#L143); [crates/shamir-client-node/src/lib.rs:311](../../../../../crates/shamir-client-node/src/lib.rs#L311); [crates/shamir-client/src/client.rs:436](../../../../../crates/shamir-client/src/client.rs#L436); [crates/shamir-client/src/client.rs:1098](../../../../../crates/shamir-client/src/client.rs#L1098); [docs/guide-docs/client-server-protocol-spec/SESSION_RESUMPTION.md:292](../../../../../docs/guide-docs/client-server-protocol-spec/SESSION_RESUMPTION.md#L292).

<a id="review-3-4"></a>

### Claim 3.4 — "Zeroised in the native side" overstates what the binding controls

Status: `confirmed-open`. Current risk: `nit`.

The eventual core password Vec is Zeroizing, but the documentation does not delimit that guarantee from JS/intermediate copies. Address or pin validation can also return before the password is wrapped. Exact transient napi-copy behavior was not verified.

Evidence: [crates/shamir-client-node/src/lib.rs:61](../../../../../crates/shamir-client-node/src/lib.rs#L61); [crates/shamir-client-node/src/lib.rs:98](../../../../../crates/shamir-client-node/src/lib.rs#L98); [crates/shamir-client-node/src/lib.rs:111](../../../../../crates/shamir-client-node/src/lib.rs#L111); [crates/shamir-client-node/src/lib.rs:127](../../../../../crates/shamir-client-node/src/lib.rs#L127); [crates/shamir-client-node/index.d.ts:76](../../../../../crates/shamir-client-node/index.d.ts#L76).

<a id="review-3-positive-notes"></a>

### Claim 3.Positive notes — Positive notes (kept for calibration parity)

Status: `not-applicable`. Current risk: —.

Source supports pre-handshake trustedPin length validation, delegation of binding authentication/crypto to Rust, MessagePack rather than JSON at the boundary, and an identical four-code retryable set. Successful TOFU capture is returned for caller-managed persistence; the binding itself does not persist it. These are scoped positive observations, not a full dependency or security certification.

Evidence: [crates/shamir-client-node/src/lib.rs:107](../../../../../crates/shamir-client-node/src/lib.rs#L107); [crates/shamir-client-node/src/lib.rs:136](../../../../../crates/shamir-client-node/src/lib.rs#L136); [crates/shamir-client-node/src/lib.rs:217](../../../../../crates/shamir-client-node/src/lib.rs#L217); [crates/shamir-client/src/client.rs:614](../../../../../crates/shamir-client/src/client.rs#L614); [crates/shamir-client-node/wrapper.js:39](../../../../../crates/shamir-client-node/wrapper.js#L39); [crates/shamir-client-ts/src/core/errors.ts:28](../../../../../crates/shamir-client-ts/src/core/errors.ts#L28).

<a id="review-4-1"></a>

### Claim 4.1 — `close()` blocks on the lock (and therefore on any in-flight request) instead of taking-and-releasing

Status: `confirmed-open`. Current risk: `medium`.

Close still queues behind request-held mutex guards and holds its own guard during shutdown. Its dependency on an unbounded request is source-proven; the report's operational timing and server-session consequences were not measured.

Evidence: [crates/shamir-client-node/src/lib.rs:207](../../../../../crates/shamir-client-node/src/lib.rs#L207); [crates/shamir-client-node/src/lib.rs:311](../../../../../crates/shamir-client-node/src/lib.rs#L311); [crates/shamir-client-node/src/lib.rs:133](../../../../../crates/shamir-client-node/src/lib.rs#L133).

Grouping/duplicate: `SUMMARY.md:2.2`. This row is not another independent defect.

<a id="review-4-2"></a>

### Claim 4.2 — Redundant decode + copy on every `repl`/`createScramUser` success, and a Uint8Array copy per `decodeOrThrow`

Status: `confirmed-open`. Current risk: `low`.

The override explicitly copies the entire Buffer through new Uint8Array(buf), then discards decoding results for repl/user creation. Installed MessagePack 3.1.3 decodes binary event blobs as subarray views, so it does not necessarily parse or copy the event contents again. Native Buffer construction copies and 'triple handling' latency were not proven. Partial overlap with 1.5, but the general response copy is broader.

Evidence: [crates/shamir-client-node/wrapper.js:82](../../../../../crates/shamir-client-node/wrapper.js#L82); [crates/shamir-client-node/wrapper.js:109](../../../../../crates/shamir-client-node/wrapper.js#L109); [crates/shamir-client-node/wrapper.js:123](../../../../../crates/shamir-client-node/wrapper.js#L123); [crates/shamir-query-types/src/wire/repl.rs:86](../../../../../crates/shamir-query-types/src/wire/repl.rs#L86); `tests/e2e/node_modules/@msgpack/msgpack/src/Decoder.ts:772` (local dependency evidence; not a committed file).

<a id="review-4-positive-notes"></a>

### Claim 4.Positive notes — Positive notes

Status: `not-applicable`. Current risk: —.

Request I/O is expressed as async Rust operations and the binding enables napi's async feature. However, synchronous wrapper encoding/decoding and copying execute on the JS thread, so 'nothing blocks the JS event loop' is too broad. Fixed arrays have constant-size copies; resumptionTicket clones a variable-length Vec. The number of serialization boundaries is visible, but 'priced correctly' has no measurement support.

Evidence: [crates/shamir-client-node/Cargo.toml:39](../../../../../crates/shamir-client-node/Cargo.toml#L39); [crates/shamir-client-node/src/lib.rs:192](../../../../../crates/shamir-client-node/src/lib.rs#L192); [crates/shamir-client-node/src/lib.rs:159](../../../../../crates/shamir-client-node/src/lib.rs#L159); [crates/shamir-client-node/src/lib.rs:178](../../../../../crates/shamir-client-node/src/lib.rs#L178); [crates/shamir-client-node/wrapper.js:82](../../../../../crates/shamir-client-node/wrapper.js#L82); [crates/shamir-client-node/wrapper.js:99](../../../../../crates/shamir-client-node/wrapper.js#L99).

<a id="review-5-1"></a>

### Claim 5.1 — *(primary: same as 1.3)* — repl error channel drift: the binding flattens `ReplResponse::Error` into a success buffer, breaking the wire's error taxonomy at the JS boundary

Status: `refuted`. Current risk: —.

The wire Error variant, code, message, and leader_epoch are preserved in the returned raw buffer. Existing documentation and the registered denial test distinguish this protocol result from DbResponse::Error exceptions; no flattening or lost taxonomy is demonstrated.

Evidence: [crates/shamir-client-node/src/lib.rs:239](../../../../../crates/shamir-client-node/src/lib.rs#L239); [crates/shamir-client-node/wrapper.d.ts:19](../../../../../crates/shamir-client-node/wrapper.d.ts#L19); [crates/shamir-query-types/src/wire/repl.rs:97](../../../../../crates/shamir-query-types/src/wire/repl.rs#L97); [tests/e2e/tests/16-replication.test.js:235](../../../../../tests/e2e/tests/16-replication.test.js#L235).

Grouping/duplicate: `SUMMARY.md:1.3`. This row is not another independent defect.

<a id="review-5-2"></a>

### Claim 5.2 — "Mirrors the Rust SDK 1:1" is false; resumption ticket getters dead-end with no resume path

Status: `confirmed-open`. Current risk: `medium`.

The binding still advertises 1:1 parity but lacks resume, connect_local, push subscription, cursor streaming, and server_query_version, while exporting ticket getters. No ticket input exists. The report additionally misquotes ticket getter documentation: persistence instructions belong to the pin getter, not the ticket getter.

Evidence: [crates/shamir-client-node/src/lib.rs:3](../../../../../crates/shamir-client-node/src/lib.rs#L3); [crates/shamir-client-node/src/lib.rs:176](../../../../../crates/shamir-client-node/src/lib.rs#L176); [crates/shamir-client-node/wrapper.d.ts:35](../../../../../crates/shamir-client-node/wrapper.d.ts#L35); [crates/shamir-client-node/wrapper.d.ts:49](../../../../../crates/shamir-client-node/wrapper.d.ts#L49); [crates/shamir-client/src/client.rs:862](../../../../../crates/shamir-client/src/client.rs#L862); [crates/shamir-client/src/client.rs:987](../../../../../crates/shamir-client/src/client.rs#L987); [crates/shamir-client/src/client.rs:996](../../../../../crates/shamir-client/src/client.rs#L996).

<a id="review-5-3"></a>

### Claim 5.3 — `execute(object)` teaches hand-assembled wire shapes; no builder, no exported BatchRequest/BatchResponse types

Status: `confirmed-open`. Current risk: `medium`.

The binding's declaration remains object→object and its example hand-assembles queries, without exported batch types or a documented builder integration. However, TS builders produce ordinary wire objects usable as execute inputs; existing Node tests demonstrate that integration structurally. Batch.execute itself requires executeWithTouch, which this binding lacks. Omitting return_all is intentional and valid, not a failure.

Evidence: [crates/shamir-client-node/src/lib.rs:19](../../../../../crates/shamir-client-node/src/lib.rs#L19); [crates/shamir-client-node/wrapper.d.ts:58](../../../../../crates/shamir-client-node/wrapper.d.ts#L58); [crates/shamir-client-ts/src/core/builders/batch.ts:334](../../../../../crates/shamir-client-ts/src/core/builders/batch.ts#L334); [crates/shamir-client-ts/src/core/builders/batch.ts:370](../../../../../crates/shamir-client-ts/src/core/builders/batch.ts#L370); [tests/e2e/tests/02-basic-crud.test.js:32](../../../../../tests/e2e/tests/02-basic-crud.test.js#L32); [crates/shamir-query-types/src/batch/batch_request.rs:90](../../../../../crates/shamir-query-types/src/batch/batch_request.rs#L90); [CLAUDE.md:546](../../../../../CLAUDE.md#L546).

<a id="review-5-4"></a>

### Claim 5.4 — `set_replicator` success buffer is fabricated from caller inputs, not the server echo

Status: `confirmed-open`. Current risk: `low`.

The core discards echoed fields and the binding reconstructs them from input, so the structural observation remains. Today's server explicitly echoes exactly those inputs, and the wrapper's public result is void. Canonical-name corruption is hypothetical, not a current defect; any action is primarily documentation hygiene.

Evidence: [crates/shamir-client/src/client.rs:1147](../../../../../crates/shamir-client/src/client.rs#L1147); [crates/shamir-client-node/src/lib.rs:291](../../../../../crates/shamir-client-node/src/lib.rs#L291); [crates/shamir-client-node/src/lib.rs:297](../../../../../crates/shamir-client-node/src/lib.rs#L297); [crates/shamir-server/src/db_handler/admin.rs:393](../../../../../crates/shamir-server/src/db_handler/admin.rs#L393); [crates/shamir-client-node/wrapper.js:148](../../../../../crates/shamir-client-node/wrapper.js#L148); [crates/shamir-client-node/wrapper.d.ts:66](../../../../../crates/shamir-client-node/wrapper.d.ts#L66).

<a id="review-5-5"></a>

### Claim 5.5 — Generated loader's platform packages are unpublished/undeclared: every non-win32-x64-msvc path is dead

Status: `unverified`. Current risk: `low` (provisional; not a confirmed defect).

Six build targets and no optionalDependencies are confirmed, but publication state and packed artifacts were not inspected. Every supported loader branch first accepts a locally built native file, so non-Windows paths are not intrinsically dead. No native binary is committed or currently present in the binding directory, including the supposedly shipped MSVC binary.

Evidence: [crates/shamir-client-node/package.json:20](../../../../../crates/shamir-client-node/package.json#L20); [crates/shamir-client-node/package.json:33](../../../../../crates/shamir-client-node/package.json#L33); [crates/shamir-client-node/index.js:126](../../../../../crates/shamir-client-node/index.js#L126); [crates/shamir-client-node/index.js:193](../../../../../crates/shamir-client-node/index.js#L193); [crates/shamir-client-node/index.js:283](../../../../../crates/shamir-client-node/index.js#L283); [.gitignore:28](../../../../../.gitignore#L28); [tests/e2e/README.md:39](../../../../../tests/e2e/README.md#L39).

<a id="review-6-1"></a>

### Claim 6.1 — *(primary: same as 2.2/4.1)* — `close()` is not a lifecycle-safe operation under stalls

Status: `confirmed-open`. Current risk: `medium`.

A request can hold the sole binding mutex indefinitely, preventing close from reaching core shutdown. Idempotence after successful completion does not provide shutdown responsiveness under stalls.

Evidence: [crates/shamir-client-node/src/lib.rs:207](../../../../../crates/shamir-client-node/src/lib.rs#L207); [crates/shamir-client-node/src/lib.rs:308](../../../../../crates/shamir-client-node/src/lib.rs#L308); [crates/shamir-client-node/src/lib.rs:311](../../../../../crates/shamir-client-node/src/lib.rs#L311).

Grouping/duplicate: `SUMMARY.md:2.2`. This row is not another independent defect.

<a id="review-6-2"></a>

### Claim 6.2 — Infrastructure errors lose all taxonomy crossing the boundary

Status: `confirmed-open`. Current risk: `low`.

infra_error still converts every ClientError variant to display text through Error::from_reason, discarding variant-specific identity and structured payloads. The report overstates this as absence of any name/code: a generic JS/napi error identity is different from preserving ClientError taxonomy.

Evidence: [crates/shamir-client-node/src/lib.rs:351](../../../../../crates/shamir-client-node/src/lib.rs#L351); [crates/shamir-client/src/error.rs:5](../../../../../crates/shamir-client/src/error.rs#L5); [crates/shamir-client/src/error.rs:44](../../../../../crates/shamir-client/src/error.rs#L44); [crates/shamir-client/src/error.rs:54](../../../../../crates/shamir-client/src/error.rs#L54).

<a id="review-6-3"></a>

### Claim 6.3 — No Drop/finalization: a GC'd-without-close client leaks the TCP connection and server session until expiry

Status: `refuted`. Current risk: —.

Dropping the binding's owned Option<Client> invokes core Client::Drop, which aborts the reader task; the writer is an owned field, not a detached socket owner. This cleanup predates the review. Exact napi GC timing is unverified, but the asserted absence of core cleanup is false. Server teardown also does not evict the session immediately after explicit close, so retained session state cannot be attributed specifically to missing binding Drop.

Evidence: [crates/shamir-client-node/src/lib.rs:82](../../../../../crates/shamir-client-node/src/lib.rs#L82); [crates/shamir-client/src/client.rs:1316](../../../../../crates/shamir-client/src/client.rs#L1316); [crates/shamir-client/src/client.rs:649](../../../../../crates/shamir-client/src/client.rs#L649); [crates/shamir-server/src/connection/request_loop.rs:410](../../../../../crates/shamir-server/src/connection/request_loop.rs#L410); [crates/shamir-connect/src/server/session.rs:539](../../../../../crates/shamir-connect/src/server/session.rs#L539).

<a id="review-6-4"></a>

### Claim 6.4 — `encode_db_error`'s own failure path degrades to a plain Error (acceptable, but untested and undocumented)

Status: `not-applicable`. Current risk: —.

The explicit serialization-error fallback remains, but the encoded value contains only owned Strings and no ordinary reachable serializer failure was established. This accepted fallback is not an actionable runtime defect; tests should prioritize real marker compatibility rather than manufacturing an unreachable branch.

Evidence: [crates/shamir-client-node/src/lib.rs:361](../../../../../crates/shamir-client-node/src/lib.rs#L361); [crates/shamir-query-types/src/wire/db_message.rs:316](../../../../../crates/shamir-query-types/src/wire/db_message.rs#L316).

<a id="review-7-1"></a>

### Claim 7.1 — `src/lib.rs` carries multiple primary exports; error-mapping helpers belong in a sibling file

Status: `refuted`. Current risk: —.

CLAUDE permits a closely coupled group in one file. ConnectOptions, ShamirClient, and its private boundary helpers form such a group; private helper functions are not unrelated primary public exports. Splitting is optional organization, not a demonstrated convention violation. Stale decoder references remain separately covered by 7.4.

Evidence: [CLAUDE.md:505](../../../../../CLAUDE.md#L505); [crates/shamir-client-node/src/lib.rs:52](../../../../../crates/shamir-client-node/src/lib.rs#L52); [crates/shamir-client-node/src/lib.rs:81](../../../../../crates/shamir-client-node/src/lib.rs#L81); [crates/shamir-client-node/src/lib.rs:351](../../../../../crates/shamir-client-node/src/lib.rs#L351); [crates/shamir-client-node/src/lib.rs:361](../../../../../crates/shamir-client-node/src/lib.rs#L361).

<a id="review-7-2"></a>

### Claim 7.2 — Test layout violates the repo convention: no `tests/` directory, no automated runner

Status: `refuted`. Current risk: —.

An automated Node runner exists and is wired into nightly CI. The Rust layout rule applies to modules containing tests; absence of Rust unit tests is a coverage gap, not proof of an incorrectly laid-out test module. Targeted typed-marker coverage remains open under 1.4/P1.8.

Evidence: [tests/e2e/helpers/runner.js:106](../../../../../tests/e2e/helpers/runner.js#L106); [tests/e2e/e2e.test.js:51](../../../../../tests/e2e/e2e.test.js#L51); [.github/workflows/ts-e2e-nightly.yml:162](../../../../../.github/workflows/ts-e2e-nightly.yml#L162); [CLAUDE.md:573](../../../../../CLAUDE.md#L573); [crates/shamir-client-node/src/lib.rs:366](../../../../../crates/shamir-client-node/src/lib.rs#L366).

<a id="review-7-3"></a>

### Claim 7.3 — Sanctioned-exception comment present at `repl` but missing at the identical `execute` rmp-serde boundary

Status: `confirmed-open`. Current risk: `nit`.

The repl boundary explains its raw-serde exception while execute does not. Both deserialize incoming bytes rather than constructing queries; the missing explanation is comment hygiene, not a runtime defect.

Evidence: [crates/shamir-client-node/src/lib.rs:205](../../../../../crates/shamir-client-node/src/lib.rs#L205); [crates/shamir-client-node/src/lib.rs:231](../../../../../crates/shamir-client-node/src/lib.rs#L231); [CLAUDE.md:554](../../../../../CLAUDE.md#L554).

<a id="review-7-4"></a>

### Claim 7.4 — Stale/self-contradictory naming: comments say the wrapper is `index.js`; it is `wrapper.js`

Status: `confirmed-open`. Current risk: `nit`.

The proof and native implementation still identify index.js as the decoder, while package.json selects wrapper.js and index.js is only the generated loader.

Evidence: [crates/shamir-client-node/proof-typed-errors.js:10](../../../../../crates/shamir-client-node/proof-typed-errors.js#L10); [crates/shamir-client-node/proof-typed-errors.js:67](../../../../../crates/shamir-client-node/proof-typed-errors.js#L67); [crates/shamir-client-node/src/lib.rs:213](../../../../../crates/shamir-client-node/src/lib.rs#L213); [crates/shamir-client-node/src/lib.rs:336](../../../../../crates/shamir-client-node/src/lib.rs#L336); [crates/shamir-client-node/package.json:5](../../../../../crates/shamir-client-node/package.json#L5).

<a id="review-executive-summary"></a>

### Claim Executive summary — Executive summary

Status: `refuted`. Current risk: —.

DNS/IPv6 and absent timeout concerns remain. The categorical dead-wrapper blocker is refuted by receiver-sensitive pinned N-API factory code, and the repl-error blocker contradicts the deliberate raw-response contract. No blanket shippability verdict or execution success follows.

Evidence: [crates/shamir-client-node/src/lib.rs:100](../../../../../crates/shamir-client-node/src/lib.rs#L100); [crates/shamir-client-node/src/lib.rs:132](../../../../../crates/shamir-client-node/src/lib.rs#L132); [crates/shamir-client-node/wrapper.d.ts:19](../../../../../crates/shamir-client-node/wrapper.d.ts#L19); [tests/e2e/tests/16-replication.test.js:235](../../../../../tests/e2e/tests/16-replication.test.js#L235).

Pinned dependency evidence: [napi 3.10.5, src/bindgen_runtime/callback_info.rs:27](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/callback_info.rs); [napi 3.10.5, src/bindgen_runtime/callback_info.rs:199](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/callback_info.rs); [napi-derive-backend 5.1.2, src/codegen/fn.rs:275](https://docs.rs/crate/napi-derive-backend/5.1.2/source/src/codegen/fn.rs); [napi-derive-backend 5.1.2, src/codegen/fn.rs:820](https://docs.rs/crate/napi-derive-backend/5.1.2/source/src/codegen/fn.rs).

<a id="review-finding-counts"></a>

### Claim Finding counts — Finding counts

Status: `not-applicable`. Current risk: —.

The original arithmetic correctly counts 28 lens rows and 25 groups using its three duplicate reductions. It is not a current verified-defect census: several rows are refuted, unverified, accepted fallbacks, or documentation-only observations. No source-proven critical defect was established.

Evidence: [docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-client-node/SUMMARY.md:475](../../../../../docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-client-node/SUMMARY.md) (line at source snapshot `92ad5826`, before this revalidation prefix).

## Current fix-plan state

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 18 | 13 | 0 | 0 | 0 | 2 | 3 |

A source-fixed item closes only its stated mechanism. Partial items retain the obligations named below; proposed fixes must obey the corrections and current contracts, not merely copy the historical recipe.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `not-applicable`. Current risk: —.

The proposed static override is not required to repair the alleged prototype defect: pinned factory code creates through the JS receiver. A direct public-wrapper/prototype regression is still useful coverage, but its absence does not establish a shipping blocker.

Evidence: [crates/shamir-client-node/wrapper.js:93](../../../../../crates/shamir-client-node/wrapper.js#L93); [crates/shamir-client-node/Cargo.toml:39](../../../../../crates/shamir-client-node/Cargo.toml#L39); [tests/e2e/e2e.test.js:35](../../../../../tests/e2e/e2e.test.js#L35); [tests/e2e/tests/02-basic-crud.test.js:19](../../../../../tests/e2e/tests/02-basic-crud.test.js#L19).

Pinned dependency evidence: [napi 3.10.5, src/bindgen_runtime/callback_info.rs:27](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/callback_info.rs); [napi 3.10.5, src/bindgen_runtime/callback_info.rs:199](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/callback_info.rs); [napi-derive-backend 5.1.2, src/codegen/fn.rs:275](https://docs.rs/crate/napi-derive-backend/5.1.2/source/src/codegen/fn.rs); [napi-derive-backend 5.1.2, src/codegen/fn.rs:820](https://docs.rs/crate/napi-derive-backend/5.1.2/source/src/codegen/fn.rs).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `not-applicable`. Current risk: —.

The proposed repl-error conversion contradicts the existing raw-buffer result contract and registered bad_role test. Preserve leader_epoch and distinguish protocol Error results from DbResponse::Error exceptions unless deliberately changing the API.

Evidence: [crates/shamir-client-node/wrapper.d.ts:19](../../../../../crates/shamir-client-node/wrapper.d.ts#L19); [crates/shamir-query-types/src/wire/repl.rs:97](../../../../../crates/shamir-query-types/src/wire/repl.rs#L97); [tests/e2e/tests/16-replication.test.js:235](../../../../../tests/e2e/tests/16-replication.test.js#L235).

<a id="plan-p0-3"></a>

### Plan P0.3 — P0.3

Status: `confirmed-open`. Current risk: —.

Neither IpAddr-based IPv6 construction nor DNS lookup exists. Host parsing and its documented behavior still need alignment.

Evidence: [crates/shamir-client-node/src/lib.rs:53](../../../../../crates/shamir-client-node/src/lib.rs#L53); [crates/shamir-client-node/src/lib.rs:100](../../../../../crates/shamir-client-node/src/lib.rs#L100).

<a id="plan-p0-4"></a>

### Plan P0.4 — P0.4

Status: `confirmed-open`. Current risk: —.

Timeout fields remain absent and both core options are None. Exposing the existing knobs is only partial mitigation: full handshake, request lock acquisition, and writes remain outside their current bounds.

Evidence: [crates/shamir-client-node/wrapper.d.ts:35](../../../../../crates/shamir-client-node/wrapper.d.ts#L35); [crates/shamir-client-node/src/lib.rs:132](../../../../../crates/shamir-client-node/src/lib.rs#L132); [crates/shamir-client/src/client.rs:478](../../../../../crates/shamir-client/src/client.rs#L478); [crates/shamir-client/src/client.rs:1277](../../../../../crates/shamir-client/src/client.rs#L1277); [crates/shamir-client/src/client.rs:1290](../../../../../crates/shamir-client/src/client.rs#L1290).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `confirmed-open`. Current risk: —.

Close still retains its guard during shutdown. Taking-and-releasing fixes that holding scope but cannot let close bypass a request already holding the same mutex; coordinate with P1.6. Core reader abort also does not execute its normal EOF drain, so the proposed guaranteed ConnectionClosed delivery requires additional lifecycle design.

Evidence: [crates/shamir-client-node/src/lib.rs:311](../../../../../crates/shamir-client-node/src/lib.rs#L311); [crates/shamir-client-node/src/lib.rs:207](../../../../../crates/shamir-client-node/src/lib.rs#L207); [crates/shamir-client/src/client.rs:407](../../../../../crates/shamir-client/src/client.rs#L407); [crates/shamir-client/src/client.rs:1308](../../../../../crates/shamir-client/src/client.rs#L1308).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`. Current risk: —.

The binding still holds its exclusive mutex across roundtrips. Clone-out/shared ownership needs a close mechanism that works with active references; Arc::try_unwrap alone cannot reliably close while callers retain Arcs.

Evidence: [crates/shamir-client-node/src/lib.rs:82](../../../../../crates/shamir-client-node/src/lib.rs#L82); [crates/shamir-client-node/src/lib.rs:215](../../../../../crates/shamir-client-node/src/lib.rs#L215); [crates/shamir-client/src/client.rs:1306](../../../../../crates/shamir-client/src/client.rs#L1306).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `unverified`. Current risk: —.

No catch_unwind attributes were added. Before treating blanket annotations as a required fix, establish pinned napi async/sync panic behavior and an actual reachable panic. The cited pin expect is source-protected by successful handshake processing.

Evidence: [crates/shamir-client-node/src/lib.rs:96](../../../../../crates/shamir-client-node/src/lib.rs#L96); [crates/shamir-client-node/Cargo.toml:39](../../../../../crates/shamir-client-node/Cargo.toml#L39); [crates/shamir-client/src/client.rs:631](../../../../../crates/shamir-client/src/client.rs#L631); [crates/shamir-connect/src/client/handshake.rs:266](../../../../../crates/shamir-connect/src/client/handshake.rs#L266).

<a id="plan-p1-8"></a>

### Plan P1.8 — P1.8

Status: `confirmed-open`. Current risk: —.

Direct typed-marker and Rust→JS byte-compatibility tests remain absent; the copied proof remains unsuitable as the production-wrapper oracle. Reuse the existing wired e2e lane and assert actual error properties. Exporting an internal decoder is not required if public wrapper methods are tested.

Evidence: [crates/shamir-client-node/proof-typed-errors.js:67](../../../../../crates/shamir-client-node/proof-typed-errors.js#L67); [crates/shamir-client-node/package.json:29](../../../../../crates/shamir-client-node/package.json#L29); [tests/e2e/tests/09-errors.test.js:33](../../../../../tests/e2e/tests/09-errors.test.js#L33); [.github/workflows/ts-e2e-nightly.yml:162](../../../../../.github/workflows/ts-e2e-nightly.yml#L162).

<a id="plan-p1-9"></a>

### Plan P1.9 — P1.9

Status: `confirmed-open`. Current risk: —.

Infrastructure errors still carry display text rather than stable variant-specific codes or structured causes.

Evidence: [crates/shamir-client-node/src/lib.rs:351](../../../../../crates/shamir-client-node/src/lib.rs#L351); [crates/shamir-client/src/error.rs:5](../../../../../crates/shamir-client/src/error.rs#L5).

<a id="plan-p2-10"></a>

### Plan P2.10 — P2.10

Status: `confirmed-open`. Current risk: —.

Discarded decoding and explicit Buffer copies remain. Passing the Buffer directly avoids the copy; merely renaming a try-decode helper hasDbErrorMarker does not eliminate decoding/materialization. Any protocol redesign must also distinguish raw user-id success bytes without changing intentional repl Error results.

Evidence: [crates/shamir-client-node/wrapper.js:82](../../../../../crates/shamir-client-node/wrapper.js#L82); [crates/shamir-client-node/wrapper.js:112](../../../../../crates/shamir-client-node/wrapper.js#L112); [crates/shamir-client-node/wrapper.js:126](../../../../../crates/shamir-client-node/wrapper.js#L126); `tests/e2e/node_modules/@msgpack/msgpack/src/utils/typedArrays.ts:7` (local dependency evidence; not a committed file).

<a id="plan-p2-11"></a>

### Plan P2.11 — P2.11

Status: `not-applicable`. Current risk: —.

Core Client::Drop already aborts its reader, and the binding owns that client. An additional async Drop-close task is not required to fix the alleged detached connection leak. Explicit close remains the clean shutdown path; exact napi finalization timing is unverified.

Evidence: [crates/shamir-client-node/src/lib.rs:82](../../../../../crates/shamir-client-node/src/lib.rs#L82); [crates/shamir-client/src/client.rs:1316](../../../../../crates/shamir-client/src/client.rs#L1316); [crates/shamir-client/src/client.rs:1306](../../../../../crates/shamir-client/src/client.rs#L1306).

<a id="plan-p2-12"></a>

### Plan P2.12 — P2.12

Status: `confirmed-open`. Current risk: —.

Neither resume nor server_query_version is exposed and the 1:1 claim remains. Documenting the supported subset is a valid scoped resolution; surfacing resume requires a separately verified identity and ticket contract.

Evidence: [crates/shamir-client-node/src/lib.rs:3](../../../../../crates/shamir-client-node/src/lib.rs#L3); [crates/shamir-client-node/wrapper.d.ts:49](../../../../../crates/shamir-client-node/wrapper.d.ts#L49); [crates/shamir-client/src/client.rs:862](../../../../../crates/shamir-client/src/client.rs#L862); [crates/shamir-client/src/client.rs:987](../../../../../crates/shamir-client/src/client.rs#L987).

<a id="plan-p2-13"></a>

### Plan P2.13 — P2.13

Status: `confirmed-open`. Current risk: —.

Batch types and a Node-facing builder recipe remain missing. TS Batch.build output is already a compatible plain wire object; document execute(db, batch.build()) rather than claiming class identity prevents integration or promising Batch.execute compatibility without executeWithTouch.

Evidence: [crates/shamir-client-node/wrapper.d.ts:58](../../../../../crates/shamir-client-node/wrapper.d.ts#L58); [crates/shamir-client-ts/src/core/builders/batch.ts:334](../../../../../crates/shamir-client-ts/src/core/builders/batch.ts#L334); [crates/shamir-client-ts/src/core/builders/batch.ts:370](../../../../../crates/shamir-client-ts/src/core/builders/batch.ts#L370); [crates/shamir-client-ts/src/index.ts:71](../../../../../crates/shamir-client-ts/src/index.ts#L71).

<a id="plan-p2-14"></a>

### Plan P2.14 — P2.14

Status: `confirmed-open`. Current risk: —.

The synthetic acknowledgement and echo wording remain. Today's server echoes identical inputs and the wrapper returns void; clarifying the comment is sufficient hygiene. A core public-signature change is not justified by a current canonicalization failure.

Evidence: [crates/shamir-client-node/src/lib.rs:291](../../../../../crates/shamir-client-node/src/lib.rs#L291); [crates/shamir-client/src/client.rs:1147](../../../../../crates/shamir-client/src/client.rs#L1147); [crates/shamir-server/src/db_handler/admin.rs:393](../../../../../crates/shamir-server/src/db_handler/admin.rs#L393); [crates/shamir-client-node/wrapper.js:148](../../../../../crates/shamir-client-node/wrapper.js#L148).

<a id="plan-p2-15"></a>

### Plan P2.15 — P2.15

Status: `unverified`. Current risk: —.

The manifest still lists six targets without optionalDependencies. Required packaging changes depend on the intended distribution contract and actual published artifacts, neither established here. Local platform builds are supported by loader branches; trimming targets is not itself proof of a runtime fix.

Evidence: [crates/shamir-client-node/package.json:20](../../../../../crates/shamir-client-node/package.json#L20); [crates/shamir-client-node/package.json:33](../../../../../crates/shamir-client-node/package.json#L33); [crates/shamir-client-node/index.js:283](../../../../../crates/shamir-client-node/index.js#L283); [tests/e2e/README.md:39](../../../../../tests/e2e/README.md#L39).

<a id="plan-p2-16"></a>

### Plan P2.16 — P2.16

Status: `confirmed-open`. Current risk: —.

Ticket/session-id snapshots remain non-zeroizing and password copy limitations remain undocumented. Focus wiping on credentials, not the public pin; distinguish overwriteable JS Buffers from immutable strings and unavoidable copies.

Evidence: [crates/shamir-client-node/src/lib.rs:85](../../../../../crates/shamir-client-node/src/lib.rs#L85); [crates/shamir-client-node/src/lib.rs:143](../../../../../crates/shamir-client-node/src/lib.rs#L143); [crates/shamir-client-node/src/lib.rs:61](../../../../../crates/shamir-client-node/src/lib.rs#L61); [crates/shamir-client-node/src/lib.rs:127](../../../../../crates/shamir-client-node/src/lib.rs#L127).

<a id="plan-p2-17"></a>

### Plan P2.17 — P2.17

Status: `confirmed-open`. Current risk: —.

Exception-comment, stale wrapper-name, and version-explanation edits remain undone. Splitting private helpers is not mandated by the closely-coupled-group rule. Prefer an intentional version note over an unsolicited version change.

Evidence: [crates/shamir-client-node/src/lib.rs:205](../../../../../crates/shamir-client-node/src/lib.rs#L205); [crates/shamir-client-node/src/lib.rs:336](../../../../../crates/shamir-client-node/src/lib.rs#L336); [crates/shamir-client-node/proof-typed-errors.js:10](../../../../../crates/shamir-client-node/proof-typed-errors.js#L10); [crates/shamir-client-node/Cargo.toml:3](../../../../../crates/shamir-client-node/Cargo.toml#L3); [crates/shamir-client-node/package.json:3](../../../../../crates/shamir-client-node/package.json#L3); [CLAUDE.md:505](../../../../../CLAUDE.md#L505).

<a id="plan-p2-18"></a>

### Plan P2.18 — P2.18

Status: `confirmed-open`. Current risk: —.

Both methods still decode before checking closed state. Decide and document error precedence before treating this diagnostic ordering as a correctness requirement.

Evidence: [crates/shamir-client-node/src/lib.rs:205](../../../../../crates/shamir-client-node/src/lib.rs#L205); [crates/shamir-client-node/src/lib.rs:207](../../../../../crates/shamir-client-node/src/lib.rs#L207); [crates/shamir-client-node/src/lib.rs:233](../../../../../crates/shamir-client-node/src/lib.rs#L233); [crates/shamir-client-node/src/lib.rs:235](../../../../../crates/shamir-client-node/src/lib.rs#L235).

## Corrections and qualified non-findings

- Do not repeat the alleged factory/subclass failure as established fact without pinned napi implementation or experimental evidence. Existing registered happy-path tests can detect its advertised TypeError consequence; registrations are not passing test results.
- ReplResponse::Error is intentionally a raw protocol result with a fencing epoch. wrapper.d.ts promises typed exceptions for DbResponse::Error. Converting the former into the latter would change tested behavior and can discard epoch information.
- The binding source is unchanged between summary commit 6df2afa9 and the inspected HEAD. Core Drop cleanup originated in d13ca5c0; Node nightly registration originated in b90a4472. These predate the review and refute omissions rather than prove later repairs.
- There is no committed native binary: .gitignore excludes binding binaries and its private lockfiles. The report's committed/shipped MSVC artifact premise is unsupported by this checkout.
- Existing automated e2e coverage follows package.json → e2e.test.js → runAll → registered .test.js functions → awaited assertions → nonzero failure exit. Message-based error assertions can detect missing rejection but do not verify typed code/retryable properties.
- Taking close outside its own guard does not bypass an already-held request guard. Core close aborts the reader rather than executing its ordinary EOF drain; do not promise pending-request settlement without tracing a coordinated shutdown mechanism.
- Exposing current timeout options does not bound the full TLS/SCRAM handshake, request write, or binding mutex wait. Availability exposure requires an unresponsive configured peer or connection failure; no arbitrary remote panic exploit was established.
- The sole cited pin expect has a source-proven invariant on successful authentication. Historical napi issue references and unspecified parser panics are not version-specific FFI failure proof.
- TS builder outputs can feed the binding as plain objects, although Batch.execute currently needs executeWithTouch. execution_time_us is a response field, and omitted return_all intentionally defaults true; those original examples are not demonstrated request defects.
- The setReplicator success buffer is reconstructed, but the current server echoes exactly the same fields and the wrapper discards them. Canonicalization corruption is hypothetical.
- Cargo/npm version divergence does not directly trip the loader: its optional version check compares npm platform-package metadata, not the Rust crate version.
- Performance conclusions are structural only. The wrapper's explicit response copy is O(buffer length); installed MessagePack returns binary blobs as views. Extra native copies, event-content decoding, latency, and 'priced correctly' were not measured or established.
- Async native I/O does not mean all JS work is nonblocking: wrapper encoding/decoding is synchronous. Ticket getter copying is variable-length, unlike fixed-array getters.
- The public pin is not secret material. JS Buffers can be explicitly overwritten, while complete erasure of immutable strings and all FFI copies cannot be promised. Password validation failures can precede Zeroizing construction.
- Server session retention also occurs after explicit close in the inspected teardown path; it is not proof that binding GC leaks a detached TCP connection.
- The one-primary-export rule permits closely coupled groups, and the tests-directory rule applies to modules containing tests. Neither establishes the original blanket style violations.
- Retain the original 28-row census only as review bookkeeping; recompute actionable status/severity counts after revalidation instead of presenting 25 allegations as 25 verified defects.

## Current follow-up order

1. Retain receiver-sensitive N-API factory construction; add direct public-wrapper/prototype and typed-error assertions. The alleged critical static-factory repair is not applicable.
2. Fix DNS/unbracketed-IPv6 host handling and accurately document connection inputs.
3. Expose timeout controls while defining bounds for the full handshake, lock acquisition, write, and response phases.
4. Redesign request ownership and coordinated close together so shutdown can interrupt stalled requests and settle pending callers.
5. Correct unsupported review claims, SDK-parity documentation, builder/type integration, and variant-specific infrastructure error handling.
6. Then address credential-copy hygiene, explicit response copies/discarded decoding, and scoped documentation nits; verify packaging against actual release artifacts.

## Coverage and limitations

- This module contains only SUMMARY.md; all 28 original findings, both positive-note sections, the executive synthesis, census, and all 18 Fix Plan items were assessed.
- Read-only inspection only: no files changed, git mutations, child agents, builds, tests, benchmarks, or reproductions.
- Root Cargo.lock resolves tokio 1.49.0, serde 1.0.228, rmp-serde 1.3.1, and zeroize 1.8.2 for the workspace, not necessarily for the separately resolved binding.
- Installed MessagePack 3.1.3 source was inspected read-only; that installation is not a committed binding dependency lock.
- Published npm packages, packed release contents, CI execution results, and measured latency were not verified.
- Parent read the exact napi 3.10.5, napi-derive 3.5.10 and backend 5.1.2 source archives. No generated native artifact or e2e run was performed; the factory refutation is source-based.

## Reviewed document inventory

- [SUMMARY.md](./SUMMARY.md) — 32 claim decisions; 18 explicit plan items.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-client-node — Cross-Lens Review (all 7 lenses, synthesized)

Crate: `crates/shamir-client-node/` (napi-rs 3.x native binding for `shamir-client`;
outside the default Cargo workspace per CLAUDE.md — built separately, MSVC-only on
Windows). Companion to the 2026-08-14 cross-crate sweep, which excluded this crate.

Review basis: every file under the crate (`src/lib.rs`, `Cargo.toml`, `build.rs`,
`package.json`, `wrapper.js`/`wrapper.d.ts`, generated `index.js`/`index.d.ts`,
`proof-typed-errors.js`, `rust-toolchain.toml`, the committed
`shamir-client.win32-x64-msvc.node`), read against the core SDK
(`crates/shamir-client/src/{client,error}.rs`), the wire types
(`shamir-query-types/src/wire/{db_message,repl}.rs`, `batch/*.rs`), the TS SDK
(`crates/shamir-client-ts/src/core/errors.ts`), CLAUDE.md, and the
`docs/guide-docs/client-server-protocol-spec/` docs. Read-only review — no build,
no tests, no source modifications.

## Executive summary

The Rust half of this binding is small and mostly disciplined (async fns on napi's
tokio runtime, `tokio::sync::Mutex` as the sanctioned exception, `Zeroizing` on
passwords, error-marker protocol documented at the boundary), but the crate is not
shippable as documented: (1) **the JS wrapper's entire enrichment layer is dead code
on the documented path** — `ShamirClient.connect()` is the *native* static factory
and returns base-class instances, so the wrapped `execute`/`repl`/typed-error
overrides never run; (2) **repl-level errors are reported as success** — the wrapper's
error marker checks `kind === "error"` but `ReplResponse::Error` is serialized with
tag `repl_kind`; (3) **the documented `host: "db.example.com"` usage can never
connect** — the host is parsed with `SocketAddr::from_str`, which only accepts IP
literals. Fix those three (plus surfacing the core SDK's timeout knobs, whose absence
makes the core `roundtrip`/drain hang race the permanent, unmitigated default for
every JS caller) before anything from this crate ships.

---

## 1. correctness-tdd

### 1.1 — critical — Wrapper's `execute`/`repl`/typed-error overrides are unreachable: `connect()` returns native-class instances
- File:line: `crates/shamir-client-node/wrapper.js:93-150` (subclass), `:152-158`
  (exports); factory at `crates/shamir-client-node/src/lib.rs:96-97`; types promising
  the wrapper surface at `wrapper.d.ts:49-67`.
- Issue: `class ShamirClient extends native.ShamirClient` overrides only *instance*
  methods (`execute`, `repl`, `createScramUser`, `setReplicator`). The only
  instantiation path is the inherited **static** `connect(opts)` — a napi
  `#[napi(factory)]` that constructs and returns a **`native.ShamirClient`**
  instance (napi-rs factories do not honor the JS `new.target`/subclass). The
  returned object's prototype is `native.ShamirClient.prototype`, so every call
  resolves to the *raw* native methods. Nothing in the package ever constructs the
  subclass (there is no `#[napi(constructor)]`, so `new` is not viable either).
- Failure scenario: a JS user follows the binding's own doc example
  (`src/lib.rs:8-24`): `const client = await ShamirClient.connect({...}); await
  client.execute('prod', { id: 'rw', queries: {...} })`. The plain object reaches the
  **raw native** `execute(db, batch: Buffer)` → napi throws `TypeError` (Buffer
  expected) — the documented happy path crashes. A user who instead passes a
  pre-encoded msgpack Buffer gets a raw Buffer back and **`DbResponse::Error`
  markers are never decoded**: a failed batch (`timeout`, `permission_denied`, …)
  resolves as a success Buffer. The typed-error feature (the entire point of task
  #519) never engages for any client obtained the documented way.
- Suggested fix: override the static in the wrapper and re-prototype the instance:
  `static async connect(opts) { const c = await super.connect(opts);
  Object.setPrototypeOf(c, ShamirClient.prototype); return c; }` (the subclass
  prototype chain terminates at `native.ShamirClient.prototype`, so `super.execute`
  inside the overrides keeps working). Add an automated test that goes through
  `ShamirClient.connect()` and asserts the wrapped `execute` encodes/decodes
  (would have caught this — see finding 1.4).

### 1.2 — high — Documented hostname usage can never connect: `host` is parsed as an IP literal
- File:line: `src/lib.rs:53-54` (doc: `"db.example.com"`) vs `:100-105`
  (`format!("{}:{}", host, port).parse::<SocketAddr>()`); core takes
  `addr: SocketAddr` (`crates/shamir-client/src/client.rs:59-60`), so neither
  layer resolves DNS.
- Issue: `std::net::SocketAddr::from_str` accepts **IP literals only**. The
  field's own doc advertises `"db.example.com"`; the binding's TLS design (SNI
  `server_name`, self-signed certs, TOFU pinning) is built for exactly that
  hostname workflow — but any non-IP host fails at parse time. IPv6 literals
  break too: `host: '::1'` formats as `"::1:3742"`, which does not parse
  (ambiguous/invalid).
- Failure scenario: a user following the field doc —
  `ShamirClient.connect({ host: 'db.example.com', port: 3742, … })` — gets
  `Error: invalid host:port: invalid IP address syntax` on every attempt, for
  every hostname. Same for `host: '::1'` on a localhost IPv6 server.
- Suggested fix: `let ip = host.parse::<IpAddr>()?` → `SocketAddr::new(ip, port)`,
  else fall back to `tokio::net::lookup_host((host.as_str(), port))` (DNS), and
  state the resolved behavior in the field doc. Red test: connect by hostname to
  a loopback alias.

### 1.3 — high — `ReplResponse::Error` is returned as success: wrapper marker checks `kind`, repl errors use `repl_kind`
- File:line: `crates/shamir-client-node/wrapper.js:81-87` (`decodeOrThrow` checks
  `decoded.kind === 'error'`) applied to repl at `:109-117`; Rust pass-through at
  `src/lib.rs:239-247`; wire truth: `shamir-query-types/src/wire/repl.rs:70`
  (`#[serde(tag = "repl_kind", …)]`), variant `Error` at `:97-110`; core doc
  `crates/shamir-client/src/client.rs:1156-1163` ("the server … returns
  `ReplResponse::Error { code: "bad_role" }`").
- Issue: `Client::repl` returns `Ok(ReplResponse::Error { leader_epoch, code,
  message })` for repl-layer failures (bad role, denied/unknown repo, stale epoch) —
  these are *successes* at the Rust `Result` level, so `src/lib.rs:239-243` encodes
  them as a normal response Buffer. `wrapper.js` even documents the tag divergence
  in its own comment (`:76-78`: "ReplResponse uses `repl_kind`") and then checks
  `kind` anyway. Only errors that surface as `ClientError::Db` (i.e.
  `DbResponse::Error`, tag `kind`, see `db_message.rs:282,347`) are converted to the
  marker and detected.
- Failure scenario: a session without the `replicator` role calls
  `client.repl(hello)`; the server replies `{repl_kind:"error", code:"bad_role", …}`;
  the Rust side encodes it as `Ok(Buffer)`, `decodeOrThrow` sees `kind === undefined`
  → `repl()` **resolves successfully** with the error payload. A follower's
  replication loop treats a denied/stale-epoch reply as data and never throws the
  documented `ShamirDbError` (`wrapper.d.ts` header claims repl throws it).
- Suggested fix (Rust side, one place): in `src/lib.rs::repl`, match
  `Ok(ReplResponse::Error { leader_epoch: _, code, message })` →
  `encode_db_error(code, message)` (add `#[napi]` BigInt accessor for the epoch if
  fencing callers need it); alternatively teach `decodeOrThrow` to also test
  `decoded.repl_kind === 'error'`. Red test: repl reply with `repl_kind:"error"`
  must reject with `ShamirDbError`.

### 1.4 — medium — No automated tests anywhere; the "proof" script re-implements the logic it claims to prove
- File:line: `proof-typed-errors.js:67-76, 93-100, 117-120` (inline copies of
  `decodeOrThrow`'s body), `package.json:29-32` (`scripts` = build only, no `test`);
  no Rust `tests/` directory exists (CLAUDE.md's mandated layout is absent by
  construction).
- Issue: sections 2–4 of the proof script copy-paste the decode-marker logic instead
  of calling the wrapper's `decodeOrThrow` (not exported), so a regression in
  `wrapper.js` cannot fail these assertions — they prove a snapshot of the logic,
  not the artifact. Finding 1.1 is precisely the class of bug this setup misses:
  every wrapper behavior claim in `wrapper.d.ts` is untested end-to-end, and
  CLAUDE.md's Red-Green-Refactor protocol has no evidence trail for task #519.
- Failure scenario: refactor `decodeOrThrow` (or rename the marker key) → all 20
  proof assertions still print ✅; CI (if wired) is green; every real client
  silently loses typed errors.
- Suggested fix: export `decodeOrThrow` (or a `hasDbErrorMarker` predicate) from
  `wrapper.js`; add `npm test` running an automated suite against the wrapper
  (mocking the native binding is enough for marker tests; the committed `.node`
  covers the rest on Windows). In-crate Rust tests per CLAUDE.md (`src/tests/` with
  a `mod.rs` manifest) for `encode_db_error` byte-compat with the JS decoder.

### 1.5 — low — `create_scram_user` wrapper msgpack-decodes 16 raw user_id bytes and discards the result
- File:line: `wrapper.js:123-131` (`decodeOrThrow(resp)` value unused; returns raw
  `resp`); Rust success path `src/lib.rs:264-268` (`Buffer::from(user_id)`).
- Issue: on success the Rust side returns 16 *raw* user_id bytes, not msgpack. The
  wrapper still runs them through `decode()`: any 16-byte string decodes to *some*
  msgpack value (self-delimiting format), and if the first value doesn't consume
  exactly 16 bytes `decode` throws "extra bytes" — which the `catch` deliberately
  swallows. So the decode is dead weight on every success, and the error-marker
  check only functions by accident of msgpack's framing.
- Failure scenario: none at runtime today (the swallow covers both outcomes); the
  cost is a guaranteed wasted allocation+parse per user creation and a fragile
  dependency on `decode` never throwing *before* the marker check on adversarial
  server bytes.
- Suggested fix: give the wrapper a non-throwing `hasDbErrorMarker(buf)` probe
  (try `decode`, return `decoded?.kind === 'error'`), use it in
  `createScramUser`/`repl`/`setReplicator`, and drop the throwaway full decode.

### 1.6 — low — `execute` decodes the payload before the closed-check; decode errors mask "client closed"
- File:line: `src/lib.rs:205-210` (`from_slice` at :205, lock + closed-check at
  :207-210; same order in `repl` at :233-238).
- Issue: an `execute` on an already-closed client with a malformed payload reports
  "invalid batch payload" instead of "client closed".
- Failure scenario: JS code closes the client, a stray in-flight call with a stale
  buffer rejects with a payload error → operator chases a phantom data bug instead
  of a lifecycle bug.
- Suggested fix: take the guard and check `Some` first, then decode.

### 1.7 — nit — Version drift: Cargo `0.1.0-alpha.1` vs npm/generated-loader `0.1.0`
- File:line: `Cargo.toml:3`, `package.json:3`, `index.js:80` (hardcoded expected
  binding version `'0.1.0'`).
- Failure scenario: publishing the Rust crate version verbatim into the npm
  package would trip the generated `NAPI_RS_ENFORCE_VERSION_CHECK` mismatch error.
- Suggested fix: derive the npm version from the crate version in the release
  script, or document the intentional divergence.

## 2. concurrency-lockfree

### 2.1 — medium — One `tokio::sync::Mutex` held across every request serializes the demultiplexed client
- File:line: `src/lib.rs:82` (`inner: Arc<Mutex<Option<core::Client>>>`), guards held
  across `.await` at `:193-197` (ping), `:207-223` (execute), `:235-247` (repl),
  `:260-271` (create_scram_user), `:285-305` (set_replicator); core capability
  defeated: `crates/shamir-client/src/client.rs:3-10` ("Concurrent callers can issue
  multiple requests in flight simultaneously… responses arrive in completion order").
- Issue: the lock's only job is to let `close()` `take()` the client, but every
  method holds it for the *whole round trip*, so concurrent JS callers are fully
  serialized — head-of-line blocking that the core SDK explicitly does not have
  (rid-demux exists precisely for this). The primitive choice is sanctioned
  (CLAUDE.md's async exception; no `std::sync::Mutex` anywhere — good), but the
  *holding scope* is wrong.
- Failure scenario: one slow 5 s `execute` blocks every other `ping`/`execute` on
  the same client instance for its full duration; a server-side stall turns into an
  event-loop-wide freeze of that client even though the wire could pipeline.
- Suggested fix: `Arc<tokio::sync::RwLock<Option<Arc<core::Client>>>>` — readers
  clone the `Arc` under a short read lock and drop it before awaiting; `close()`
  takes the write lock. (If `core::Client` is kept behind `Arc`, `close`'s
  consume-self problem needs `Arc::try_unwrap`/`Option::take` on the `Arc` slot, or
  a `close`-flag + reader-shutdown call added to the core API.)

### 2.2 — medium — Concurrent JS callers see stale pin/ticket snapshots only (correct), but `close()`'s take-then-await ordering can strand in-flight callers behind a permanent lock (see 4.1/6.1)
- File:line: `src/lib.rs:311-317` (`close` holds the guard while awaiting
  `client.close()`); interaction with 2.1's held-across-await guards.
- Issue: `close()` waits for the *current* lock holder's full round trip before it
  can take the client. With unbounded timeouts (finding 3.1) a stalled request
  means `close()` pends forever — JS `await client.close()` never settles and the
  process cannot shut the socket down.
- Failure scenario: server accepts TCP then stops responding; task A is inside
  `execute()` (hung); task B calls `close()` during shutdown → B hangs; the
  app's graceful-shutdown path never completes; Node exits only via signal/abort.
- Suggested fix: `let client = { self.inner.lock().await.take() };` then
  `client.close().await` **outside** the lock (in-flight callers get
  `ConnectionClosed` from the core reader drain instead of blocking `close`).

## 3. security-crypto

### 3.1 — high — No timeouts surfaced and no cancellation: JS callers get the core SDK's known unbounded-hang class as the *only* behavior
- File:line: `src/lib.rs:130-133` (`connect_timeout: None, request_timeout: None`
  hardcoded — comment: "preserve prior unbounded-wait behaviour"); core knobs
  exist: `crates/shamir-client/src/client.rs:85-92`; the core-side hang race they
  were added for is documented HIGH in the sweep:
  `docs/.../shamir-client/error-handling-lifecycle.md` finding 1
  (`client.rs` closed-check vs reader-drain).
- Issue: the binding is the *only* consumer class that cannot opt out of unbounded
  waits: no `connectTimeoutMs`/`requestTimeoutMs` in `ConnectOptions`
  (`index.d.ts:66-91`), no `AbortSignal`/cancel on any method. A hung napi promise
  is not cancelable from JS at all.
- Failure scenario: server accepts the TCP connection then stalls during SCRAM →
  `await ShamirClient.connect(...)` never settles; Node's default no-keepalive
  socket timeouts don't apply (TLS established or SCRAM in flight); the app's
  request queue backs up with no error to retry on. This is the production-shape
  of the core team's own HIGH finding, made unavoidable.
- Suggested fix: add `connect_timeout_ms`/`request_timeout_ms` (camelCase
  `connectTimeoutMs`/`requestTimeoutMs` to mirror the TS SDK) to
  `ConnectOptions` and thread into `core_opts` (finding 5.2's drift closes with
  the same edit). AbortSignal support is a P2 follow-up.

### 3.2 — medium — No `catch_unwind` discipline at the FFI boundary
- File:line: all exports in `src/lib.rs` (`#[napi]`/`#[napi(factory)]` at :96, :158,
  :164, :171, :177, :185, :191, :203, :229, :253, :283, :310) — no
  `#[napi(catch_unwind)]` anywhere; reachable panic site in the call graph:
  `crates/shamir-client/src/client.rs:539-542` (`.expect("either trusted_pin
  pre-set or TOFU callback fired")`, flagged LOW upstream); untrusted-input decode
  at `src/lib.rs:205,233`.
- Issue: per napi-rs docs, an uncaught panic in a *sync* generated callback
  terminates the Node process; for *async* fns the runtime normally rejects the
  promise but known napi-rs issues (#2047) report hung promises in some versions.
  The sync getters here are panic-trivial, but `connect` reaches the core
  `.expect()`, and rmp-serde decodes attacker-influenced bytes (a malicious/
  compromised server's frames are decoded inside the core client).
- Failure scenario: a hostile server crafts a frame that trips a core invariant →
  worst case the entire Node process aborts (every request in flight dies, not
  just this client); best case one promise hangs forever.
- Suggested fix: `#[napi(catch_unwind)]` on all exports (documented napi-rs
  mechanism; converts panic payload → JS `Error`), and upstream-fix the core
  `.expect()` per the sweep's error-handling finding 9.

### 3.3 — low — Secret copies in the napi struct are not zeroized (`resumption_ticket`, `session_id`, `pin`)
- File:line: `src/lib.rs:85-89` (plain `[u8; 32]` ×2 + `Option<Vec<u8>>`), populated
  at `:140-144`; contrast core, where the ticket copy is `Zeroizing`
  (`crates/shamir-client/src/client.rs` — `resumption_ticket:
  Option<Zeroizing<Vec<u8>>>`), and where `session_id` is secret material (it
  derives the request-HMAC key: `client.rs:1101`).
- Issue: the binding duplicates the resumption ticket (a bearer credential that
  skips Argon2id on reconnect) and the session_id into un-wiped heap allocations
  that outlive the logical session, partially defeating the core crate's `Zeroizing`
  hygiene. Inherent limitation worth stating: the JS-side `Buffer`/`String` copies
  (password included) can never be zeroized.
- Failure scenario: heap-scraper or core-dump forensics recovers resumption tickets
  from long-lived Node processes after the sessions ended; the CLAUDE.md
  zeroization story ("Drift … impossible", `src/lib.rs:27-30`) quietly stops
  applying one layer above the crypto.
- Suggested fix: `Zeroizing<Vec<u8>>` for the ticket copy (and `Zeroizing` for
  pin/session_id if cheap); document the JS-side non-zeroizable copies in
  `ConnectOptions.password`'s doc.

### 3.4 — nit — "Zeroised in the native side" overstates what the binding controls
- File:line: `src/lib.rs:61-63`, `index.d.ts:75-79`; actual zeroization at
  `src/lib.rs:127` (`Zeroizing::new(opts.password.into_bytes())`).
- Issue: the final Rust `String` is zeroized on drop (correct), but the napi
  `String` FromNapiValue conversion necessarily creates transient UTF-8 copies
  before that point, and the JS-side string is permanent.
- Failure scenario: none beyond the hygiene gap described above; the doc sentence
  implies stronger guarantees than the FFI allows.
- Suggested fix: one doc line — "the final Rust-side copy is zeroized; JS-side and
  intermediate FFI copies are not wipeable."

### Positive notes (kept for calibration parity)
`trustedPin` length-validated before the handshake (`src/lib.rs:107-121`); TOFU
pin capture/persist flow matches the core contract; all TLS/SCRAM/Argon2id stays
in Rust; msgpack-only at the boundary (no JSON intermediate); the retryable-code
set is byte-identical to the TS SDK (`wrapper.js:39-44` vs
`shamir-client-ts/src/core/errors.ts:28-33` — verified).

## 4. performance-hotpath

### 4.1 — medium — `close()` blocks on the lock (and therefore on any in-flight request) instead of taking-and-releasing
- File:line: `src/lib.rs:311-317`; same-lock holders as finding 2.1.
- Issue: close is a lifecycle op that should never queue behind data-plane
  round trips; as written it inherits their worst-case latency (unbounded — see
  3.1). This is also the trivially fixable half of finding 2.2.
- Failure scenario: shutdown path issues `close()` while a stalled `execute` holds
  the lock → close never resolves; operator kills -9; TLS close_notify never sent;
  server keeps the session until expiry.
- Suggested fix: see 2.2 (take under lock, close outside).

### 4.2 — low — Redundant decode + copy on every `repl`/`createScramUser` success, and a Uint8Array copy per `decodeOrThrow`
- File:line: `wrapper.js:82` (`new Uint8Array(buf)` — copies an already-byte-typed
  Buffer), `:109-117, 123-131` (full `decodeOrThrow` whose result is discarded),
  `src/lib.rs:239-243` (Rust encodes every repl response, incl. huge `Pull` event
  blobs, only for JS to decode-and-discard or pass through raw).
- Failure scenario: a replication `Pull` response of N MB is msgpack-encoded in
  Rust, copied into a Buffer, decoded in JS (plus one `Uint8Array` copy), and
  thrown away — triple handling per pull in the hottest replication path.
- Suggested fix: probe-without-materializing (`hasDbErrorMarker` from 1.5, using
  `buf` directly as the Uint8Array view — `new Uint8Array(buf.buffer, buf.byteOffset,
  buf.byteLength)`, no copy); longer term, move the marker check to the Rust side
  (finding 1.3's fix removes the need to decode repl responses in JS at all).

### Positive notes
All I/O is async on napi's tokio runtime (`features = ["async"]` → `tokio_rt`,
`Cargo.toml:27-30`) — nothing blocks the JS event loop; sync getters are O(1)
array/vec copies; `execute`'s double msgpack hop (JS object ↔ Rust ↔ wire) is
inherent to the FFI design and priced correctly.

## 5. api-wire-protocol

### 5.1 — high — *(primary: same as 1.3)* — repl error channel drift: the binding flattens `ReplResponse::Error` into a success buffer, breaking the wire's error taxonomy at the JS boundary
- File:line: `src/lib.rs:239-247`, `wrapper.js:81-87,109-117`;
  `shamir-query-types/src/wire/repl.rs:70,97-110`.
- (See finding 1.3 for the full write-up; listed here because it is the lens-defining
  drift: the wire has **two** error channels — `DbResponse::Error` (`kind`) and
  `ReplResponse::Error` (`repl_kind`) — and the binding translates exactly one of
  them into JS exceptions.)

### 5.2 — medium — "Mirrors the Rust SDK 1:1" is false; resumption ticket getters dead-end with no resume path
- File:line: claim at `src/lib.rs:3`; missing surface vs
  `crates/shamir-client/src/client.rs`: `resume` (:862, needs `ResumeOptions` —
  absent from `ConnectOptions`, `index.d.ts:66-91`), `connect_local` (:675),
  `stream_cursor` (:1178), `subscribe_push` (:996), `get_ddl_op_status` (:1194),
  `change_password*` wire ops, and `server_query_version` (:987 — the documented
  gate for emitting v2 id-keyed protocol).
- Issue: the header doc promises parity; the binding exposes a strict subset with
  no `#[allow]`-style enumeration of the gap. Worst case is resumption: JS callers
  are handed `resumptionTicket()`/`resumptionExpiresAtNs()` (`src/lib.rs:176-188`,
  whose docs say "persist this") with **no way to pass the ticket back** — the
  core `Client::resume(opts.ticket)` path simply doesn't exist here, so the
  Argon2id skip the protocol doc (`SESSION_RESUMPTION.md`) advertises is
  unreachable from Node.
- Failure scenario: a Node service diligently persists the ticket per the getter
  docs, reconnects via `ShamirClient.connect()` with the *password* every time —
  paying full Argon2id on every reconnect — and never learns the feature is
  unimplemented because nothing says so.
- Suggested fix: either surface `resume` (extend `ConnectOptions` with
  `resumptionTicket?: Buffer` and route to `Client::resume`, pinning against
  `trustedPin`) or correct the header doc to name the unsupported surface; same
  for `server_query_version` (v2 gating is currently impossible from JS).

### 5.3 — medium — `execute(object)` teaches hand-assembled wire shapes; no builder, no exported BatchRequest/BatchResponse types
- File:line: `src/lib.rs:19-22` (doc example: `{ id: 'rw', queries: { rd: { from:
  'items' } } }`), `wrapper.js:98-102`, `wrapper.d.ts:57-58` (`batch: object` /
  `Promise<object>`).
- Issue: CLAUDE.md's builder-only rule names "the typed client builder in
  `shamir-client-ts`" as the sanctioned construction path; the documented napi/FFI
  exception covers *deserializing what arrived as bytes*, not a public JS API that
  invites users to hand-write snake_case wire objects (`execution_time_us`,
  `interner_epochs`, `result_encoding`) with zero compile-time checking and no
  exported types. The TS SDK's builder cannot drive this binding (different
  client class), so Node users of *this* package have no builder at all.
- Failure scenario: a JS caller writes `executionTimeUs` (camelCase) or omits
  `return_all`; serde silently `#[serde(default)]`s or hard-fails at runtime with
  "missing field `queries`"-shaped errors that reference Rust field names the
  user never saw documented.
- Suggested fix: re-export TS types for `BatchRequest`/`BatchResponse` (even loose
  interfaces) in `wrapper.d.ts`, and document that `shamir-client-ts`' builder
  output is the intended producer of `batch` objects — or accept a builder instance
  from `shamir-client-ts` directly.

### 5.4 — low — `set_replicator` success buffer is fabricated from caller inputs, not the server echo
- File:line: `src/lib.rs:290-301` (constructs `DbResponse::ReplicatorSet { user, on }`
  locally); core discards the real echo (`crates/shamir-client/src/client.rs:1135-1161`
  matches `ReplicatorSet { .. }` and returns `()`).
- Issue: the comment claims the buffer carries "the echoed … values"; it carries the
  *caller's* values re-serialized. Today server echo == request, but if the server
  ever normalizes the username, the JS layer can't observe it — and the fabricated
  round-trip exists only so the wrapper can error-marker-decode it.
- Failure scenario: server starts echoing a normalized user (e.g. case-mapped) →
  JS caller sees its raw input reflected back and persists the wrong canonical
  name.
- Suggested fix: have the core `set_replicator` return the echo (small signature
  change) or drop the pretense: return a dedicated `{ ok: true }` marker buffer and
  say so in the doc.

### 5.5 — low — Generated loader's platform packages are unpublished/undeclared: every non-win32-x64-msvc path is dead
- File:line: `package.json:18-28` (`napi.targets` lists 6 triples; no
  `optionalDependencies` at all), `index.js:106-140` (win32 fallbacks require
  `shamir-client-win32-x64-*` packages), `index.js:561-576` (final failure with
  the npm-bug hint).
- Failure scenario: `npm i shamir-client` on Linux/macOS → `require('shamir-client-linux-x64-gnu')`
  → MODULE_NOT_FOUND → `Error: Cannot find native binding` with a misleading
  "npm optional-deps bug, reinstall" message. Fine for the current MSVC-only
  reality, wrong for the advertised target list.
- Suggested fix: until cross-publishing exists, trim `napi.targets` to the shipped
  triple (or add the optionalDependencies map), so the loader fails honestly.

## 6. error-handling-lifecycle

### 6.1 — medium — *(primary: same as 2.2/4.1)* — `close()` is not a lifecycle-safe operation under stalls
- (Full write-up at 2.2/4.1; listed here because the lifecycle contract "Close the
  TLS write half cleanly. Idempotent" (`src/lib.rs:308-317`) is unreachable in
  exactly the situations — server stall, hung request — where closing matters.)

### 6.2 — low — Infrastructure errors lose all taxonomy crossing the boundary
- File:line: `src/lib.rs:351-353` (`infra_error` → `Error::from_reason(e.to_string())`);
  rich source enum at `crates/shamir-client/src/error.rs:5-55` (ConnectTimeout,
  RequestTimeout, Tls, Handshake, ConnectionClosed, Protocol…).
- Issue: the Db/infra split is deliberate and documented (`src/lib.rs:320-343`), but
  the infra half erases the `thiserror` taxonomy JS-side: no `.code`, no `.cause`,
  no `name` — callers must regex English prose to distinguish "server identity
  changed (possible MITM)" from "bad password" or "request timed out".
- Failure scenario: a retry policy that should re-connect on `ConnectionClosed` but
  never on `Handshake` cannot be written without `err.message.includes(...)`;
  the sweep's core-crate finding (typed handshake variants flattened to strings)
  compounds one layer up.
- Suggested fix: `Error::new(Status::GenericFailure, …)` with
  `set_named_property` for a stable `.code` string per variant (mirror
  `ShamirDbError.code`), keeping the message for prose.

### 6.3 — low — No Drop/finalization: a GC'd-without-close client leaks the TCP connection and server session until expiry
- File:line: `src/lib.rs:80-90` (no `Drop` impl, no napi finalizer; JS-side `close()`
  is opt-in).
- Failure scenario: exception path abandons a client without `await close()` → the
  napi object is GC'd, the `core::Client` drops without TLS close_notify, the
  server holds the authenticated session until `expires_at_ns`; under reconnect
  loops this accumulates server-side session state.
- Suggested fix: `impl Drop for ShamirClient` that spawns a best-effort
  `client.close()` (or at minimum an abort/shutdown) on the napi tokio runtime;
  document that `close()` remains the clean path.

### 6.4 — nit — `encode_db_error`'s own failure path degrades to a plain Error (acceptable, but untested and undocumented)
- File:line: `src/lib.rs:361-366`.
- Issue: if serializing the error marker itself failed, the caller gets a generic
  napi Error — unreachable in practice (serializing two owned Strings), but it is
  an untested branch of the package's central error contract.
- Suggested fix: none required; cover it in the Rust tests from finding 1.4 if the
  harness makes it cheap.

## 7. style-claude-md

### 7.1 — low — `src/lib.rs` carries multiple primary exports; error-mapping helpers belong in a sibling file
- File:line: `src/lib.rs` defines `ConnectOptions` (:52), `ShamirClient` (:81),
  `infra_error` (:351), `encode_db_error` (:361) plus ~40 lines of design-comment
  prose (:320-343).
- Issue: CLAUDE.md's "one file = one primary export" rule would split the
  error-mapping layer (the two free fns + their rationale comment) into e.g.
  `src/error_map.rs`, keeping `lib.rs` to the binding surface. Defensible for a
  single-module FFI shim, but the error-mapping comment block is now larger than
  either function and will rot faster than the code it explains (it already
  references `index.js` as the decoder — see 7.3).
- Failure scenario: diff-blame on the error protocol touches the class file;
  the next variant added to `ClientError` must be threaded through a comment
  essay to find the one match arm that matters.
- Suggested fix: move `infra_error`/`encode_db_error` + the design comment into
  `src/error_map.rs`; `lib.rs` re-exports.

### 7.2 — low — Test layout violates the repo convention: no `tests/` directory, no automated runner
- File:line: crate root (no `src/tests/`, no `tests/`), `package.json:29-32`
  (`scripts` lacks `test`), `proof-typed-errors.js` (manual script, not wired into
  anything).
- Issue: CLAUDE.md mandates one `tests/` directory per module with a `mod.rs`
  manifest and the Red-Green-Refactor protocol; this crate's only verification
  artifact is a console script that must be run by hand on Windows and that tests
  a copy of the logic (finding 1.4).
- Failure scenario: the crate cannot participate in the repo's pre-commit gate
  story at all — `./scripts/test.sh` scopes can't name it (outside the workspace,
  by design), and nothing fails when `wrapper.js` regresses.
- Suggested fix: `npm test` wired to an automated suite; Rust-side
  `encode_db_error`/marker byte-compat tests under `src/tests/mod.rs` per
  convention (run via `cargo test -p shamir-client-node` in the crate's own
  toolchain, documented as the separate-build exception it already is).

### 7.3 — nit — Sanctioned-exception comment present at `repl` but missing at the identical `execute` rmp-serde boundary
- File:line: `src/lib.rs:231-233` (repl: "FFI boundary — raw serde is the sanctioned
  exception (CLAUDE.md)") vs `:205` (execute: bare `rmp_serde::from_slice`, no
  comment).
- Issue: CLAUDE.md's builder-only rule requires a one-line *why* wherever raw JSON/
  msgpack appears outside the builder; both sites qualify under the napi exception,
  but only one says so.
- Failure scenario: a future reader pattern-matching on the rule flags `execute`
  (or, worse, "fixes" it into the builder and breaks the wire).
- Suggested fix: copy the one-line exception comment to `:205`.

### 7.4 — nit — Stale/self-contradictory naming: comments say the wrapper is `index.js`; it is `wrapper.js`
- File:line: `proof-typed-errors.js:10` ("The JS wrapper (index.js) detects the
  marker"), `:67` ("from index.js"); contrast `wrapper.js:3-6` ("index.js is
  auto-generated … This wrapper is the package's real entry point").
- Issue: the #519 split renamed the layers but the proof script's comments still
  point at the generated loader; anyone grepping `index.js` for `decodeOrThrow`
  finds loader boilerplate instead.
- Failure scenario: purely documentary — misleads the next reviewer/maintainer.
- Suggested fix: update the two comment references to `wrapper.js`.

---

## Finding counts

| Severity | Lens-tagged findings | Finding numbers (dedup groups in one row count once) |
|---|---|---|
| critical | 1 | 1.1 |
| high | 4 | 1.2 (host/IP parse), 1.3 + 5.1 (repl drift — one defect, two lenses), 3.1 (unbounded waits) |
| medium | 8 | 1.4, 2.1, 2.2 + 4.1 + 6.1 (close-under-lock — one defect, three lenses), 3.2, 5.2, 5.3 |
| low | 10 | 1.5, 1.6, 3.3, 4.2, 5.4, 5.5, 6.2, 6.3, 7.1, 7.2 |
| nit | 5 | 1.7, 3.4, 6.4, 7.3, 7.4 |
| **total** | **28** | lens-tagged findings; **25 distinct defects** after dedup (1.3/5.1 and 2.2/4.1/6.1) |

Deduplicated defect census: **1 critical, 3 high, 6 medium, 10 low, 5 nit = 25
distinct defects** (28 lens-tagged findings).

## Fix Plan

**P0 — before anything else ships from this crate**
1. **Make the wrapper actually wrap.** Override `static connect` in `wrapper.js`,
   re-prototype the factory result onto the subclass, and add an automated
   end-to-end test through `ShamirClient.connect()`. Closes **1.1** (critical) and
   makes every other wrapper fix verifiable.
2. **Route repl errors through the marker.** Match `Ok(ReplResponse::Error {..})`
   in `src/lib.rs::repl` → `encode_db_error(code, message)` (or extend the JS
   probe to `repl_kind`). Red test with `repl_kind:"error"`. Closes **1.3/5.1**.
3. **Fix/align host parsing with its docs.** Parse `host` as `IpAddr` →
   `SocketAddr::new(ip, port)`, else `tokio::net::lookup_host((host, port))`; fix
   the doc at `src/lib.rs:53-54` to state DNS support explicitly. Closes **1.2** —
   keeps `host: "db.example.com"` from being a permanent connect failure.
4. **Surface the timeout knobs.** `connectTimeoutMs`/`requestTimeoutMs` in
   `ConnectOptions` → `core::ConnectOptions`. Closes **3.1** and removes the
   unmitigated exposure to the core roundtrip/drain hang race.

**P1 — soon**
5. **`close()` takes-and-releases**: `take()` under the lock, `client.close()`
   outside it. Closes **2.2/4.1/6.1**.
6. **Stop serializing the multiplexer**: short-scope lock + `Arc<core::Client>`
   clone-out (or `RwLock`). Closes **2.1**.
7. **`#[napi(catch_unwind)]` on all exports** + upstream fix for the core
   `.expect()` (`shamir-client/src/client.rs:539`). Closes **3.2**.
8. **Automated tests**: `npm test` (wrapper unit + connect-path integration),
   export `decodeOrThrow`/`hasDbErrorMarker`, Rust marker byte-compat tests in
   `src/tests/`. Closes **1.4, 7.2**.
9. **Infra error taxonomy**: stable `.code` per `ClientError` variant on napi
   Errors. Closes **6.2**.

**P2 — backlog**
10. `hasDbErrorMarker` probe without materializing decodes; kill the discarded
    `decodeOrThrow` calls. Closes **1.5, 4.2**.
11. `Drop`-based best-effort close for GC'd clients. Closes **6.3**.
12. Surface `resume` (ticket round-trip) or correct the "1:1" doc; expose
    `server_query_version`. Closes **5.2**.
13. Re-export `BatchRequest`/`BatchResponse` TS types; document the builder
    story for Node. Closes **5.3**.
14. Return the real server echo from `set_replicator` (core signature change) or
    reword the fabrication comment. Closes **5.4**.
15. Trim `napi.targets`/add optionalDependencies to match shipped binaries.
    Closes **5.5**.
16. `Zeroizing` for the napi ticket/session-id copies + doc line on
    non-wipeable JS copies. Closes **3.3, 3.4**.
17. Split `src/error_map.rs`; comment hygiene (`execute` exception comment,
    `index.js`→`wrapper.js` references); version-drift note. Closes
    **7.1, 7.3, 7.4, 1.7**.
18. Check decode-before-closed-check ordering. Closes **1.6**.

</details>
