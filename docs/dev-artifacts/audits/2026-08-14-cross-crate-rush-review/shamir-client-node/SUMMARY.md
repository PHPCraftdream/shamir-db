<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-client-node — SUMMARY independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Receiver-sensitive factory construction, raw replication errors, automatic finalization and existing nightly registration independently refute the original major omissions. Address parsing, unbounded waits, serialization of concurrent requests and stalled-close behavior remain. Exact dependency inspection also exposes caller-owned Buffer races and integer-fidelity loss. Claim 1.5 has a valid raw-ID false-positive witness, so its current non-functional-only assurance is too strong. Several remaining alleged defects are optional API policies rather than violated contracts.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 32 | 16 | 0 | 0 | 8 | 1 | 7 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1-1"></a>

### Claim 1.1 — Wrapper's `execute`/`repl`/typed-error overrides are unreachable: `connect()` returns native-class instances

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Calling the inherited factory through the wrapper supplies that subclass as its receiver. Exact napi 3.10.5 _factory constructs through the restored receiver; inspected backend 5.1.2 generates cb.factory. Existing registered object-level CRUD would fail under the alleged base-instance mutation. No reprototyping repair is justified.

Evidence: [crates/shamir-client-node/wrapper.js:93](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L93); [crates/shamir-client-node/src/lib.rs:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L96); [crates/shamir-client-node/Cargo.toml:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/Cargo.toml#L39); [napi 3.10.5 published src/bindgen_runtime/callback_info.rs:199](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/callback_info.rs); [napi-derive-backend 5.1.2 published src/codegen/fn.rs:275](https://docs.rs/crate/napi-derive-backend/5.1.2/source/src/codegen/fn.rs); [tests/e2e/tests/02-basic-crud.test.js:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/tests/e2e/tests/02-basic-crud.test.js#L19).

<a id="review-1-2"></a>

### Claim 1.2 — Documented hostname usage can never connect: `host` is parsed as an IP literal

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The documented DNS host fails before networking, as does unbracketed ::1 after concatenation. IPv4 and bracketed IPv6 work. This is a configuration-specific public-input defect, not universal connection failure. A discriminating oracle must exercise hostname resolution and IPv6 address construction through the actual connect path.

Evidence: [crates/shamir-client-node/src/lib.rs:53](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L53); [crates/shamir-client-node/src/lib.rs:100](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L100); [crates/shamir-client/src/client.rs:60](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L60).

<a id="review-1-3"></a>

### Claim 1.3 — `ReplResponse::Error` is returned as success: wrapper marker checks `kind`, repl errors use `repl_kind`

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Raw protocol Error results are intentional and retain leader_epoch. The server capability gate, core pass-through, wrapper contract and registered bad_role assertions agree. Converting them into the smaller DB-error marker would alter behavior and potentially discard fencing information.

Evidence: [crates/shamir-server/src/db_handler/repl_handler.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/repl_handler.rs#L52); [crates/shamir-client/src/client.rs:1164](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1164); [crates/shamir-client-node/wrapper.js:104](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L104); [crates/shamir-query-types/src/wire/repl.rs:97](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/wire/repl.rs#L97); [tests/e2e/tests/16-replication.test.js:235](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/tests/e2e/tests/16-replication.test.js#L235).

<a id="review-1-4"></a>

### Claim 1.4 — No automated tests anywhere; the "proof" script re-implements the logic it claims to prove

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `partially-fixed`.

The no-tests premise is refuted by pre-existing nightly registration, not repaired after review. The residual oracle gap remains open: the proof copies detection, and actual e2e errors only inspect messages. Removing ShamirDbError.code/retryable assignments while preserving its message would evade these assertions. Test actual public methods and Rust-produced marker bytes.

Evidence: [.github/workflows/ts-e2e-nightly.yml:162](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/.github/workflows/ts-e2e-nightly.yml#L162); [tests/e2e/e2e.test.js:51](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/tests/e2e/e2e.test.js#L51); [tests/e2e/helpers/runner.js:106](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/tests/e2e/helpers/runner.js#L106); [tests/e2e/tests/09-errors.test.js:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/tests/e2e/tests/09-errors.test.js#L33); [crates/shamir-client-node/proof-typed-errors.js:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/proof-typed-errors.js#L67); [crates/shamir-client-node/wrapper.js:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L64).

<a id="review-1-5"></a>

### Claim 1.5 — `create_scram_user` wrapper msgpack-decodes 16 raw user_id bytes and discards the result

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Besides avoidable decoding, there is a valid false-positive shape. The 16 bytes 82 a4 6b 69 6e 64 a5 65 72 72 6f 72 a1 78 a1 79 encode {kind:error,x:y}; inspected codec 3.1.3 accepts that map, and the wrapper throws ShamirDbError with undefined code/message. The directory's unrestricted random-byte generator permits this ID; its masked first-eight-byte principal is nonzero, and it is accepted when unused. This is a rare valid-ID edge case, not a measured incidence or remote privilege escalation.

Evidence: [crates/shamir-client-node/wrapper.js:82](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L82); [crates/shamir-client-node/wrapper.js:123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L123); [crates/shamir-client-node/src/lib.rs:268](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L268); [crates/shamir-server/src/db_handler/admin.rs:223](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L223); [crates/shamir-server/src/user_directory.rs:513](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/user_directory.rs#L513); [crates/shamir-server/src/user_directory.rs:526](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/user_directory.rs#L526); [crates/shamir-types/src/access.rs:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/access.rs#L33); [@msgpack/msgpack 3.1.3 src/Decoder.ts:640](https://github.com/msgpack/msgpack-javascript/blob/v3.1.3/src/Decoder.ts#L640).

<a id="review-1-6"></a>

### Claim 1.6 — `execute` decodes the payload before the closed-check; decode errors mask "client closed"

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The ordering is real, but no contract requires closed-state errors to outrank invalid-input errors. It is a diagnostic policy choice, not established incorrect behavior. Moving decoding under the mutex would also increase its holding scope.

Evidence: [crates/shamir-client-node/src/lib.rs:205](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L205); [crates/shamir-client-node/src/lib.rs:207](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L207); [crates/shamir-client-node/src/lib.rs:233](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L233); [crates/shamir-client-node/wrapper.d.ts:57](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.d.ts#L57).

<a id="review-1-7"></a>

### Claim 1.7 — Version drift: Cargo `0.1.0-alpha.1` vs npm/generated-loader `0.1.0`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The metadata mismatch persists without a local explanation. It does not itself cause loader rejection: the optional comparison concerns npm platform-package metadata, while local native-file loading bypasses it. A release-policy note is sufficient; no unsolicited version change is warranted.

Evidence: [crates/shamir-client-node/Cargo.toml:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/Cargo.toml#L3); [crates/shamir-client-node/package.json:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/package.json#L3); [crates/shamir-client-node/index.js:126](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/index.js#L126); [crates/shamir-client-node/index.js:132](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/index.js#L132).

<a id="review-2-1"></a>

### Claim 2.1 — One `tokio::sync::Mutex` held across every request serializes the demultiplexed client

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Every request retains the exclusive binding guard through its core await, so a second call cannot register/send while the first awaits a response. This defeats the core's documented concurrent-call semantics. A controlled peer observing two requests before releasing either response discriminates this mechanism; existing sequential CRUD does not.

Evidence: [crates/shamir-client-node/src/lib.rs:82](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L82); [crates/shamir-client-node/src/lib.rs:193](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L193); [crates/shamir-client-node/src/lib.rs:207](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L207); [crates/shamir-client-node/src/lib.rs:285](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L285); [crates/shamir-client/src/client.rs:418](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L418); [crates/shamir-client/src/client.rs:1267](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1267).

<a id="review-2-2"></a>

### Claim 2.2 — Concurrent JS callers see stale pin/ticket snapshots only (correct), but `close()`'s take-then-await ordering can strand in-flight callers behind a permanent lock (see 4.1/6.1)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Getter snapshots are immutable and independent of the request mutex. However, an unanswered request holds that mutex indefinitely and close cannot acquire it. Close also retains its own guard during shutdown. The useful oracle is close plus pending-call settlement after a peer receives but withholds a response, not a sleep-based timing assertion.

Evidence: [crates/shamir-client-node/src/lib.rs:140](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L140); [crates/shamir-client-node/src/lib.rs:178](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L178); [crates/shamir-client-node/src/lib.rs:207](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L207); [crates/shamir-client-node/src/lib.rs:311](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L311); [crates/shamir-client-node/src/lib.rs:132](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L132).

<a id="review-3-1"></a>

### Claim 3.1 — No timeouts surfaced and no cancellation: JS callers get the core SDK's known unbounded-hang class as the *only* behavior

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Both timeout options are None, and there is no cancellation surface. A configured peer can stall TLS/SCRAM or withhold a response; Promise.race does not cancel the native operation. The reader can also mark closed/drain between roundtrip's initial check and later registration, leaving a new waiter if its write succeeds. Existing knobs only bound TCP establishment and post-write response waiting, not the complete operation.

Evidence: [crates/shamir-client-node/src/lib.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L130); [crates/shamir-client-node/wrapper.d.ts:35](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.d.ts#L35); [crates/shamir-client/src/client.rs:479](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L479); [crates/shamir-client/src/client.rs:509](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L509); [crates/shamir-client/src/client.rs:407](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L407); [crates/shamir-client/src/client.rs:1240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1240); [crates/shamir-client/src/client.rs:1267](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1267); [crates/shamir-client/src/client.rs:1290](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1290).

<a id="review-3-2"></a>

### Claim 3.2 — No `catch_unwind` discipline at the FFI boundary

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `unverified`.

As a mandatory repair for the cited async panic/hung-promise scenario, this is refuted by exact dependency evidence. napi 3.10.5 monitors the spawned future's JoinError and rejects the promise on unwinding panic. Successful authentication protects the pin expect, and decoding has Result paths with relevant recursion guards. Missing attributes remain literal facts, but do not prove a defect. Synchronous resolver/finalizer panics and non-unwinding failures are not certified safe.

Evidence: [crates/shamir-client-node/Cargo.toml:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/Cargo.toml#L39); [crates/shamir-client-node/src/lib.rs:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L96); [crates/shamir-client/src/client.rs:631](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L631); [crates/shamir-connect/src/client/handshake.rs:266](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L266); [napi 3.10.5 published src/tokio_runtime.rs:256](https://docs.rs/crate/napi/3.10.5/source/src/tokio_runtime.rs); [napi 3.10.5 published src/tokio_runtime.rs:299](https://docs.rs/crate/napi/3.10.5/source/src/tokio_runtime.rs).

<a id="review-3-3"></a>

### Claim 3.3 — Secret copies in the napi struct are not zeroized (`resumption_ticket`, `session_id`, `pin`)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Ticket and session-id snapshots are ordinary owned storage, duplicated from core and retained after close. Ticket getters make additional caller-owned copies. This is local memory-disclosure hygiene, not demonstrated remote extraction. The public-key pin is not secret; wiping JS Buffers is possible but cannot erase every copy.

Evidence: [crates/shamir-client-node/src/lib.rs:85](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L85); [crates/shamir-client-node/src/lib.rs:143](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L143); [crates/shamir-client-node/src/lib.rs:178](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L178); [crates/shamir-client-node/src/lib.rs:311](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L311); [crates/shamir-client/src/client.rs:436](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L436); [docs/guide-docs/client-server-protocol-spec/SESSION_RESUMPTION.md:292](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/SESSION_RESUMPTION.md#L292).

<a id="review-3-4"></a>

### Claim 3.4 — "Zeroised in the native side" overstates what the binding controls

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Invalid address/port/pin validation can return before the ordinary password String is wrapped. Exact napi 3.10.5 String conversion allocates one UTF-8 Vec and transfers it into String; mandatory additional transient native copies are not proven. createScramUser also wraps its password only after awaiting the binding lock. The documentation should scope the eventual Zeroizing guarantee and separately qualify JS copies.

Evidence: [crates/shamir-client-node/src/lib.rs:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L61); [crates/shamir-client-node/src/lib.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L98); [crates/shamir-client-node/src/lib.rs:111](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L111); [crates/shamir-client-node/src/lib.rs:127](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L127); [crates/shamir-client-node/src/lib.rs:260](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L260); [napi 3.10.5 published src/bindgen_runtime/js_values/string.rs:64](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/js_values/string.rs).

<a id="review-3-positive-notes"></a>

### Claim 3.Positive notes — Positive notes (kept for calibration parity)

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Trusted-pin length validation, native authentication delegation, MessagePack exchange and the matching four-code retryable set are supported. TOFU persistence remains caller-managed. These scoped positives do not establish race-free Buffer access, complete credential erasure or impossibility of client/server deployment drift.

Evidence: [crates/shamir-client-node/src/lib.rs:107](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L107); [crates/shamir-client-node/src/lib.rs:136](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L136); [crates/shamir-client-node/wrapper.js:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L39); [crates/shamir-client-ts/src/core/errors.ts:28](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/src/core/errors.ts#L28); [crates/shamir-connect/src/client/handshake.rs:266](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L266).

<a id="review-4-1"></a>

### Claim 4.1 — `close()` blocks on the lock (and therefore on any in-flight request) instead of taking-and-releasing

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The stalled preceding request prevents close from acquiring the lock. Shortening close's own guard lifetime alone cannot fix that dependency. No operational timing or forced-termination scenario was measured.

Evidence: [crates/shamir-client-node/src/lib.rs:207](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L207); [crates/shamir-client-node/src/lib.rs:311](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L311).

Grouping/duplicate: [SUMMARY.md#2.2](SUMMARY.md#review-2-2). This is not an additional independent defect.

<a id="review-4-2"></a>

### Claim 4.2 — Redundant decode + copy on every `repl`/`createScramUser` success, and a Uint8Array copy per `decodeOrThrow`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

new Uint8Array(Buffer) copies the response, and repl/user creation discard decoded results. Exact installed @msgpack/msgpack 3.1.3 returns binary blobs as subarray views rather than parsing their contents. Exact napi 3.10.5 normally transfers Vec storage through an external Buffer, with a copying fallback; a mandatory additional native copy is refuted. Latency/RSS impact remains unmeasured.

Evidence: [crates/shamir-client-node/wrapper.js:82](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L82); [crates/shamir-client-node/wrapper.js:112](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L112); [crates/shamir-client-node/wrapper.js:126](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L126); [crates/shamir-query-types/src/wire/repl.rs:86](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/wire/repl.rs#L86); [@msgpack/msgpack 3.1.3 src/Decoder.ts:772](https://github.com/msgpack/msgpack-javascript/blob/v3.1.3/src/Decoder.ts#L772); [napi 3.10.5 published src/bindgen_runtime/js_values/buffer.rs:401](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/js_values/buffer.rs); [napi 3.10.5 published src/bindgen_runtime/js_values/buffer.rs:556](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/js_values/buffer.rs).

<a id="review-4-positive-notes"></a>

### Claim 4.Positive notes — Positive notes

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Native I/O uses async Rust and the pinned napi async feature maps to tokio_rt. Wrapper encode/decode and copies remain synchronous JS work. Fixed-size getters have fixed copying cost; ticket copying is length-dependent. Neither universal event-loop nonblocking behavior nor pricing correctness follows.

Evidence: [crates/shamir-client-node/Cargo.toml:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/Cargo.toml#L39); [crates/shamir-client-node/wrapper.js:82](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L82); [crates/shamir-client-node/wrapper.js:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L99); [crates/shamir-client-node/src/lib.rs:178](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L178); `napi 3.10.5 published Cargo.toml, async and tokio_rt features`.

<a id="review-5-1"></a>

### Claim 5.1 — *(primary: same as 1.3)* — repl error channel drift: the binding flattens `ReplResponse::Error` into a success buffer, breaking the wire's error taxonomy at the JS boundary

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The protocol variant, code, message and fencing epoch survive unchanged in the raw buffer. Successful Promise resolution is the documented and tested raw-result contract, not taxonomy flattening.

Evidence: [crates/shamir-client-node/src/lib.rs:239](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L239); [crates/shamir-client-node/wrapper.d.ts:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.d.ts#L19); [crates/shamir-query-types/src/wire/repl.rs:97](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/wire/repl.rs#L97); [tests/e2e/tests/16-replication.test.js:235](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/tests/e2e/tests/16-replication.test.js#L235).

Grouping/duplicate: [SUMMARY.md#1.3](SUMMARY.md#review-1-3). This is not an additional independent defect.

<a id="review-5-2"></a>

### Claim 5.2 — "Mirrors the Rust SDK 1:1" is false; resumption ticket getters dead-end with no resume path

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The API is a subset despite its parity statement: resume, connect_local, subscription/cursor APIs, DDL status and server_query_version are absent. Ticket getters exist without an input path, but their docs do not instruct persistence. Correcting the subset documentation is a valid resolution; blindly exposing current core resume is not identity-safe.

Evidence: [crates/shamir-client-node/src/lib.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L3); [crates/shamir-client-node/src/lib.rs:176](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L176); [crates/shamir-client-node/wrapper.d.ts:49](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.d.ts#L49); [crates/shamir-client/src/client.rs:862](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L862); [crates/shamir-client/src/client.rs:987](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L987); [crates/shamir-client/src/client.rs:1194](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1194).

<a id="review-5-3"></a>

### Claim 5.3 — `execute(object)` teaches hand-assembled wire shapes; no builder, no exported BatchRequest/BatchResponse types

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The hand-assembled example and object-to-object declaration remain underspecified. Existing Node tests prove structural integration with TS builders for their exercised values; different client classes do not prohibit passing built objects. Batch.execute requires executeWithTouch, which is absent. Omitted return_all is valid. Full builder compatibility additionally requires resolving the BigInt codec gap, not merely documenting batch.build().

Evidence: [CLAUDE.md:546](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L546); [crates/shamir-client-node/src/lib.rs:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L19); [crates/shamir-client-node/wrapper.d.ts:58](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.d.ts#L58); [crates/shamir-client-ts/src/core/builders/batch.ts:334](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/src/core/builders/batch.ts#L334); [crates/shamir-client-ts/src/core/builders/batch.ts:370](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/src/core/builders/batch.ts#L370); [tests/e2e/helpers/fixtures.js:23](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/tests/e2e/helpers/fixtures.js#L23); [crates/shamir-query-types/src/batch/batch_request.rs:90](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/batch_request.rs#L90).

<a id="review-5-4"></a>

### Claim 5.4 — `set_replicator` success buffer is fabricated from caller inputs, not the server echo

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Reconstruction occurs, but the current server returns those exact user/on inputs and the public wrapper returns void. No caller can persist a fabricated canonical name through that result. Future server normalization is a prospective compatibility consideration, not a current violated guarantee requiring a core signature change.

Evidence: [crates/shamir-server/src/db_handler/admin.rs:393](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L393); [crates/shamir-client/src/client.rs:1147](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1147); [crates/shamir-client-node/src/lib.rs:297](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L297); [crates/shamir-client-node/wrapper.js:148](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L148); [crates/shamir-client-node/wrapper.d.ts:66](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.d.ts#L66).

<a id="review-5-5"></a>

### Claim 5.5 — Generated loader's platform packages are unpublished/undeclared: every non-win32-x64-msvc path is dead

Status: `unverified`. Current risk: `low`.

Prior-cycle decision: `unverified`.

No optionalDependencies are declared, but every listed supported platform can load a locally supplied native file first. No binary is committed or presently in the binding directory. Publication/packed-artifact claims remain unverified, and the root README explicitly describes source-first distribution.

Evidence: [README.md:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/README.md#L18); [crates/shamir-client-node/package.json:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/package.json#L20); [crates/shamir-client-node/package.json:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/package.json#L33); [crates/shamir-client-node/index.js:126](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/index.js#L126); [crates/shamir-client-node/index.js:193](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/index.js#L193); [crates/shamir-client-node/index.js:283](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/index.js#L283); [.gitignore:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/.gitignore#L29).

<a id="review-6-1"></a>

### Claim 6.1 — *(primary: same as 2.2/4.1)* — `close()` is not a lifecycle-safe operation under stalls

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Close is idempotent after completion, but cannot interrupt an unanswered request holding the same mutex. This is the same shutdown-responsiveness mechanism as 2.2.

Evidence: [crates/shamir-client-node/src/lib.rs:207](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L207); [crates/shamir-client-node/src/lib.rs:308](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L308); [crates/shamir-client-node/src/lib.rs:311](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L311).

Grouping/duplicate: [SUMMARY.md#2.2](SUMMARY.md#review-2-2). This is not an additional independent defect.

<a id="review-6-2"></a>

### Claim 6.2 — Infrastructure errors lose all taxonomy crossing the boundary

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Variant-specific core information is reduced to display text, but this is an explicitly documented plain-error boundary; stable infrastructure variant codes are not promised. Published napi 3.10.5 supplies GenericFailure as the generic error code, so absence of any code/name is also false. A richer taxonomy is an optional API enhancement, separate from the promised DB-error properties.

Evidence: [crates/shamir-client-node/src/lib.rs:340](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L340); [crates/shamir-client-node/src/lib.rs:351](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L351); [crates/shamir-client-node/wrapper.d.ts:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.d.ts#L19); [crates/shamir-client/src/error.rs:5](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/error.rs#L5); `napi 3.10.5 published src/error.rs, Error::from_reason and JsError conversion`.

<a id="review-6-3"></a>

### Claim 6.3 — No Drop/finalization: a GC'd-without-close client leaks the TCP connection and server session until expiry

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Exact napi finalization consumes the boxed binding object, whose owned client invokes core Drop and aborts the reader. Writer ownership is also released. This cleanup predates the audit. Pending calls may delay GC, and server session retention occurs after explicit close too; neither establishes the asserted missing-finalizer leak.

Evidence: [crates/shamir-client-node/src/lib.rs:82](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L82); [crates/shamir-client/src/client.rs:1316](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1316); [napi 3.10.5 published src/bindgen_runtime/mod.rs:51](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/mod.rs); `napi-derive-backend 5.1.2 published src/codegen/struct.rs, generated ObjectFinalize implementation`; [crates/shamir-server/src/connection/request_loop.rs:410](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L410); [crates/shamir-connect/src/server/session.rs:539](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/session.rs#L539).

<a id="review-6-4"></a>

### Claim 6.4 — `encode_db_error`'s own failure path degrades to a plain Error (acceptable, but untested and undocumented)

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

The plain-error fallback is explicit and reasonable, not an established violation of the promised DB-error surface. Exact rmp-serde 1.3.1's fallible Vec writer can return a reservation error for supported owned-string markers, so the fallback must not be called intrinsically unreachable. Its presence avoids an unconditional panic; preserve it and separately test real marker shape/property compatibility. This is conditional codec/resource behavior, not a claim about every separately resolved Node dependency graph.

Evidence: [crates/shamir-client-node/src/lib.rs:361](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L361); [crates/shamir-query-types/src/wire/db_message.rs:316](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/wire/db_message.rs#L316); [crates/shamir-transport-tcp/src/framing.rs:15](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L15).

<a id="review-7-1"></a>

### Claim 7.1 — `src/lib.rs` carries multiple primary exports; error-mapping helpers belong in a sibling file

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The project permits a closely coupled group. Connection options, the binding class and its private boundary helpers form that group. Private helper functions are not unrelated primary exports; splitting is optional organization.

Evidence: [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505); [crates/shamir-client-node/src/lib.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L52); [crates/shamir-client-node/src/lib.rs:81](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L81); [crates/shamir-client-node/src/lib.rs:351](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L351).

<a id="review-7-2"></a>

### Claim 7.2 — Test layout violates the repo convention: no `tests/` directory, no automated runner

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The Node runner and nightly registration exist. There is no Rust test module whose layout violates the per-module rule; lack of Rust unit coverage is a coverage gap, not an orphaned-module/layout violation. Workspace exclusion is intentional.

Evidence: [CLAUDE.md:573](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L573); [Cargo.toml:14](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.toml#L14); [tests/e2e/helpers/runner.js:106](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/tests/e2e/helpers/runner.js#L106); [tests/e2e/e2e.test.js:51](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/tests/e2e/e2e.test.js#L51); [.github/workflows/ts-e2e-nightly.yml:162](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/.github/workflows/ts-e2e-nightly.yml#L162).

<a id="review-7-3"></a>

### Claim 7.3 — Sanctioned-exception comment present at `repl` but missing at the identical `execute` rmp-serde boundary

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Both operations deserialize incoming FFI bytes rather than constructing queries. Only repl explicitly explains the sanctioned exception. This is scoped comment hygiene, not a decoding or builder-construction defect.

Evidence: [CLAUDE.md:554](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L554); [crates/shamir-client-node/src/lib.rs:205](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L205); [crates/shamir-client-node/src/lib.rs:231](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L231).

<a id="review-7-4"></a>

### Claim 7.4 — Stale/self-contradictory naming: comments say the wrapper is `index.js`; it is `wrapper.js`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Native comments and the copied proof point to the generated loader as the decoder, although package.json selects wrapper.js. The stale references remain and can misdirect maintenance.

Evidence: [crates/shamir-client-node/proof-typed-errors.js:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/proof-typed-errors.js#L10); [crates/shamir-client-node/proof-typed-errors.js:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/proof-typed-errors.js#L67); [crates/shamir-client-node/src/lib.rs:213](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L213); [crates/shamir-client-node/src/lib.rs:336](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L336); [crates/shamir-client-node/package.json:5](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/package.json#L5).

<a id="review-executive-summary"></a>

### Claim Executive summary — Executive summary

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

The dead-wrapper and repl-error shipping blockers are contradicted by exact factory semantics and the intentional raw-result contract. Address and availability defects remain, and fresh ownership/integer-fidelity evidence requires attention. This bounded inspection cannot supply a blanket shippability or runtime-success verdict.

Evidence: [crates/shamir-client-node/Cargo.toml:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/Cargo.toml#L39); [crates/shamir-client-node/src/lib.rs:100](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L100); [crates/shamir-client-node/src/lib.rs:132](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L132); [crates/shamir-client-node/wrapper.js:93](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L93); [tests/e2e/tests/16-replication.test.js:235](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/tests/e2e/tests/16-replication.test.js#L235).

<a id="review-finding-counts"></a>

### Claim Finding counts — Finding counts

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Historical arithmetic counts 28 lens rows and 25 groups, not 25 proven defects. This revalidation yields 16 open, 8 refuted, 1 unverified and 7 not-applicable current claim rows, with no fixed/partially-fixed rows. These include duplicate roots, documentation issues and structural cost observations; the two fresh mechanisms are additional.

Evidence: [docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-client-node/SUMMARY.md:12](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-client-node/SUMMARY.md#L12); [docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-client-node/SUMMARY.md:344](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-client-node/SUMMARY.md#L344); [docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-client-node/SUMMARY.md:1071](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-client-node/SUMMARY.md#L1071).

## Revalidated plan decisions

| Plan decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 18 | 10 | 0 | 0 | 0 | 1 | 7 |

Historical P0/P1/P2 numbering is an identifier, not a current release mandate. The reasons below include completion status, safety qualifications and discriminating acceptance requirements.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `not-applicable`.

Prior-cycle decision: `not-applicable`.

Receiver-sensitive construction already supports the wrapper. A public connect/prototype regression oracle is useful, but a static override/reprototype operation is not a required repair.

Evidence: [crates/shamir-client-node/wrapper.js:93](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L93); [napi 3.10.5 published src/bindgen_runtime/callback_info.rs:199](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/callback_info.rs); [tests/e2e/tests/02-basic-crud.test.js:19](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/tests/e2e/tests/02-basic-crud.test.js#L19).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `not-applicable`.

Prior-cycle decision: `not-applicable`.

Converting repl_kind:error into the DB-error marker would break the registered raw-response contract and can lose leader_epoch. Preserve the two error channels unless making an explicitly versioned API change.

Evidence: [crates/shamir-query-types/src/wire/repl.rs:97](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/wire/repl.rs#L97); [crates/shamir-client-node/wrapper.js:104](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L104); [tests/e2e/tests/16-replication.test.js:235](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/tests/e2e/tests/16-replication.test.js#L235).

<a id="plan-p0-3"></a>

### Plan P0.3 — P0.3

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

DNS resolution and ordinary IPv6 construction remain absent. IpAddr plus SocketAddr::new is suitable, but the recipe must preserve previously accepted bracketed IPv6, keep TLS serverName/pin semantics independent of address resolution, and define address-selection/deadline behavior.

Evidence: [crates/shamir-client-node/src/lib.rs:53](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L53); [crates/shamir-client-node/src/lib.rs:100](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L100); [crates/shamir-client-node/src/lib.rs:125](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L125).

<a id="plan-p0-4"></a>

### Plan P0.4 — P0.4

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Both knobs remain None. Merely surfacing them leaves DNS/full TLS-SCRAM, lock waits and writes outside the existing budgets. Define whole-operation terminal deadlines and pending-entry cleanup; a timed-out write/request must not be treated as proof that the server performed no mutation.

Evidence: [crates/shamir-client-node/src/lib.rs:132](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L132); [crates/shamir-client/src/client.rs:479](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L479); [crates/shamir-client/src/client.rs:1277](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1277); [crates/shamir-client/src/client.rs:1290](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1290).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Taking the client then releasing close's guard shortens only its own holding scope; it cannot bypass a request already holding the mutex. Coordinate with ownership redesign. Core reader abort skips its normal EOF drain, so explicit pending settlement and admission closure are required before promising ConnectionClosed delivery.

Evidence: [crates/shamir-client-node/src/lib.rs:207](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L207); [crates/shamir-client-node/src/lib.rs:311](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L311); [crates/shamir-client/src/client.rs:407](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L407); [crates/shamir-client/src/client.rs:1306](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1306).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

The exclusive guard still spans roundtrips. Shared clone-out must include a shutdown mechanism usable while references are active; Arc::try_unwrap is not reliable under those conditions. Preserve request correlation and define cancellation of partially written frames as terminal transport cleanup.

Evidence: [crates/shamir-client-node/src/lib.rs:82](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L82); [crates/shamir-client-node/src/lib.rs:215](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L215); [crates/shamir-client/src/client.rs:1233](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1233); [crates/shamir-client/src/client.rs:1306](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1306).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `not-applicable`.

Prior-cycle decision: `unverified`.

No concrete cited panic mechanism requires blanket annotations: pinned napi already rejects unwinding async-body panics, and the pin expect has a proved invariant. Generated catch_unwind around setup is not a substitute for polling-time handling, nor can it fix Buffer undefined behavior or non-unwinding failures. Sync resolver/finalizer containment remains a separately scoped question.

Evidence: [crates/shamir-client-node/Cargo.toml:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/Cargo.toml#L39); [crates/shamir-connect/src/client/handshake.rs:266](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L266); [napi 3.10.5 published src/tokio_runtime.rs:299](https://docs.rs/crate/napi/3.10.5/source/src/tokio_runtime.rs); `napi-derive-backend 5.1.2 published src/codegen/fn.rs, native_call and catch_unwind generation`.

<a id="plan-p1-8"></a>

### Plan P1.8 — P1.8

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Actual typed-property and cross-language marker oracles remain absent. Extend the existing public-wrapper lane; assert code/retryable/detail and exercise the valid 16-byte raw-ID collision. Use Rust-produced bytes or independent golden fixtures. Exporting private decoder helpers or copying detection logic is unnecessary and weakens the oracle.

Evidence: [tests/e2e/tests/09-errors.test.js:33](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/tests/e2e/tests/09-errors.test.js#L33); [crates/shamir-client-node/proof-typed-errors.js:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/proof-typed-errors.js#L67); [crates/shamir-client-node/src/lib.rs:361](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L361); [.github/workflows/ts-e2e-nightly.yml:162](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/.github/workflows/ts-e2e-nightly.yml#L162).

<a id="plan-p1-9"></a>

### Plan P1.9 — P1.9

Status: `not-applicable`.

Prior-cycle decision: `confirmed-open`.

Stable infrastructure variant codes would be an API enhancement, not completion of a currently promised contract. Error::new(GenericFailure, ...) still supplies only the generic code; setting properties requires a real JS-value/resolver mechanism, not a property operation on an ordinary Rust napi::Error.

Evidence: [crates/shamir-client-node/src/lib.rs:340](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L340); [crates/shamir-client-node/src/lib.rs:351](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L351); `napi 3.10.5 published src/error.rs, JsError conversion`; [napi 3.10.5 published src/tokio_runtime.rs:256](https://docs.rs/crate/napi/3.10.5/source/src/tokio_runtime.rs).

<a id="plan-p2-10"></a>

### Plan P2.10 — P2.10

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Passing Buffer directly avoids the explicit copy, but renaming a try-decode helper does not eliminate parsing/materialization. Marker detection must validate the complete typed kind/code/message shape, thereby excluding the 16-byte success collision. Preserve raw user IDs and intentional repl Error results; do not introduce an incompatible response envelope casually.

Evidence: [crates/shamir-client-node/wrapper.js:82](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L82); [crates/shamir-client-node/wrapper.js:112](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L112); [crates/shamir-client-node/wrapper.js:126](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L126); [crates/shamir-client-node/src/lib.rs:361](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L361); [@msgpack/msgpack 3.1.3 src/utils/typedArrays.ts:7](https://github.com/msgpack/msgpack-javascript/blob/v3.1.3/src/utils/typedArrays.ts#L7).

<a id="plan-p2-11"></a>

### Plan P2.11 — P2.11

Status: `not-applicable`.

Prior-cycle decision: `not-applicable`.

Default napi finalization and core Drop already provide ownership cleanup. Adding an asynchronous Drop-close task is not required and would create another task-lifetime/runtime dependency. Explicit close remains the clean protocol-shutdown path.

Evidence: [napi 3.10.5 published src/bindgen_runtime/mod.rs:51](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/mod.rs); [crates/shamir-client/src/client.rs:1316](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1316); [crates/shamir-client/src/client.rs:1306](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1306).

<a id="plan-p2-12"></a>

### Plan P2.12 — P2.12

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

The subset/parity documentation remains inaccurate. Correcting it is safe. Directly routing a new Node resume method to current core resume is not: its accept-any-cert TLS configuration is followed by unsigned ResumeOk acceptance, and pinned_hash is only cached. Establish authenticated server identity and bounded lifecycle semantics before exposing that path.

Evidence: [crates/shamir-client-node/src/lib.rs:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L3); [crates/shamir-client/src/client.rs:862](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L862); [crates/shamir-client/src/client.rs:937](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L937); [crates/shamir-client/src/wire_frames.rs:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/wire_frames.rs#L52); [crates/shamir-transport-tcp/src/tls.rs:63](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L63); [crates/shamir-transport-tcp/src/tls.rs:130](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/tls.rs#L130).

<a id="plan-p2-13"></a>

### Plan P2.13 — P2.13

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Batch declarations and a Node-facing builder recipe remain missing. Built plain objects integrate structurally, while Batch.execute still needs executeWithTouch. Re-exported TS types must not promise unsupported bigint values; resolve codec fidelity alongside full compatibility claims.

Evidence: [crates/shamir-client-node/wrapper.d.ts:58](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.d.ts#L58); [crates/shamir-client-ts/src/core/builders/batch.ts:334](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/src/core/builders/batch.ts#L334); [crates/shamir-client-ts/src/core/builders/batch.ts:370](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/src/core/builders/batch.ts#L370); [crates/shamir-client-ts/src/core/framing.ts:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/src/core/framing.ts#L61); [crates/shamir-client-node/wrapper.js:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L99).

<a id="plan-p2-14"></a>

### Plan P2.14 — P2.14

Status: `not-applicable`.

Prior-cycle decision: `confirmed-open`.

The current server echo equals the reconstructed acknowledgement and the wrapper returns void. No semantic repair requires changing the core signature or success wire shape. Clarifying that reconstruction occurs is optional documentation hygiene.

Evidence: [crates/shamir-server/src/db_handler/admin.rs:393](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L393); [crates/shamir-client/src/client.rs:1147](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1147); [crates/shamir-client-node/src/lib.rs:297](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L297); [crates/shamir-client-node/wrapper.js:148](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L148).

<a id="plan-p2-15"></a>

### Plan P2.15 — P2.15

Status: `unverified`.

Prior-cycle decision: `unverified`.

Required publishing changes depend on an actual artifact/distribution contract not established here. Local native-file loading supports non-Windows branches. Trimming build targets would remove supported build possibilities without proving a packaging repair.

Evidence: [README.md:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/README.md#L18); [crates/shamir-client-node/package.json:20](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/package.json#L20); [crates/shamir-client-node/index.js:283](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/index.js#L283); [tests/e2e/README.md:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/tests/e2e/README.md#L39).

<a id="plan-p2-16"></a>

### Plan P2.16 — P2.16

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Credential snapshots remain ordinary storage. Drop-zeroizing owned ticket/session copies can preserve immutable getter behavior; clearing them on close requires an explicit post-close getter policy. Wrap native password ownership before validation/await where feasible. The public pin need not be treated as a secret, and JS-copy erasure cannot be universally promised.

Evidence: [crates/shamir-client-node/src/lib.rs:85](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L85); [crates/shamir-client-node/src/lib.rs:143](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L143); [crates/shamir-client-node/src/lib.rs:127](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L127); [crates/shamir-client-node/src/lib.rs:260](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L260); [crates/shamir-client-node/src/lib.rs:311](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L311).

<a id="plan-p2-17"></a>

### Plan P2.17 — P2.17

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Stale wrapper names, the exception explanation and version-policy note remain actionable nits. Splitting closely coupled private helpers is optional. Correct documentation without an unsolicited version bump.

Evidence: [crates/shamir-client-node/src/lib.rs:205](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L205); [crates/shamir-client-node/src/lib.rs:336](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L336); [crates/shamir-client-node/proof-typed-errors.js:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/proof-typed-errors.js#L10); [crates/shamir-client-node/Cargo.toml:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/Cargo.toml#L3); [crates/shamir-client-node/package.json:3](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/package.json#L3); [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505).

<a id="plan-p2-18"></a>

### Plan P2.18 — P2.18

Status: `not-applicable`.

Prior-cycle decision: `confirmed-open`.

No documented error-precedence obligation establishes a defect to close. A policy change is optional; decoding while holding the lifecycle mutex would worsen contention and should not be presented as a neutral repair.

Evidence: [crates/shamir-client-node/src/lib.rs:205](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L205); [crates/shamir-client-node/src/lib.rs:207](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L207); [crates/shamir-client-node/src/lib.rs:233](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L233).

## Additional observations

| Observation decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 2 | 2 | 0 | 0 | 0 | 0 | 0 |

Existing observation IDs remain stable. New cycle-2 rows follow them; cross-module duplicates and extra triggers are grouped explicitly. None is an implemented fix.

<a id="observation-new-1"></a>

### Observation NEW.1 — Caller-owned Buffer aliases are read on async workers without mutation isolation

Status: `confirmed-open`. Current risk: `high`.

Additional observation in this independent cycle; it may overlap an existing root.

Public repl forwards the caller's Buffer unchanged; connect also retains the caller's trustedPin Buffer. Generated async dispatch runs Rust reading/copying of these bytes on a Tokio worker. Exact napi 3.10.5 FromNapiValue obtains the original JS pointer and holds only a lifetime reference; its Send comment explicitly acknowledges unsynchronized JS mutation as undefined behavior. Ordinary JS can reuse or overwrite the buffer while the worker reads it. This is a local API ownership/safety witness, not proof of remote exploitability or observed crashing. Snapshot ownership must be established synchronously before off-thread dispatch; copying only inside the worker still races. Published source: https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/js_values/buffer.rs.

Evidence: [crates/shamir-client-node/Cargo.toml:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/Cargo.toml#L39); [crates/shamir-client-node/wrapper.js:109](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L109); [crates/shamir-client-node/src/lib.rs:107](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L107); [crates/shamir-client-node/src/lib.rs:233](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L233); [napi 3.10.5 published src/bindgen_runtime/js_values/buffer.rs:390](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/js_values/buffer.rs); [napi 3.10.5 published src/bindgen_runtime/js_values/buffer.rs:449](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/js_values/buffer.rs); [napi 3.10.5 published src/bindgen_runtime/js_values/buffer.rs:490](https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/js_values/buffer.rs); [napi-derive-backend 5.1.2 published src/codegen/fn.rs:265](https://docs.rs/crate/napi-derive-backend/5.1.2/source/src/codegen/fn.rs).

<a id="observation-new-2"></a>

### Observation NEW.2 — Default object-level MessagePack conversion loses integer fidelity and rejects BigInt inputs

Status: `confirmed-open`. Current risk: `high`.

Additional observation in this independent cycle; it may overlap an existing root.

For inspected @msgpack/msgpack 3.1.3, permitted by the binding's ^3.0.0 range, both codec calls omit useBigInt64. A legitimate Rust Int value 9007199254740993 in a record response is emitted as a 64-bit integer but decoded to Number 9007199254740992. A Node read followed by rewriting that value can therefore corrupt data. Genuine bigint inputs are rejected before reaching Rust. Values above i64::MAX represented as Big strings are a different, preserved case. A discriminating oracle needs independently produced positive/negative unsafe-integer fixtures and actual wrapper input tests; a symmetric default-JS roundtrip misses the original value. A fix must also preserve integer markers for safe wide Number fields, not blindly enable BigInt64 and change them to float64. Published package source label: @msgpack/msgpack 3.1.3 src/Decoder.ts, src/Encoder.ts and src/utils/int.ts; package archive provenance is recorded in the TS lockfile.

Evidence: [crates/shamir-client-node/package.json:34](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/package.json#L34); [crates/shamir-client-node/wrapper.js:82](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L82); [crates/shamir-client-node/wrapper.js:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/wrapper.js#L99); [crates/shamir-types/src/types/value.rs:70](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/value.rs#L70); [crates/shamir-types/src/types/value.rs:142](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/value.rs#L142); [crates/shamir-client-ts/src/core/framing.ts:61](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/src/core/framing.ts#L61); [crates/shamir-client-ts/src/core/framing.ts:71](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/src/core/framing.ts#L71); [crates/shamir-client-ts/package-lock.json:438](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-ts/package-lock.json#L438); [@msgpack/msgpack 3.1.3 src/Decoder.ts:243](https://github.com/msgpack/msgpack-javascript/blob/v3.1.3/src/Decoder.ts#L243); [@msgpack/msgpack 3.1.3 src/Decoder.ts:522](https://github.com/msgpack/msgpack-javascript/blob/v3.1.3/src/Decoder.ts#L522); [@msgpack/msgpack 3.1.3 src/Encoder.ts:189](https://github.com/msgpack/msgpack-javascript/blob/v3.1.3/src/Encoder.ts#L189); [@msgpack/msgpack 3.1.3 src/utils/int.ts:28](https://github.com/msgpack/msgpack-javascript/blob/v3.1.3/src/utils/int.ts#L28).

## Evidence and recipe corrections

- Claim 1.4's partially-fixed label conflates pre-existing counter-evidence with a repair. Nightly registration originated in b90a4472; core Drop cleanup originated in d13ca5c0. The relevant Node source diff from 6df2afa9 to frozen HEAD is empty.
- The current assertion that user-id decoding is only overhead misses a valid 16-byte kind:error map. Validate the complete marker shape; a kind-only discriminator is ambiguous with unrestricted raw IDs.
- Exact napi 3.10.5 source settles ordinary async-body unwinding behavior: the task is monitored and its deferred rejected. Do not retain historical hung-promise allegations from unspecified versions as evidence. This is not a universal FFI-safety assurance.
- napi-derive 3.5.10 is exactly pinned, but its published backend requirement beginning at 5.1.2 is a compatible range. Calling the whole transitive graph exactly pinned is inaccurate without the excluded binding's lock.
- The historical assumption that Buffer::from(Vec) necessarily copies is false for napi 3.10.5's normal external-buffer path. Its fallback copies. The JS new Uint8Array(Buffer) copy remains explicit.
- Raw repl Error results intentionally preserve leader_epoch. A generic exception conversion is an API change, not a repair; the production server capability gate confirms the registered denial scenario.
- Error precedence and infrastructure-code enrichment are optional API policies under the current documented surface. Synthetic setReplicator acknowledgement fields match the current server and are discarded by the void wrapper; canonical-name corruption is not a current caller-visible mechanism.
- The resume recipe has a concrete unresolved identity problem: core caches the supplied pin without verifying it after accept-any-cert TLS and unsigned ResumeOk. Documentation-only subset correction is safe; direct feature exposure is not yet a verified repair.
- TS builder compatibility is established only for exercised shapes. The binding's default MessagePack encoder cannot accept genuine bigint inputs, so a universal execute(db, batch.build()) assurance requires qualification.
- The Rust module example requires shamir-client-node, while package.json names the package shamir-client. Also, wrapper.d.ts's header suggests object-level repl/user creation although those methods retain raw Buffer results.
- No method here proves synchronous JS work is nonblocking or serialization is priced correctly. Ticket getter cost depends on ticket length. Existing toy-value CRUD does not prove complete numeric-domain roundtrip fidelity.
- The root's source-first alpha statement precludes inferring a publishing failure from undeclared platform packages alone. Loader local-file branches support platform builds; no committed/shipped MSVC binary was found.
- Current claim totals should be updated to 16 open, 8 refuted, 1 unverified and 7 not-applicable; plan totals become 10 open, 1 unverified and 7 not-applicable. Neither tally is a unique runtime-defect count.
- Parent acceptance: rmp-serde 1.3.1 FallibleWriter returns try_reserve failures as I/O errors. A supported marker can therefore reach the plain-error fallback under reservation failure; N/A means the fallback is reasonable, not that allocation-error return paths are impossible.

## Module scope and limitations

Coverage: 1 assigned documents, 32 current claim rows, 18 plan rows, 0 pre-existing observation rows; 2 added observation rows in this cycle. Counts are calculated from the accepted rows.

Assigned documents: [SUMMARY.md](SUMMARY.md).

- Read-only source, documentation, history and dependency inspection; no builds, tests, reproductions or performance measurements.
- The excluded binding has no committed or present Cargo.lock. Its napi 3.10.5 and napi-derive 3.5.10 pins were inspected from cached published archives. napi-derive's backend requirement is a compatible range beginning at 5.1.2, not an exact transitive pin; backend 5.1.2 was inspected.
- Frozen workspace versions are tokio 1.49.0, serde 1.0.228, rmp-serde 1.3.1 and zeroize 1.8.2; these do not establish the binding's complete separately resolved graph. The rmp-serde archive SHA-256 matches the frozen lock checksum.
- Exact installed harness source for @msgpack/msgpack 3.1.3 was inspected. This version satisfies the binding's ^3.0.0 requirement, but the binding has no present npm installation or lockfile. Codec findings specify this inspected resolution rather than asserting every deployment uses it.
- No native binary is committed or presently in the binding directory. Published npm artifacts, actual GC scheduling, CI results and deployed configurations were not verified.
- The checkout acquired concurrent dependency, toolchain and CI metadata changes. Canonical affected metadata was re-read from frozen HEAD; assigned reports and reviewed Node/core implementation files remained unchanged. This reviewer wrote nothing.
- Coverage is complete for the assigned claim checklist, not an exhaustive Rust Intel or upstream-core audit.

## Guarantee checks

- **The project is source-first alpha, and the Node binding is built separately.** — `supported`. The binding is excluded from workspace selection. Its toolchain declares stable plus an MSVC target, whereas frozen root metadata pins 1.94.0. Neither published binaries nor an exactly reproducible separately resolved binding are established. Reference: README.md:16; Cargo.toml:14; crates/shamir-client-node/rust-toolchain.toml:7.
- **Inherited connect constructs instances with the exported wrapper subclass prototype.** — `supported`. Published napi 3.10.5 callback_info.rs restores the factory receiver and passes it to napi_new_instance. Inspected backend 5.1.2 generates receiver-preserving cb.factory calls. Sources: https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/callback_info.rs and https://docs.rs/crate/napi-derive-backend/5.1.2/source/src/codegen/fn.rs. This is source counter-evidence, not an executed native-artifact result. Reference: crates/shamir-client-node/src/lib.rs:96; crates/shamir-client-node/wrapper.js:93; crates/shamir-client-node/Cargo.toml:39.
- **DbResponse::Error becomes ShamirDbError with code, detail and retryable on wrapped buffer-returning methods.** — `supported`. The named MessagePack marker and wrapper property assignments agree structurally. The four retryable codes match the TS SDK. Existing error tests do not independently prove these properties across the actual native boundary. Reference: crates/shamir-query-types/src/wire/db_message.rs:280; crates/shamir-client-node/src/lib.rs:361; crates/shamir-client-node/wrapper.js:61.
- **Every raw user-id success buffer is distinguishable from a DB-error marker.** — `diverges`. A valid 16-byte ID can itself encode a map containing kind:error. The detector validates only kind, not the required string code/message fields. The directory generates unrestricted random bytes, rather than UUID-version-filtered bytes. Claim 1.5 contains the concrete witness. Reference: crates/shamir-client-node/wrapper.js:82; crates/shamir-client-node/wrapper.js:123; crates/shamir-server/src/user_directory.rs:513.
- **Replication-layer Error replies remain raw protocol results carrying leader_epoch.** — `supported`. The server's capability-denial path returns ReplResponse::Error. Core and binding preserve that variant, and the registered denial test requires normal resolution with its code and fencing epoch. DbResponse::Error exceptions are a separate contract. Reference: crates/shamir-query-types/src/wire/repl.rs:66; crates/shamir-server/src/db_handler/repl_handler.rs:52; tests/e2e/tests/16-replication.test.js:235.
- **ConnectOptions.host accepts the documented DNS hostname and ordinary IPv6 host literal.** — `diverges`. SocketAddr parsing performs no DNS lookup. db.example.com and unbracketed ::1 fail; IPv4 and bracketed IPv6 remain usable. Reference: crates/shamir-client-node/src/lib.rs:53; crates/shamir-client-node/src/lib.rs:100.
- **The binding mirrors the Rust client's concurrent request and API capabilities.** — `diverges`. An exclusive binding mutex spans every roundtrip, defeating core multiplexing. Resume, local IPC connection, push subscription, cursor streaming, DDL-status and query-version capabilities are also absent. Reference: crates/shamir-client-node/src/lib.rs:3; crates/shamir-client-node/src/lib.rs:193; crates/shamir-client/src/client.rs:418.
- **Close can terminate the client independently of an unanswered request.** — `diverges`. An unanswered request retains the mutex with no response deadline, and close cannot acquire it. Moving only close's shutdown await outside its guard does not remove this preceding-holder dependency. Reference: crates/shamir-client-node/src/lib.rs:311; crates/shamir-client-node/src/lib.rs:132; crates/shamir-client/src/client.rs:1290.
- **An otherwise collectible native client has ownership-based cleanup without a custom binding Drop.** — `supported`. Published napi 3.10.5 raw_finalize_unchecked consumes the boxed object through ObjectFinalize; owned fields then drop, invoking core reader abort and writer ownership cleanup. Source: https://docs.rs/crate/napi/3.10.5/source/src/bindgen_runtime/mod.rs. Pending native calls can retain references and delay collectibility; immediate GC or session eviction is not promised. Reference: crates/shamir-client-node/src/lib.rs:82; crates/shamir-client/src/client.rs:1316.
- **Missing catch_unwind attributes imply ordinary async-body panics escape or leave promises pending.** — `diverges`. Published napi 3.10.5 execute_tokio_future_with_finalize_callback awaits the spawned task and rejects its deferred on an unwinding panic. The cited pin expect is protected by successful authentication. Source: https://docs.rs/crate/napi/3.10.5/source/src/tokio_runtime.rs. This does not certify synchronous resolver/finalizer paths, OOM, stack overflow or undefined behavior. Reference: crates/shamir-client-node/Cargo.toml:39; crates/shamir-client-node/src/lib.rs:96; crates/shamir-connect/src/client/handshake.rs:266.
- **Native credential storage and password conversion are comprehensively zeroized.** — `diverges`. Ticket/session snapshots are ordinary storage retained after close. Connect validation can drop an ordinary password String before Zeroizing construction. Published napi 3.10.5 string.rs converts through one UTF-8 Vec reused by String; additional transient native copies are not established. The pin is public, and JS-owned copies require separate qualifications. Reference: crates/shamir-client-node/src/lib.rs:61; crates/shamir-client-node/src/lib.rs:85; crates/shamir-client-node/src/lib.rs:127.
- **Caller-owned Buffer arguments are safe to read on the async worker while JavaScript retains mutable aliases.** — `diverges`. Exact napi 3.10.5 Buffer conversion references the original JS allocation without copying or preventing writes. Its Send safety comment explicitly identifies unsynchronized JS modification as undefined behavior. The public repl and trustedPin paths expose this ownership gap. Reference: crates/shamir-client-node/src/lib.rs:107; crates/shamir-client-node/src/lib.rs:233; crates/shamir-client-node/wrapper.js:109.
- **Object-level MessagePack exchange preserves all valid 64-bit integer values.** — `diverges`. With inspected @msgpack/msgpack 3.1.3, default int64/uint64 decoding returns Number and rounds unsafe integers; default encoding rejects genuine bigint inputs. Rust's Int representation remains exact. The separate Big-as-decimal-string representation does not protect Int values between 2^53 and i64::MAX. Reference: crates/shamir-types/src/types/value.rs:70; crates/shamir-client-node/wrapper.js:82; crates/shamir-client-node/wrapper.js:99.
- **Malformed MessagePack necessarily follows an unbounded recursion path.** — `diverges`. Exact rmp-serde 1.3.1 any_inner guards array/map/ext descent with a default depth counter of 1024. Struct/sequence and QueryValue deserialize_any paths use it; flattened buffering and internally tagged repl input also consume container data through guarded dispatch. deserialize_enum has a distinct path without the same outer guard, so this is not a universal parser-safety certification or a measured stack threshold. Source: https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs. Reference: Cargo.lock:2949; crates/shamir-types/src/types/value.rs:283; crates/shamir-query-types/src/batch/batch_op.rs:262.
- **Registered Node tests exercise the public wrapper and prove typed-error properties.** — `diverges`. Public-wrapper connect/CRUD tests are registered in the Windows nightly lane and can catch a raw-Buffer/prototype regression. Error tests check message text, not code/retryable assignments; removing those assignments while preserving messages would evade them. The manual proof copies marker detection. Reference: tests/e2e/package.json:14; tests/e2e/e2e.test.js:35; tests/e2e/helpers/runner.js:106; tests/e2e/tests/09-errors.test.js:33.
- **The proposed direct exposure of core resume would preserve pinned server identity.** — `diverges`. Core resume uses the accept-any-certificate configuration, accepts an unsigned WireResumeOk and merely caches opts.pinned_hash. The supplied pin is not verified on that path. This is a concrete safety objection to the proposed recipe, not a currently reachable Node-resume vulnerability because Node exposes no resume method. Reference: crates/shamir-client/src/client.rs:862; crates/shamir-client/src/client.rs:937; crates/shamir-client/src/wire_frames.rs:52; crates/shamir-transport-tcp/src/tls.rs:63.

## Reviewer's prior-cycle comparison

These are the independent reviewer's comparisons before parent refinements; the accepted ledgers above govern final decisions and counts.

- 1.4: partially-fixed -&gt; confirmed-open for the residual oracle gap; automated coverage is pre-existing counter-evidence to the no-tests premise, not a later fix.
- 1.5: confirmed-open retained, but the overhead-only/no-functional-failure assessment is corrected by a concrete valid 16-byte raw-ID false-positive witness.
- 3.2: unverified -&gt; refuted as the asserted mandatory async-panic repair; exact napi 3.10.5 already monitors unwinding future panics and rejects their promises. Synchronous resolver/finalizer and non-unwinding safety remain expressly uncertified.
- 1.6, 5.4 and 6.2: confirmed-open -&gt; not-applicable because the observed mechanisms are current intentional/unspecified API policies or harmless acknowledgement reconstruction, not proved contract violations. Corresponding P2.18, P2.14 and P1.9 recipes are optional.
- 1.2 remains open, calibrated medium: the documented DNS/unbracketed-IPv6 configurations fail, but numeric IPv4 and bracketed IPv6 remain usable.
- P2.12 remains open, with stronger negative safety evidence: the proposed core resume route caches rather than verifies the pin after accept-any-cert TLS. Node currently does not expose this route.
- Factory/refuted-repl/finalization/style conclusions were independently supported by actual source and exact archive inspection; no earlier accepted conclusion was used as proof.
- All assigned current rows and the complete historical bodies/recipes were inspected. No source fix or passing execution result is claimed.

## Current follow-up order

1. Establish immutable owned snapshots before async dispatch for externally supplied Buffers; catch_unwind cannot repair this ownership violation.
2. Preserve integer fidelity across the actual public wrapper, with independent unsafe-integer fixtures and BigInt input coverage.
3. Strengthen the error discriminator to validate complete marker shape and cover the valid 16-byte user-ID collision through the production wrapper.
4. Design request admission, bounded handshake/write/response lifetimes and coordinated close together; verify pending settlement without assuming timeout means rollback.
5. Fix documented DNS/unbracketed-IPv6 inputs while preserving bracketed IPv6 and identity-pin semantics.
6. Correct subset/builder documentation and typed declarations; do not expose current core resume until server identity verification is established.
7. Then address credential-copy hygiene, explicit response copying and documentation/version-policy nits. Verify packaging only against an actual distribution contract.

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
