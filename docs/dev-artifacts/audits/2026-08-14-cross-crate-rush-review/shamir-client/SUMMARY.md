<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-client — SUMMARY independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

The principal lifecycle, reference, authentication, dump and provenance defects remain. Several entries instead describe intentional APIs or prospective wire evolution. Decoder and cache guarantees require additional corrections beyond the first cycle.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 54 | 39 | 0 | 0 | 4 | 0 | 11 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1-1"></a>

### Claim 1.1 — Reader exit never closes subscription channels: every live subscriber hangs forever on any disconnect

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

After EOF the registry retains senders; each handle retains that registry. After queued items drain, next remains pending even after Client Drop. A regression must assert receive closure, not merely pending-map drain.

Evidence: [crates/shamir-client/src/client.rs:407](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L407); [crates/shamir-client/src/subscription.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/subscription.rs#L36); [crates/shamir-client/src/subscription.rs:54](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/subscription.rs#L54).

<a id="review-1-2"></a>

### Claim 1.2 — `batch_has_refs` misses two ref carriers: `QueryEntry.when` guards and `BatchOp::ForEach`

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Only entry.op is scanned, and ForEach falls through. A prior read becomes opaque IdBytes under smart-path Id encoding; guards or loop sources then resolve missing data. Existing when/ForEach e2e uses execute, not execute_with_touch.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:513](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L513); [crates/shamir-client/src/interner_cache_ops.rs:519](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L519); [crates/shamir-engine/src/query/batch/query_runner.rs:136](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/query_runner.rs#L136); [crates/shamir-engine/src/query/batch/query_runner.rs:758](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-engine/src/query/batch/query_runner.rs#L758); [crates/shamir-query-types/src/read/query_record.rs:195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/query_record.rs#L195).

<a id="review-1-3"></a>

### Claim 1.3 — `subscribe_push` / early-buffer routing race strands pushes (and reorders delivery)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Reader miss, registration plus empty flush, then reader append strands an envelope. Publishing the sender before flushing also permits a newer direct send before older buffered frames, violating the normative FIFO handoff.

Evidence: [crates/shamir-client/src/client.rs:330](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L330); [crates/shamir-client/src/client.rs:349](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L349); [crates/shamir-client/src/client.rs:998](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L998).

<a id="review-1-4"></a>

### Claim 1.4 — `dump_repo`: failed dump poisons the OnceCell as "populated", later calls never re-dump, and the `Result` lies

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Transport and parsing errors resolve unit, so Tokio 1.49.0 get_or_init commits population and dump_repo returns Ok. Later calls skip even a successful prior dump, contradicting unconditional re-dump documentation. The warm-cache test cannot distinguish a no-op.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:199](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L199); [crates/shamir-client/src/interner_cache_ops.rs:222](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L222); [crates/shamir-client/src/interner_cache.rs:175](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache.rs#L175); [crates/shamir-client/src/tests/interner_cache_tests.rs:194](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/interner_cache_tests.rs#L194).

<a id="review-1-5"></a>

### Claim 1.5 — Multi-repo v2 de-intern: first-match FieldMap can silently attach wrong field names

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Results are traversed without their aliases. If two repositories map the same ID to different names, both rows may resolve through the first candidate and one result is mislabeled. Public smart execution does not document this limitation.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:638](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L638); [crates/shamir-client/src/interner_cache_ops.rs:707](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L707); [crates/shamir-client/src/interner_cache_ops.rs:728](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L728).

<a id="review-1-6"></a>

### Claim 1.6 — Vacuous test: `atomic_u8_plumbing_stores_and_reads_correctly` tests std, not the client

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The registered test constructs standalone atomics. Changing Client's getter or handshake assignment cannot fail it; v2 tests also early-return when the getter reports an old version.

Evidence: [crates/shamir-client/src/tests/wire_version_tests.rs:135](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/wire_version_tests.rs#L135); [crates/shamir-client/src/tests/v2_passthrough_tests.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/v2_passthrough_tests.rs#L154).

<a id="review-1-7"></a>

### Claim 1.7 — `get_ddl_op_status`: comment/doc/behavior three-way contradiction, dead `DbResponse::Error` arm, zero coverage

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

roundtrip converts DbResponse::Error before this method matches it, making its special arm dead. Existing callers already receive structured Db.code. Unknown IDs and unsupported operations are not equivalent contracts.

Evidence: [crates/shamir-client/src/client.rs:1207](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1207); [crates/shamir-client/src/client.rs:1293](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1293).

<a id="review-1-8"></a>

### Claim 1.8 — Duplicate `subscribe_push(sub_id)`: dropping the first handle closes the second

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Registering twice replaces the sender; dropping the first handle unconditionally removes the replacement by sub_id. Identity-safe removal or duplicate rejection is needed.

Evidence: [crates/shamir-client/src/client.rs:1000](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1000); [crates/shamir-client/src/subscription.rs:59](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/subscription.rs#L59).

<a id="review-1-9"></a>

### Claim 1.9 — `collect_map_keys` skips `Value::Set` (asymmetric with `qv_has_fn_marker`)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

For a set containing maps, collection misses nested names but the id-keyed serializer visits them. A cold nested key fails encoding despite successful pre-touch of other keys.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:468](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L468); [crates/shamir-types/src/codecs/interned/messagepack.rs:965](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L965).

<a id="review-1-10"></a>

### Claim 1.10 — TDD-coverage gaps: untested public paths

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Registration inspection found no client-local resume e2e, public subscribe_push flush/closed test, reader-abort liveness assertion, or smart-path when/ForEach combination. External server resume coverage exists.

Evidence: [crates/shamir-client/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/mod.rs#L1); [crates/shamir-client/src/tests/resume_wire_tests.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/resume_wire_tests.rs#L1); [crates/shamir-client/tests/batch_when_e2e.rs:219](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/tests/batch_when_e2e.rs#L219); [crates/shamir-server/tests/duplex_e2e.rs:208](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/duplex_e2e.rs#L208).

<a id="review-1-11"></a>

### Claim 1.11 — `connect_timeout_fires` depends on 10.255.255.1:9 being a silent black hole

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Both tests assume the private address silently drops SYNs. Refusal or routing failure defeats their assertions. A connected but silent loopback listener would not test establishment timeout either.

Evidence: [crates/shamir-client/src/tests/timeout_tests.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/timeout_tests.rs#L26); [crates/shamir-client/src/tests/timeout_tests.rs:37](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/timeout_tests.rs#L37); [crates/shamir-client/src/tests/timeout_tests.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/timeout_tests.rs#L67).

<a id="review-1-12"></a>

### Claim 1.12 — `close()` never sets the `closed` flag though `ConnectionClosed`'s doc lists "explicit close()" as a trigger

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

The absence is real, but close consumes Client and the flag is private. No subsequent public request can observe it. Subscription teardown is a separate real defect; a future shared-handle bookkeeping preference is not a current contract failure.

Evidence: [crates/shamir-client/src/client.rs:1306](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1306); [crates/shamir-client/src/error.rs:41](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/error.rs#L41).

<a id="review-1-13"></a>

### Claim 1.13 — `next_request_id` u32 `fetch_add` overflow panics in debug builds / aliases silently in release

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Atomic addition wraps in both profiles. After a full cycle, an outstanding sender can be replaced or a foreign late response delivered. Replacement drops the old sender, yielding ConnectionClosed rather than an enduring wait.

Evidence: [crates/shamir-client/src/client.rs:1259](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1259); [crates/shamir-client/src/client.rs:1271](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1271); [crates/shamir-client/src/client.rs:290](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L290).

<a id="review-2-1"></a>

### Claim 2.1 — Reader-exit drain races pending registration → permanent hang of an in-flight request

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

A caller passes the initial check, reader sets closed and drains, then caller inserts. If its write succeeds, no reader remains to resolve its retained sender. A protocol-close frame or half-closed transport can leave writing available. None timeout makes the wait indefinite.

Evidence: [crates/shamir-client/src/client.rs:408](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L408); [crates/shamir-client/src/client.rs:1240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1240); [crates/shamir-client/src/client.rs:1271](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1271); [crates/shamir-client/src/client.rs:1290](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1290).

<a id="review-2-2"></a>

### Claim 2.2 — `std::sync::Mutex` on the demux hot paths (`PendingMap`, `SubscriptionMap`, `EarlyBuffer`) without a sanctioned-category justification

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Request and push paths take global standard mutexes. Comments justify no-await scopes, not sanctioned contention categories. Serialization is proven; runtime slowdown and its severity are not measured.

Evidence: [crates/shamir-client/src/client.rs:250](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L250); [crates/shamir-client/src/client.rs:1269](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1269); [crates/shamir-client/src/subscription.rs:28](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/subscription.rs#L28); [CLAUDE.md:269](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L269).

<a id="review-2-3"></a>

### Claim 2.3 — Concurrency-claim test gaps: stampede guard and drain race never exercised concurrently

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Dump testing is sequential and already warm; EOF testing registers all senders before reader launch. Neither counts concurrent cold dump requests nor drives production registration after terminal draining.

Evidence: [crates/shamir-client/src/tests/interner_cache_tests.rs:194](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/interner_cache_tests.rs#L194); [crates/shamir-client/src/tests/demux_tests.rs:175](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/demux_tests.rs#L175).

<a id="review-2-4"></a>

### Claim 2.4 — `CursorStream::cursor_id` cell could be a `OnceLock` instead of `StdMutex`

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Write-once representation is an optional implementation choice. No current observable malfunction follows from this mutex. Its inaccurate lock-skip claim is separately confirmed as NEW.2.

Evidence: [crates/shamir-client/src/cursor_stream.rs:221](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/cursor_stream.rs#L221); [crates/shamir-client/src/cursor_stream.rs:250](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/cursor_stream.rs#L250).

<a id="review-3-1"></a>

### Claim 3.1 — `Client::resume` hands the bearer ticket to an unverified peer; `pinned_hash` is dead bookkeeping on the resume path

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

An active endpoint substitute can receive a valid bearer ticket through accepted TLS and use it first against the real endpoint. Pin bookkeeping and fresh-exporter strength checks do not prevent disclosure. Checking resume_ok afterward is too late.

Evidence: [crates/shamir-client/src/client.rs:866](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L866); [crates/shamir-client/src/client.rs:895](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L895); [crates/shamir-client/src/client.rs:937](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L937); [crates/shamir-connect/src/server/resume.rs:310](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/resume.rs#L310); [crates/shamir-connect/src/server/resume.rs:324](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/resume.rs#L324).

<a id="review-3-2"></a>

### Claim 3.2 — SDK silently discards `rotation_in_progress` and `kdf_upgrade_required` from `auth_ok` — orphan-recovery (spec §6.5) and KDF-upgrade hints (§13) are unreachable

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Client and server transport mirrors omit both signals. Supplying fields alone would not recover changed pins because process_auth_ok rejects first. Recovery must preserve strict signatures, time bounds and explicit approval; the KDF signal is informational.

Evidence: [crates/shamir-client/src/wire_frames.rs:66](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/wire_frames.rs#L66); [crates/shamir-client/src/client.rs:603](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L603); [crates/shamir-server/src/connection/handshake.rs:632](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L632); [crates/shamir-connect/src/client/handshake.rs:266](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L266).

<a id="review-3-3"></a>

### Claim 3.3 — Cleartext password persists in unzeroized serialization buffers after `create_scram_user`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

The default crypto feature wipes SecretString, not retained REQ_BUF, its clone, envelope bytes or framing bytes. Exposure requires memory access/disclosure, not merely a remote query. clear is not zeroization.

Evidence: [crates/shamir-client/src/client.rs:1104](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1104); [crates/shamir-client/src/client.rs:1252](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1252); [crates/shamir-transport-tcp/src/framing.rs:160](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L160); [crates/shamir-query-types/Cargo.toml:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/Cargo.toml#L36).

<a id="review-3-4"></a>

### Claim 3.4 — No depth guard on nested msgpack decode of peer frames

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Exact rmp-serde 1.3.1 guarded array/map dispatch refutes unlimited serde recursion. Opaque IdBytes use separate RecordView guards, not that counter. Guard existence does not establish stack safety or explicit malformed-record rejection.

Evidence: [Cargo.lock:2949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2949); [crates/shamir-types/src/types/value.rs:283](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/types/value.rs#L283); [crates/shamir-types/src/record_view/lens.rs:38](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/lens.rs#L38).

<a id="review-3-5"></a>

### Claim 3.5 — No negative-path coverage for pin verification or resume anywhere in the crate's suite

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Client fixtures use unpinned TOFU; resume tests are serde-only. Server-library replay/rejection/rotation tests exist, but they cannot catch SDK omission of pin verification.

Evidence: [crates/shamir-client/src/tests/interner_cache_tests.rs:98](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/interner_cache_tests.rs#L98); [crates/shamir-client/src/tests/resume_wire_tests.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/resume_wire_tests.rs#L1); [crates/shamir-connect/tests/integration_resume.rs:437](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/tests/integration_resume.rs#L437); [crates/shamir-connect/tests/integration_resume.rs:984](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/tests/integration_resume.rs#L984).

<a id="review-4-1"></a>

### Claim 4.1 — `roundtrip` clones the whole serialized request per call and ignores the zero-copy envelope built for exactly this path

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Scratch cloning, owning sid construction, envelope serialization and framing staging remain. Borrowed envelopes remove staging ownership, not output serialization. No measured evidence supports a medium or high performance impact.

Evidence: [crates/shamir-client/src/client.rs:1256](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1256); [crates/shamir-connect/src/common/envelope.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/envelope.rs#L43); [crates/shamir-connect/src/common/envelope.rs:92](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/envelope.rs#L92); [crates/shamir-transport-tcp/src/framing.rs:160](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L160).

<a id="review-4-2"></a>

### Claim 4.2 — v2 read path: per-row `ByteBuf` clone and per-row×repo `get_or_create` key allocations in de-intern

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Each IdBytes row is cloned, and candidate probes construct owned db/repo strings. These are avoidable staging costs, but impact is unmeasured. Correct provenance must precede hoisting the same ambiguous candidate list.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:654](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L654); [crates/shamir-client/src/interner_cache_ops.rs:729](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L729); [crates/shamir-client/src/interner_cache.rs:207](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache.rs#L207).

<a id="review-4-3"></a>

### Claim 4.3 — `early_buffer` key cardinality is server-controlled and unbounded for the life of the connection

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Each unknown peer-supplied sub ID creates retained state. A per-key envelope cap does not cap keys or aggregate bytes. Ordinary use can also accumulate abandoned grants; arbitrary-ID flooding requires a faulty or hostile peer.

Evidence: [crates/shamir-client/src/client.rs:350](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L350); [crates/shamir-client/src/client.rs:407](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L407); [crates/shamir-client/src/subscription.rs:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/subscription.rs#L30).

<a id="review-4-4"></a>

### Claim 4.4 — Orphaned `pending` entries when the caller's future is cancelled (no drop guard)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Dropping the future after registration bypasses explicit write-error and local response-timeout cleanup. Against unanswered requests, entries persist for the connection lifetime. Removing them alone does not solve partially cancelled writes.

Evidence: [crates/shamir-client/src/client.rs:1267](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1267); [crates/shamir-client/src/client.rs:1277](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1277); [crates/shamir-client/src/client.rs:1290](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1290).

<a id="review-4-5"></a>

### Claim 4.5 — Ambient interner sync runs on every `execute` regardless of server version or cache state

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Traversal and cache-key construction occur without a version gate, but cold-v2 sync is functional and pre-touch is the advertised method contract. This is an unmeasured optimization opportunity, not universally wasted or unbounded work.

Evidence: [crates/shamir-client/src/client.rs:1053](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1053); [crates/shamir-client/src/interner_cache_ops.rs:357](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L357); [crates/shamir-client/src/tests/ambient_sync_tests.rs:143](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/ambient_sync_tests.rs#L143).

<a id="review-4-6"></a>

### Claim 4.6 — `encode_record_idmsgpack` pays an avoidable full-record copy per INSERT (`Bytes::to_vec`)

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The unique owned encoding is copied with to_vec. Exact bytes 1.11.1 supplies consuming From&lt;Bytes&gt; for Vec, not a public Bytes::into_vec method. A consuming conversion can preserve wire bytes while removing staging.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:612](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L612); [crates/shamir-types/src/codecs/interned/messagepack.rs:873](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L873); [Cargo.lock:571](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L571).

<a id="review-4-7"></a>

### Claim 4.7 — `collect_field_names` clones every field-name key of every record before dedup

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Every visited map-key occurrence is cloned, sorted and deduplicated, including cached names. Borrowed deduplicated collection can avoid these allocations without changing ordering-independent registration semantics.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:471](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L471); [crates/shamir-client/src/interner_cache_ops.rs:377](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L377).

<a id="review-4-8"></a>

### Claim 4.8 — Perf claims are unmeasured: no benches, no allocation-behaviour tests

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

No client benchmark or allocation-count oracle was found. Functional tests do not establish claimed latency, allocation totals or global growth bounds.

Evidence: [crates/shamir-client/Cargo.toml:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/Cargo.toml#L43); [crates/shamir-client/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/mod.rs#L1).

<a id="review-5-1"></a>

### Claim 5.1 — `query_version: 2` stamped unconditionally — the `server_query_version` negotiation is parsed but never applied to the request version

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Execute and cursor creation stamp 2 regardless of the advertised version. History before f37f6579 supported only 1. Current peers support both; the defect is the documented older-server fallback, not rejection by today's server.

Evidence: [crates/shamir-client/src/client.rs:1061](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1061); [crates/shamir-query-builder/src/cursor.rs:43](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/cursor.rs#L43); [crates/shamir-server/src/version.rs:45](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/version.rs#L45).

<a id="review-5-2"></a>

### Claim 5.2 — Handshake/resume frames are positional (array) msgpack while everything post-handshake is named-map — order is load-bearing, enforced only by duplicated struct definitions

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Exact current client/server layouts match, and positional encoding is deliberately documented. A hypothetical middle-field insertion is a prospective compatibility risk, not a current defect. Changing encoding/versioning requires coordinated compatibility work.

Evidence: [crates/shamir-client/src/wire_frames.rs:13](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/wire_frames.rs#L13); [crates/shamir-server/src/connection/wire.rs:35](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/wire.rs#L35); [crates/shamir-client/src/client.rs:504](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L504).

<a id="review-5-3"></a>

### Claim 5.3 — `ResumeOptions` has no `connect_timeout`/`request_timeout` — resumed clients silently revert to unbounded waits

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

This is explicitly documented unbounded behavior; commit 01f5883d identifies intentional scope. Resume does not receive an existing Client whose deadlines could be lost. Adding knobs is useful capability parity, not a proven broken guarantee.

Evidence: [crates/shamir-client/src/client.rs:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L99); [crates/shamir-client/src/client.rs:870](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L870); [crates/shamir-client/src/client.rs:949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L949).

<a id="review-5-4"></a>

### Claim 5.4 — Envelope-level server errors are stringly-typed into `ClientError::Protocol` — spec §14 codes are not machine-readable

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Session codes become formatted Protocol strings, unlike Db errors. Callers must parse display text; the existing demux test explicitly asserts a substring rather than a structured code.

Evidence: [crates/shamir-client/src/client.rs:374](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L374); [crates/shamir-client/src/tests/demux_tests.rs:235](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/demux_tests.rs#L235).

<a id="review-5-5"></a>

### Claim 5.5 — Dead public API: `ClientError::RequestIdMismatch` is never constructed anywhere in the workspace

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Only the declaration remains. Unknown IDs are dropped rather than reported as mismatches. This is misleading public taxonomy, not a demonstrated routing failure.

Evidence: [crates/shamir-client/src/error.rs:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/error.rs#L29); [crates/shamir-client/src/client.rs:400](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L400).

<a id="review-5-6"></a>

### Claim 5.6 — Frame demux is shape-sniffing with no discriminator — fragile routing plus up to three decode attempts per frame

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Current response/error/push shapes are distinguishable and no current overlapping emitted envelope was found. A future response-like progress frame is a design risk; additional parse attempts are a separate measured-neither optimization fact.

Evidence: [crates/shamir-client/src/client.rs:194](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L194); [crates/shamir-client/src/client.rs:329](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L329); [crates/shamir-connect/src/common/push_envelope.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/push_envelope.rs#L25).

<a id="review-5-7"></a>

### Claim 5.7 — Ambient interner-epoch advertisement is all-or-nothing per batch

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

One caller-supplied epoch disables automatic insertion for every other referenced repository, contrary to advertising each targeted repo transparently. Per-key insertion preserves explicit values.

Evidence: [crates/shamir-client/src/client.rs:1038](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1038); [crates/shamir-client/src/client.rs:1053](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1053).

<a id="review-5-8"></a>

### Claim 5.8 — `ConnectOptions` has no `Default` impl — every call site must spell out 8 fields

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `confirmed-open`.

Endpoint and credentials intentionally require explicit values; history explains why Default was absent. An options constructor may improve ergonomics, but permissive TOFU or dummy credential defaults are not warranted fixes.

Evidence: [crates/shamir-client/src/client.rs:58](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L58); [crates/shamir-client-node/src/lib.rs:123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client-node/src/lib.rs#L123).

<a id="review-6-1"></a>

### Claim 6.1 — Stringly-typed `Handshake`/`Tls`/`Transport`/`Protocol` variants erase typed sources

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Handshake, framing and rustls errors are converted to strings, losing discriminants and source chains. Io/Encode/Decode are already typed. API improvement is real, but medium runtime severity is unsupported.

Evidence: [crates/shamir-client/src/error.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/error.rs#L9); [crates/shamir-client/src/error.rs:58](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/error.rs#L58); [crates/shamir-client/src/client.rs:626](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L626).

<a id="review-6-2"></a>

### Claim 6.2 — Push-subscription lifecycle and closed-client paths have no error-path tests

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

EOF tests prove pending drain only. No registered test asserts subscription receive closure, dead-client registration or the public roundtrip closed guard. These require production seams, not manually mutating maps.

Evidence: [crates/shamir-client/src/tests/demux_tests.rs:175](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/demux_tests.rs#L175); [crates/shamir-client/src/tests/demux_tests.rs:276](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/demux_tests.rs#L276); [crates/shamir-client/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/mod.rs#L1).

<a id="review-6-3"></a>

### Claim 6.3 — `.expect()` in library code rests on an unenforced cross-crate invariant

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Pinned captures start Some. Unpinned successful authentication necessarily invokes the callback, and disallowed TOFU is rejected by build. Future dependency drift is not a present user-triggerable panic.

Evidence: [crates/shamir-client/src/client.rs:610](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L610); [crates/shamir-client/src/client.rs:817](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L817); [crates/shamir-connect/src/client/handshake.rs:149](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L149); [crates/shamir-connect/src/client/handshake.rs:274](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L274).

<a id="review-6-4"></a>

### Claim 6.4 — `touch_fields` silently omits names the server failed to map

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

After response merging, filter_map omits unmapped requested names and still returns Ok. The ordinary server maps every requested name; this is incomplete-peer-response validation, not an ordinary missing-map server witness.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:280](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L280); [crates/shamir-db/src/shamir_db/execute/admin_interner.rs:154](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_interner.rs#L154).

<a id="review-6-5"></a>

### Claim 6.5 — Undecodable response frames are dropped at `debug` level; default-unbounded waiters hang

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

If a malformed frame replaces the only response and the connection stays live, log-and-continue never resolves its waiter without a configured response timeout. Raising log level alone does not repair liveness.

Evidence: [crates/shamir-client/src/client.rs:360](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L360); [crates/shamir-client/src/client.rs:274](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L274).

<a id="review-6-6"></a>

### Claim 6.6 — `ResumeOptions::ticket` and its wire copy are not `Zeroizing`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The credential moves into an ordinary wire Vec and also has ordinary serialization/framing copies. Only the retained Client ticket is Zeroizing. Memory-disclosure preconditions apply; wrapping only ResumeOptions is incomplete.

Evidence: [crates/shamir-client/src/client.rs:105](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L105); [crates/shamir-client/src/client.rs:891](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L891); [crates/shamir-client/src/client.rs:939](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L939).

<a id="review-7-1"></a>

### Claim 7.1 — Crate-level doc example no longer compiles against `ConnectOptions`; drift is invisible because doctests are disabled

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The six-field literal omits two required struct fields; doctests are disabled. The missing-field mismatch is statically visible without compiling.

Evidence: [crates/shamir-client/src/lib.rs:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/lib.rs#L10); [crates/shamir-client/src/client.rs:85](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L85); [crates/shamir-client/Cargo.toml:52](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/Cargo.toml#L52).

<a id="review-7-2"></a>

### Claim 7.2 — `use rand::RngCore;` inside the body of `Client::resume`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The ordinary block-local import has no documented collision/cfg exception. This is repository style drift only.

Evidence: [crates/shamir-client/src/client.rs:883](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L883).

<a id="review-7-3"></a>

### Claim 7.3 — Mid-body `use` statements in three `src/tests/` files

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

All three cited imports remain and can be hoisted without a documented exception. No behavioral defect follows.

Evidence: [crates/shamir-client/src/tests/batch_has_refs_tests.rs:18](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/batch_has_refs_tests.rs#L18); [crates/shamir-client/src/tests/demux_tests.rs:408](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/demux_tests.rs#L408); [crates/shamir-client/src/tests/wire_version_tests.rs:137](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/wire_version_tests.rs#L137).

<a id="review-nf-1"></a>

### Claim NF.1 — `connect` fail-closed

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Positive counter-evidence supports builder rejection and full-handshake proof/pin/signature checks. It does not apply to resume.

Evidence: [crates/shamir-connect/src/client/handshake.rs:149](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L149); [crates/shamir-connect/src/client/handshake.rs:258](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L258).

<a id="review-nf-2"></a>

### Claim NF.2 — KDF-DoS: server-supplied Argon2id params are double-capped before allocation

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Both checks precede derivation; effective protocol limits are 256 MiB/eight passes. Per-operation caps are not an aggregate memory-safety guarantee.

Evidence: [crates/shamir-connect/src/client/handshake.rs:209](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L209); [crates/shamir-connect/src/common/types.rs:123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/types.rs#L123); [crates/shamir-connect/src/common/kdf_params.rs:77](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/kdf_params.rs#L77).

<a id="review-nf-3"></a>

### Claim NF.3 — Comparison hygiene and CSPRNG-backed nonce generation

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Checks use constant-time equality and strict Ed25519 verification. Exact rand 0.9.4 thread RNG uses OS-seeded ChaCha12; its documented fork-reseed qualification prevents a blanket all-process-model assurance.

Evidence: [crates/shamir-connect/src/common/crypto.rs:131](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/crypto.rs#L131); [crates/shamir-connect/src/common/crypto.rs:249](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/crypto.rs#L249); [crates/shamir-client/src/client.rs:884](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L884); [Cargo.lock:2692](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2692).

<a id="review-nf-4"></a>

### Claim NF.4 — Untrusted-input size bounds, fixed-size validation, and no client unsafe

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

16 MiB read caps and fallible fixed-size conversions exist, and client sources contain no unsafe. This does not validate the inherited unsafe reader, the 4 KiB pre-auth cap or IdBytes-derived allocation bounds.

Evidence: [crates/shamir-transport-tcp/src/framing.rs:110](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L110); [crates/shamir-client/src/client.rs:513](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L513); [crates/shamir-client/src/client.rs:905](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L905).

<a id="review-nf-5"></a>

### Claim NF.5 — Builder-only query construction and names never parsed as numeric IDs

Status: `not-applicable`. Current risk: `—`.

Prior-cycle decision: `not-applicable`.

Query construction uses typed builders; numeric-looking names are string lookups. The server-only provenance assurance applies to normal SDK merging, not the public insert_entry API.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:268](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L268); [crates/shamir-client/src/interner_cache.rs:115](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache.rs#L115).

<a id="review-nf-6"></a>

### Claim NF.6 — Structure and module-doc coverage claims

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `unverified`.

Structural registration conforms, but the broad coverage assertion is positively contradicted: AtomicU8 bypasses Client, dump/refresh tests start already warm, and cold-v2 refresh is preempted by ambient deltas. Module presence cannot establish those advertised seams.

Evidence: [crates/shamir-client/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/mod.rs#L1); [crates/shamir-client/src/tests/wire_version_tests.rs:135](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/wire_version_tests.rs#L135); [crates/shamir-client/src/tests/interner_cache_tests.rs:194](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/interner_cache_tests.rs#L194); [crates/shamir-client/src/tests/v2_passthrough_tests.rs:405](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/v2_passthrough_tests.rs#L405).

<a id="review-nf-7"></a>

### Claim NF.7 — `interner_cache.rs` is fully pillar-compliant and lock-free; no unsanctioned lock crosses await

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Exact scc 3.8.4 uses bucket locks for read_sync/insert_sync. THasher, monotonic CAS and short standard-mutex scopes are separate supported properties, not proof of lock-free access.

Evidence: [crates/shamir-client/src/interner_cache.rs:39](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache.rs#L39); [crates/shamir-client/src/interner_cache.rs:115](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache.rs#L115); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123).

## Revalidated plan decisions

| Plan decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 24 | 22 | 0 | 0 | 0 | 0 | 2 |

Historical P0/P1/P2 numbering is an identifier, not a current release mandate. The reasons below include completion status, safety qualifications and discriminating acceptance requirements.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No teardown/closed-registration fix exists. Coordinate registration with closure and clean both natural reader exit and explicit close/Drop; abort alone skips the reader tail.

Evidence: [crates/shamir-client/src/client.rs:407](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L407); [crates/shamir-client/src/client.rs:996](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L996); [crates/shamir-client/src/client.rs:1308](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1308).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Registration still lacks terminal synchronization. Same-lock store/drain and insertion/recheck are sound; the regression must drive roundtrip's real registration seam, not directly insert into an already-drained raw map.

Evidence: [crates/shamir-client/src/client.rs:408](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L408); [crates/shamir-client/src/client.rs:1271](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1271); [crates/shamir-client/src/tests/demux_tests.rs:175](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/demux_tests.rs#L175).

<a id="plan-p0-3"></a>

### Plan P0.3 — P0.3

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Authentication remains absent. A signed resume_ok after ticket transmission or documentation-only metadata relabeling does not repair theft. Authenticate the endpoint before disclosure; old/new exporter equality is an incompatible recipe.

Evidence: [crates/shamir-client/src/client.rs:895](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L895); [crates/shamir-connect/src/server/resume.rs:324](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/resume.rs#L324).

<a id="plan-p0-4"></a>

### Plan P0.4 — P0.4

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Version stamps remain constant. Select a supported intersection and use existing create_cursor_with_version. Downgrading the numeric stamp must also reject or translate unsupported v2-only request content.

Evidence: [crates/shamir-client/src/client.rs:1061](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1061); [crates/shamir-query-builder/src/cursor.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-builder/src/cursor.rs#L47); [crates/shamir-server/src/version.rs:45](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/version.rs#L45).

<a id="plan-p0-5"></a>

### Plan P0.5 — P0.5

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No when/ForEach/ValueCompare fix exists. Include both ValueCompare operands and nested conditions. Ref-bearing caller-supplied Id encoding must not remain silently unsafe; use name encoding or explicit rejection consistently.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:513](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L513); [crates/shamir-client/src/interner_cache_ops.rs:415](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L415); [crates/shamir-client/src/interner_cache_ops.rs:535](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L535).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Policy drift remains. scc synchronous methods are not lock-free. An independently justified primitive/comment decision must preserve compound close/registration and FIFO invariants; replacing two maps independently does not do so.

Evidence: [crates/shamir-client/src/client.rs:250](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L250); [crates/shamir-client/src/subscription.rs:28](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/subscription.rs#L28); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Errors still initialize unit. Tokio 1.49.0 get_or_try_init preserves retryability on Err, but concurrent waiters may start another failed attempt rather than share one error. Define post-success re-dump semantics separately; permanently storing Err does not preserve retry.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:199](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L199); [crates/shamir-client/src/interner_cache.rs:175](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache.rs#L175); [Cargo.lock:4195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4195).

<a id="plan-p1-8"></a>

### Plan P1.8 — P1.8

Status: `not-applicable`.

Prior-cycle decision: `confirmed-open`.

Deadline parity is an optional enhancement to explicitly scoped resume behavior. If chosen, adding public fields is source-breaking and must accurately distinguish TCP/response budgets from whole-handshake/write budgets.

Evidence: [crates/shamir-client/src/client.rs:99](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L99); [crates/shamir-client/src/client.rs:870](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L870); [crates/shamir-client/src/client.rs:949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L949).

<a id="plan-p1-9"></a>

### Plan P1.9 — P1.9

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

No RAII guard exists. Identity-safe pending removal addresses retention; it must be combined with frame-write cancellation safety so a cancelled partially written frame cannot corrupt later requests.

Evidence: [crates/shamir-client/src/client.rs:1267](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1267); [crates/shamir-client/src/client.rs:1277](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1277); [crates/shamir-transport-tcp/src/framing.rs:163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L163).

<a id="plan-p1-10"></a>

### Plan P1.10 — P1.10

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Total admission and teardown clearing remain absent. Prefer explicit key/envelope/byte budgets and observable overflow; arbitrarily evicting old buffered frames would contradict the current drop-new policy and can lose terminal markers.

Evidence: [crates/shamir-client/src/client.rs:350](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L350); [crates/shamir-client/src/subscription.rs:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/subscription.rs#L30).

<a id="plan-p1-11"></a>

### Plan P1.11 — P1.11

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Typed sources and session codes remain absent. The proposed shamir_connect::client::Error path is wrong; the actual error type is common::error::Error. Preserve distinct Db, session and locally generated protocol errors.

Evidence: [crates/shamir-connect/src/client/handshake.rs:22](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L22); [crates/shamir-client/src/error.rs:9](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/error.rs#L9); [crates/shamir-client/src/client.rs:374](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L374).

<a id="plan-p1-12"></a>

### Plan P1.12 — P1.12

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Staging and unwiped copies remain. Borrowed-envelope encoding preserves wire shape if the scratch borrow ends before await. Include framing buffers and error/cancellation exits; zeroize wrappers do not retroactively erase prior reallocated buffers or prove one allocation.

Evidence: [crates/shamir-client/src/client.rs:1252](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1252); [crates/shamir-connect/src/common/envelope.rs:92](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/common/envelope.rs#L92); [crates/shamir-transport-tcp/src/framing.rs:160](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L160).

<a id="plan-p1-13"></a>

### Plan P1.13 — P1.13

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Per-row costs and wrong-map selection remain. Preserve result provenance, including nested results, rather than probing ambiguously when provenance is unknown. Keep useful cold-v2 ambient synchronization and repair gap-free epochs first.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:638](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L638); [crates/shamir-client/src/interner_cache_ops.rs:654](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L654); [crates/shamir-client/src/client.rs:1053](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1053).

<a id="plan-p1-14"></a>

### Plan P1.14 — P1.14

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Required seam-specific tests are absent. Assert negotiated version positively, count actual concurrent dump requests, and test authentication before ticket emission. No passing-run claim follows from existing registration.

Evidence: [crates/shamir-client/src/tests/wire_version_tests.rs:135](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/wire_version_tests.rs#L135); [crates/shamir-client/src/tests/interner_cache_tests.rs:194](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/interner_cache_tests.rs#L194); [crates/shamir-client/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/mod.rs#L1).

<a id="plan-p2-15"></a>

### Plan P2.15 — P2.15

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Handoff remains split. Synchronize both miss-to-buffer and registration-through-FIFO-flush with consistent lock order or one per-sub state machine. Moving only one side's flush is insufficient.

Evidence: [crates/shamir-client/src/client.rs:330](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L330); [crates/shamir-client/src/client.rs:349](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L349); [crates/shamir-client/src/client.rs:998](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L998).

<a id="plan-p2-16"></a>

### Plan P2.16 — P2.16

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Dead DDL arm remains. Existing Db.code already distinguishes unsupported. Remove dead reshaping or explicitly handle Err(Db); do not conflate unsupported protocol with an unknown operation ID without a contract change.

Evidence: [crates/shamir-client/src/client.rs:1207](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1207); [crates/shamir-client/src/client.rs:1293](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1293).

<a id="plan-p2-17"></a>

### Plan P2.17 — P2.17

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Duplicate replacement remains unsafe. Token/channel-identity removal or explicit rejection is sound; reusing a sender does not itself permit two independently owned mpsc receivers.

Evidence: [crates/shamir-client/src/client.rs:1000](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1000); [crates/shamir-client/src/subscription.rs:59](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/subscription.rs#L59).

<a id="plan-p2-18"></a>

### Plan P2.18 — P2.18

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Set recursion is still missing. Test a genuinely cold nested set-of-maps through execute_with_touch and verify stored/read fields, not only a collector copy.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:468](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L468); [crates/shamir-types/src/codecs/interned/messagepack.rs:965](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L965).

<a id="plan-p2-19"></a>

### Plan P2.19 — P2.19

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Network-dependent tests, ID collision policy and incomplete automatic epochs remain. A silent accepted listener cannot test connect establishment. Merely setting closed at ID wrap neither drains waiters nor prevents an already admitted frame write; close-flag bookkeeping itself is optional.

Evidence: [crates/shamir-client/src/tests/timeout_tests.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/timeout_tests.rs#L26); [crates/shamir-client/src/client.rs:1259](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1259); [crates/shamir-client/src/client.rs:1053](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1053).

<a id="plan-p2-20"></a>

### Plan P2.20 — P2.20

Status: `not-applicable`.

Prior-cycle decision: `not-applicable`.

Unlimited serde recursion is refuted by the exact dependency. A lower depth/stack assessment is optional hardening. spawn_blocking alone does not establish a larger stack, and neither approach fixes IdBytes cardinality or silent iterator truncation.

Evidence: [Cargo.lock:2949](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L2949); [crates/shamir-types/src/record_view/lens.rs:38](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/lens.rs#L38); [crates/shamir-types/src/codecs/interned/codec.rs:227](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/codec.rs#L227).

<a id="plan-p2-21"></a>

### Plan P2.21 — P2.21

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Malformed-frame liveness remains; positional evolution and future overlap are optional hardening. Preserve map-order independence: a first-key-only classifier is unsafe. Tags or appended resume fields require actual peer compatibility checks.

Evidence: [crates/shamir-client/src/client.rs:194](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L194); [crates/shamir-client/src/client.rs:360](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L360); [crates/shamir-server/src/connection/wire.rs:65](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/wire.rs#L65).

<a id="plan-p2-22"></a>

### Plan P2.22 — P2.22

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Copies and eager cloning remain unoptimized, with no measurement. bytes 1.11.1 consuming Vec conversion is valid; _into currently consumes scratch capacity and is not a persistent reuse guarantee. Benchmarks alone are not growth-invariant regression tests.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:612](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L612); [crates/shamir-types/src/codecs/interned/messagepack.rs:907](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/messagepack.rs#L907); [Cargo.lock:571](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L571).

<a id="plan-p2-23"></a>

### Plan P2.23 — P2.23

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Incomplete-touch validation, misleading error surface and ticket-copy hygiene remain. Default and replacing the justified expect are preferences. Changing ticket type alone leaves serialization copies; removing a public error variant affects exhaustive matches.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:280](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L280); [crates/shamir-client/src/error.rs:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/error.rs#L29); [crates/shamir-client/src/client.rs:891](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L891).

<a id="plan-p2-24"></a>

### Plan P2.24 — P2.24

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Example/import drift and flush/liveness coverage gaps remain. An import/example cleanup cannot itself close behavioral test obligations; keep those as explicit acceptance criteria.

Evidence: [crates/shamir-client/src/lib.rs:10](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/lib.rs#L10); [crates/shamir-client/src/client.rs:883](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L883); [crates/shamir-client/src/tests/mod.rs:1](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/tests/mod.rs#L1).

## Additional observations

| Observation decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 6 | 6 | 0 | 0 | 0 | 0 | 0 |

Existing observation IDs remain stable. New cycle-2 rows follow them; cross-module duplicates and extra triggers are grouped explicitly. None is an implemented fix.

<a id="new-1"></a>

### Observation NEW.1 — The reference-walk fix also misses Filter::ValueCompare operands

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

ValueCompare falls through filter_has_refs, including when guards and Cond conditions reached recursively. A positive literal-versus-query guard using an earlier read is a discriminating smart-path oracle; adding only entry.when traversal leaves this defect.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:535](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L535); [crates/shamir-client/src/interner_cache_ops.rs:562](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L562); [crates/shamir-client/src/interner_cache_ops.rs:585](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L585); [crates/shamir-query-types/src/filter/filter_enum.rs:146](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/filter/filter_enum.rs#L146).

Grouping/duplicate: [SUMMARY.md#1.2](SUMMARY.md#review-1-2). This is not an additional independent defect.

<a id="new-2"></a>

### Observation NEW.2 — The claimed cursor mutex-skip optimization still locks on every yielded record

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The successful Buffered result locks first, then tests is_none. Only repeated assignment is avoided; every yielded successful record still takes the mutex. No quantified performance impact is established.

Evidence: [crates/shamir-client/src/cursor_stream.rs:237](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/cursor_stream.rs#L237); [crates/shamir-client/src/cursor_stream.rs:250](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/cursor_stream.rs#L250).

<a id="observation-new-3"></a>

### Observation NEW.3 — Sparse touch responses advance the client's completeness epoch past missing IDs

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

With a fresh Client and an existing repository dictionary containing IDs 1..3, touch_fields of only the name at ID 3 merges that one mapping and advances epoch to at least 3. InternerTouch is an admin op, so distinct_repos does not request an ambient delta. refresh_repo and subsequent automatic sync ask for IDs greater than 3, never recovering missing IDs 1 and 2; an id-keyed read using them can fail despite refresh. No hostile peer or concurrency is required. The discriminating oracle is a fresh client touching one high existing ID, then refreshing and reading a row containing a lower untouched ID. Retain a completeness watermark separate from sparse observed mappings.

Evidence: [crates/shamir-client/src/interner_cache.rs:96](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache.rs#L96); [crates/shamir-client/src/interner_cache_ops.rs:227](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L227); [crates/shamir-client/src/interner_cache_ops.rs:275](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L275); [crates/shamir-query-types/src/batch/batch_op.rs:561](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/batch/batch_op.rs#L561); [crates/shamir-db/src/shamir_db/execute/admin_interner.rs:179](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/admin_interner.rs#L179); [crates/shamir-db/src/shamir_db/execute/ambient_interner.rs:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/execute/ambient_interner.rs#L48).

<a id="observation-new-4"></a>

### Observation NEW.4 — Cancelling a partially written roundtrip leaves a live connection with corrupted frame alignment

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

roundtrip holds the write mutex while write_frame awaits Tokio write_all. Exact Tokio 1.49.0 write_all may accept a prefix, advance its private buffer slice, then return Pending. External cancellation drops that progress and releases the mutex without closing or marking the Client unusable. The next request starts a new length prefix inside the unfinished previous frame, corrupting server framing. This is reachable with ordinary socket backpressure and an outer timeout/task cancellation; request_timeout does not cover writing. A pending-map RAII guard alone cannot fix it. The oracle must cancel after a controlled real framing prefix write, then verify either completion of that frame or explicit connection failure before another send. Source: https://docs.rs/crate/tokio/1.49.0/source/src/io/util/write_all.rs

Evidence: [crates/shamir-client/src/client.rs:1277](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1277); [crates/shamir-client/src/client.rs:1278](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L1278); [crates/shamir-transport-tcp/src/framing.rs:160](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L160); [crates/shamir-transport-tcp/src/framing.rs:163](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/framing.rs#L163); [Cargo.lock:4195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4195).

<a id="observation-new-5"></a>

### Observation NEW.5 — Opaque IdBytes map counts trigger uncapped allocation before body validation

Status: `confirmed-open`. Current risk: `high`.

Additional observation in this independent cycle; it may overlap an existing root.

A v2 smart-path response can contain an opaque binary row consisting of a Map32 header with an attacker-chosen large count and no entries. Outer rmp-serde accepts the binary payload without interpreting its count. RecordView::new validates only the header; record_view_deintern_with immediately passes view.len to new_map_wc. Exact indexmap 2.14.0 with_capacity_and_hasher calls Core::with_capacity, reserving its hash index and Vec for that count before any body traversal. Thus a tiny in-cap frame can request extremely large allocation or encounter allocation/capacity failure outside ClientError. This requires a faulty/hostile authenticated or resume-substituted peer and a consumer using execute_with_touch, not ordinary Node execute or replication pull. No allocation/crash threshold was measured. Reject implausible cardinality against available bytes and an explicit budget before allocation. Source: https://docs.rs/crate/indexmap/2.14.0/source/src/inner.rs

Evidence: [crates/shamir-query-types/src/read/query_record.rs:88](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/read/query_record.rs#L88); [crates/shamir-client/src/interner_cache_ops.rs:724](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L724); [crates/shamir-types/src/record_view/lens.rs:158](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/lens.rs#L158); [crates/shamir-types/src/record_view/lens.rs:767](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/lens.rs#L767); [crates/shamir-types/src/codecs/interned/codec.rs:227](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/codec.rs#L227); [crates/shamir-collections/src/lib.rs:29](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-collections/src/lib.rs#L29); [Cargo.lock:1782](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1782).

<a id="observation-new-6"></a>

### Observation NEW.6 — Malformed id-keyed rows can be returned successfully as empty or partial records

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

For a small header-declared map with a missing/truncated entry, RecordView::new succeeds. fields converts malformed keys/values, including depth errors, into iterator termination. record_view_deintern_with then returns Ok of the map collected so far, and try_deintern_repos treats it as successful resolution. A one-entry map header with no body therefore becomes an empty Direct record; a valid first entry followed by truncation becomes a partial record. This is distinct from unknown-ID refresh and wrong-repository selection. It requires malformed IdBytes from a faulty/hostile peer or corruption; normal server production is not alleged. Use fallible traversal and validate declared cardinality/complete consumption. A discriminating oracle rejects these rows with a typed error rather than checking only symmetric valid roundtrips.

Evidence: [crates/shamir-client/src/interner_cache_ops.rs:731](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L731); [crates/shamir-client/src/interner_cache_ops.rs:657](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/interner_cache_ops.rs#L657); [crates/shamir-types/src/record_view/lens.rs:767](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/lens.rs#L767); [crates/shamir-types/src/record_view/lens.rs:979](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/record_view/lens.rs#L979); [crates/shamir-types/src/codecs/interned/codec.rs:227](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-types/src/codecs/interned/codec.rs#L227).

## Evidence and recipe corrections

- Entry counts are not a runtime-bug census. Intentional resume scope, optional Default/OnceLock, private close bookkeeping, and hypothetical wire evolution should not remain mandatory defect fixes.
- get_or_try_init does not broadcast one failed attempt's error to every waiter; waiters can retry initialization. A stored permanent Err is not equivalent retry behavior.
- The typed-error recipe names a nonexistent client::Error path; use the actual common::error::Error type or a deliberately designed wrapper.
- Pre-touch and refresh require a gap-free client completeness watermark, not just CAS-max of arbitrary observed IDs or the server's global high-water mark.
- src/tests/interner_cache_tests.rs is included in library runs despite its --full-only header. Its refresh test touches gamma through the same Client before refresh, so deleting refresh behavior leaves the asserted mappings intact.
- V2 roundtrips demonstrate transparent values, not independently that records_idmsgpack/IdBytes dispatch occurred. Ambient synchronization masks the purported unknown-ID refresh test.
- Partial write cancellation can invalidate the entire connection; RAII pending cleanup is not a complete cancellation-safety recipe.
- Framing soundness is positively contradicted by Tokio's initialized-buffer contract, not merely left unproved by a comment.
- The 16 MiB cap is not the specified 4 KiB pre-auth cap and does not bound header-derived IdBytes reservations.
- A sole current-server response-version field is not proof of support for every lower version; negotiation must preserve content semantics as well as the version stamp.
- Rotation approval must never be replaced by automatically clearing a pin. The current KDF spec describes an informational hint and admin-driven migration.
- Historical all-language 1:1, no rejected-ticket tests anywhere, exact allocation totals, scc lock-free, and debug atomic-panic assertions remain false.

## Module scope and limitations

Coverage: 8 assigned documents, 139 current claim rows, 24 plan rows, 2 pre-existing observation rows; 4 added observation rows in this cycle. Counts are calculated from the accepted rows.

Assigned documents: [SUMMARY.md](SUMMARY.md); [correctness-tdd.md](correctness-tdd.md); [concurrency-lockfree.md](concurrency-lockfree.md); [security-crypto.md](security-crypto.md); [performance-hotpath.md](performance-hotpath.md); [api-wire-protocol.md](api-wire-protocol.md); [error-handling-lifecycle.md](error-handling-lifecycle.md); [style-claude-md.md](style-claude-md.md).

- HEAD matched the required snapshot. All assigned Markdown, including collapsed histories and recipes, was inspected. No assigned TASK_GROUPS.md exists.
- Strictly read-only: no files written, programs reproduced, toolchains invoked, tests run, benchmarks run, child agents launched, or git state changed.
- Source-derived witnesses establish mechanisms, not measured exploit success, crashes, RSS, latency, throughput, or passing tests.
- Exact locked dependency archives were inspected without extraction. rmp-serde 1.3.1 and scc 3.8.4 archive SHA-256 values matched Cargo.lock. Extracted-source availability was checked separately.
- Threat-dependent decoder findings require a malformed or hostile peer, potentially enabled by unauthenticated resume. They are not evidence that the ordinary server emits those payloads.
- This is bounded claim revalidation, not an exhaustive audit of the crate or its dependencies.

## Guarantee checks

- **Subscriptions terminate with their connection, and early pushes flush before subsequently dispatched pushes.** — `diverges`. Registry senders survive connection death, including Client Drop because handles retain the registry. Separate lookup/buffer and registration/flush critical sections permit stranded and reordered pushes. Reference: docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md:35; docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md:333.
- **Full authentication refuses unauthorized TOFU and checks SCRAM proof, the supplied pin, and strict Ed25519 identity signatures.** — `supported`. Builder rejection and all three success-path checks are explicit. The pin-capture expect has an enforced invariant; this evidence predates the review and is not a later fix. Reference: crates/shamir-connect/src/client/handshake.rs:149; crates/shamir-connect/src/client/handshake.rs:258.
- **Resumption preserves authenticated server identity before exposing a bearer credential.** — `diverges`. Accept-any-certificate TLS is followed immediately by ticket transmission. pinned_hash is only copied into the resulting Client. Server anti-downgrade checks compare binding strength, not endpoint identity. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:36; crates/shamir-client/src/client.rs:866.
- **Rotation recovery requires verified transition material and explicit approval; KDF upgrades are informational hints.** — `diverges`. Both transport wire mirrors omit the hints, and process_auth_ok rejects a changed pin without invoking rotation verification. KDF migration is admin-driven in this spec, not an available self-service changePassword flow. Reference: docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:377; docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:871.
- **Server-provided KDF parameters are capped before derivation.** — `supported`. Effective protocol ceilings are 256 MiB, eight passes and eight lanes, with additional outer ceilings. This bounds one derivation; it does not prove aggregate OOM immunity or safety on every client device. Reference: crates/shamir-connect/src/client/handshake.rs:209; crates/shamir-connect/src/common/types.rs:123.
- **Peer-frame decoding is not unbounded recursive serde decoding.** — `supported`. The exact archive initializes depth to 1024 and guards array/map dispatch through any_inner. Response/Error envelopes, PushEnvelope and QueryValue use these paths. IdBytes are opaque to this guard and instead enter RecordView, whose walkers have a separate 128-level cap. Neither proves target-stack safety. Source: https://docs.rs/crate/rmp-serde/1.3.1/source/src/decode.rs Reference: Cargo.lock:2949; rmp-serde 1.3.1 published archive src/decode.rs.
- **A frame-size cap bounds all client decoder allocations and malformed nested records fail explicitly.** — `diverges`. An opaque IdBytes map header controls an uncapped IndexMap reservation before body validation. RecordView fields also silently end on malformed entries, allowing successful partial records. These paths are not covered by rmp-serde's outer depth counter. Reference: crates/shamir-client/src/interner_cache_ops.rs:724; crates/shamir-types/src/codecs/interned/codec.rs:227.
- **All client reads enforce the protocol's pre-authentication 4 KiB cap.** — `diverges`. Challenge/auth_ok/resume_ok reads use the 16 MiB data-frame constant. The report's narrower statement that reads are capped at 16 MiB is true, but pre-authentication cap conformance is not. Reference: docs/guide-docs/client-server-protocol-spec/TRANSPORT_TCP.md:29; crates/shamir-client/src/client.rs:509.
- **Inherited read_frame_into is sound because read_exact initializes bytes before they can be observed.** — `diverges`. Fresh capacity is exposed with set_len before initialization. Tokio 1.49.0 read_exact constructs ReadBuf::new, marking that entire slice initialized; safe AsyncRead implementations may inspect it. Cancellation also bypasses the error-path clear. The pinned Rust set_len contract requires initialization first. Sources: https://docs.rs/crate/tokio/1.49.0/source/src/io/util/read_exact.rs ; https://docs.rs/crate/tokio/1.49.0/source/src/io/read_buf.rs ; [Rust 1.94 source](https://raw.githubusercontent.com/rust-lang/rust/1.94.0/library/alloc/src/vec/mod.rs) Reference: crates/shamir-transport-tcp/src/framing.rs:118; Cargo.lock:4195.
- **FieldMap epoch is the highest gap-free ID actually known by the client.** — `diverges`. Sparse touch mappings advance the cache to their maximum ID and the server-wide epoch without supplying preceding mappings. Subsequent since=epoch refreshes cannot recover those holes. Reference: crates/shamir-client/src/interner_cache.rs:31; crates/shamir-client/src/interner_cache_ops.rs:275.
- **FieldMap forward/reverse mappings remain consistent and only server IDs enter them.** — `supported`. Normal SDK merging consumes server IDs and treats numeric-looking names as strings. The assurance is conditional on a stable repository identity and consistent pairs: insert_entry is public, the two insertions are not atomic, and conflicting pairs can create non-bijective mappings. Reference: crates/shamir-client/src/interner_cache.rs:96; crates/shamir-client/src/interner_cache_ops.rs:13.
- **Cache operations using scc::HashMap are truly lock-free.** — `diverges`. The exact package documents and implements bucket read/write locks and warns against blocking synchronous methods in asynchronous code. Lock-free resizing is not lock-free entry access. Source: https://docs.rs/crate/scc/3.8.4/source/src/hash_map.rs Reference: Cargo.lock:3123; scc 3.8.4 published archive src/hash_map.rs.
- **Timeouts bound precisely TCP establishment and post-write response awaiting.** — `supported`. Neither option covers TLS/SCRAM completion, write-lock acquisition or write_frame. Resume's absent knobs are an explicit documented scope choice, not a lost value carried from an existing Client. Reference: crates/shamir-client/src/client.rs:218; crates/shamir-client/src/client.rs:1277.
- **Cursor close and mid-pagination cancellation have discriminating registered tests.** — `supported`. Both tests probe the same cursor ID afterward for cursor_not_found. The mid-pagination fixture uses an admin session, so authorization bypasses the server's authorization-failure removal; the subsequent table lookup error itself retains the cursor. Removing client cancellation would therefore change the probe result. Cleanup awaits can still delay the original error indefinitely. Reference: crates/shamir-client/src/tests/cursor_stream_tests.rs:147; crates/shamir-client/src/tests/cursor_stream_tests.rs:194.
- **Default library selection includes src/tests live-server tests; crate-root integration targets require full selection.** — `supported`. All nine src/tests modules are registered in the library target. Eight crate-root integration files are auto-discovered. The interner_cache_tests header's claim that its src/tests tests require --full is false. Reference: crates/shamir-client/src/lib.rs:38; crates/shamir-client/src/tests/mod.rs:1; scripts/test.sh:177.
- **AtomicU32 fetch_add panics on debug overflow.** — `diverges`. The pinned Rust implementation explicitly specifies wrapping addition. Outstanding-ID replacement remains separate. Source: [Rust 1.94 atomic source](https://raw.githubusercontent.com/rust-lang/rust/1.94.0/library/core/src/sync/atomic.rs) Reference: crates/shamir-client/src/client.rs:1259; rust-toolchain.toml:21.
- **Node and TypeScript wrap this Rust SDK with identical capabilities.** — `diverges`. Node wraps selected methods but omits resume/version/timeout parity and serializes calls through an outer mutex. TypeScript is independent. Node's exact napi manifest pins do not establish identical client behavior. Reference: crates/shamir-client/src/lib.rs:26; crates/shamir-client-node/src/lib.rs:123.

## Reviewer's prior-cycle comparison

These are the independent reviewer's comparisons before parent refinements; the accepted ledgers above govern final decisions and counts.

- security-crypto.md#NF.5: unverified -&gt; confirmed-open/high. Exact Tokio 1.49.0 ReadBuf initialization semantics and cancellation supply a concrete inherited unsafe violation; a SAFETY comment is insufficient and contradicted.
- SUMMARY.md#NF.6 and style-claude-md.md#NF.5: unverified -&gt; refuted. The broad coverage assurance is positively contradicted by production-seam-bypassing and already-warm/ambient-masked oracles.
- SUMMARY.md#5.3, api-wire-protocol.md#4, error-handling-lifecycle.md#4 and Plan P1.8: confirmed-open -&gt; not-applicable. Commit 01f5883d and current source explicitly scope resume to None deadlines; parity is an enhancement, not loss of an existing supplied setting.
- SUMMARY.md#5.2 and #5.6, api-wire-protocol.md#2 and #7: confirmed-open -&gt; not-applicable for current defects. Current peer shapes match; the alleged failure requires prospective protocol changes. Repeated parse work remains independently visible and unmeasured.
- SUMMARY.md#5.8/api-wire-protocol.md#11, SUMMARY.md#2.4/concurrency-lockfree.md#7, and SUMMARY.md#1.12/correctness-tdd.md#13: confirmed-open -&gt; not-applicable. Default, OnceLock and an unobservable consumed-client flag are optional design preferences, not current broken public behavior.
- security-crypto.md#Nits/client.rs:603: confirmed-open -&gt; not-applicable. Capability checking while the server derives its fresh exporter is intentional; this is not a possession proof or a substitute for endpoint authentication.
- Copy/allocation and typed-error findings retain source-proven mechanisms but are downgraded where runtime severity was unsupported. Older-version fallback is medium and conditional on the documented historical peer configuration.
- The first cycle's core resume-theft, reference-carrier, subscription lifecycle/handoff, dump, provenance, cancellation-retention, atomic-wrap and scc counter-evidence conclusions are independently supported. No source-fixed label was warranted.
- Additional decoder and epoch witnesses arose directly from revalidating frame-size, de-interning and gap-free-cache guarantees; they do not rely on earlier report wording.
- Cursor cancellation coverage was challenged against actual server cleanup: the admin fixture bypasses authorization removal, while table lookup failure retains the cursor. Its same-ID probe is therefore discriminating, unlike the refresh and standalone-atomic tests.

## Current follow-up order

1. Repair inherited read_frame_into initialization/cancellation soundness; authenticate resumed endpoints before ticket transmission.
2. Close subscription ownership on every teardown and coordinate terminal state with pending/subscription registration; retain FIFO startup ordering.
3. Complete reference detection and preserve name-keyed dependent results, including caller-supplied encoding; add positive smart-path when/ForEach/ValueCompare oracles.
4. Validate opaque IdBytes cardinality and malformed-body traversal before allocation/de-intern success; retain result-to-repository provenance.
5. Separate sparse mappings from a gap-free completeness epoch; make dump failures retryable and actually test cold population/refresh.
6. Make frame writes cancellation-safe together with identity-safe pending cleanup; globally bound early-buffer keys/envelopes/bytes.
7. Implement content-safe older-version fallback and coordinated, explicitly approved identity rotation signaling.
8. Repair error taxonomy, secret-copy hygiene, DDL/test/doc drift; treat deadline parity, Default, OnceLock and wire evolution as explicit enhancement choices.
9. Measure allocation/copy optimizations before assigning substantial performance severity.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-client — Cross-Lens Review (all 7 lenses, synthesized)

Crate: `crates/shamir-client/` — the Rust client SDK: TLS 1.3 + SCRAM/Argon2id
handshake (via `shamir-connect`), rid-demultiplexed request/response over a
single connection, push subscriptions with an early buffer, server-side cursor
streaming, and the v2 interner-cache ops (dump/touch/de-intern/id-keyed
encode). This is the core SDK that `shamir-client-node` (napi) and
`shamir-client-ts` wrap — those two are reviewed separately.

Review basis: the seven 2026-08-14 lens reports under
`docs/dev-artifacts/audits/2026-08-14-cross-crate-rush-review/shamir-client/` —
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`,
`performance-hotpath.md`, `api-wire-protocol.md`,
`error-handling-lifecycle.md`, `style-claude-md.md` — read in full and
synthesized read-only (no builds, no tests, no source modifications). The
workspace `SUMMARY.md` was consulted for this crate's per-crate row and
health-scorecard verdict (context only; its numbers are pre-dedup
lens-tagged counts). Structure/tone calibrated on the two completed exemplar
summaries: `shamir-client-node/SUMMARY.md` and
`shamir-transport-ipc/SUMMARY.md`.

> **Line-reference note (added during synthesis).** Cited line numbers are as
> captured by the 2026-08-14 review. The working tree has since gained
> IPC-transport integration in `client.rs` (`WriteSink::Tls | WriteSink::Ipc`,
> `connect_local`/`ConnectLocalOptions`, `reader_task` generic over
> `AsyncRead`), shifting that file's lines (~+75 at `decode_frame`, growing to
> ~+275 by `roundtrip`); all other cited files (`interner_cache_ops.rs`,
> `wire_frames.rs`, `subscription.rs`, `error.rs`, `lib.rs`,
> `cursor_stream.rs`) still match the audited refs exactly. Load-bearing sites
> were spot-checked in the current tree and every finding below still holds
> substantively. Key remaps: reader exit now `client.rs:407-416` (audited
> 318-327); `roundtrip` closed-check/insert now `:1240`/`:1268-1272` (audited
> 966/993-998); `REQ_BUF` clone now `:1256` (audited 982); `subscribe_push`
> now `:996-1024` (audited 722-750); resume `pinned_hash` store now `:937`
> (audited 663); `query_version` stamp now `:1061` (audited 786-790);
> `get_ddl_op_status` now `:1187-1227` (audited 913-953). Note that the
> post-audit IPC integration itself is covered by none of the 7 lenses.

## Executive summary

The SDK's connect path, wire hygiene, test layout, and builder-only query
discipline are genuinely strong, but the crate is not shippable as-is: the
reader-exit seam produces **two permanent-hang defects** — subscription
channels are never closed on disconnect, so every live subscriber's
`next().await` hangs forever (unconditional, no race needed; found
independently by three of the seven lenses, workspace headline issue #2 and
workspace **P0 #8**), and a `roundtrip` closed-check/drain race hangs
in-flight requests forever under the default `request_timeout = None` — while
the **resume path performs zero server-identity verification** and hands the
bearer resumption ticket to whatever peer accepts any certificate. Two further
highs silently break correctness against real deployments: `query_version: 2`
is stamped unconditionally, making the carefully built v1-fallback ladder dead
code against exactly the servers it was built for, and `batch_has_refs` misses
`when` guards and `ForEach`, corrupting v2 id-keyed encoding. Fix those five
first.

---

## 1. correctness-tdd

### 1.1 — high — Reader exit never closes subscription channels: every live subscriber hangs forever on any disconnect
- File:line: `crates/shamir-client/src/client.rs:318-327` (reader exit path;
  current: `:407-416`), `crates/shamir-client/src/subscription.rs:54-65`.
- **Dedup: also flagged by error-handling-lifecycle #2** (rated medium there;
  re-rated high in the workspace catalog, which carries it as workspace
  **P0 #8**: "reader exit never closes subscription channels — every live
  subscriber hangs forever on any disconnect (unconditional, no race needed)").
  Together with 2.1 this is the reader-exit seam that three lenses
  (correctness, concurrency, error-handling) converged on independently — one
  of the workspace's top-3 headline issues.
- Issue: `reader_task`'s exit path marks `closed` and drains only the
  `pending` map. It never touches `subscriptions` (nor `early_buffer`). The
  registry entry is the SOLE holder of the mpsc `tx` (the local clone in
  `subscribe_push` is dropped on return; the handle holds only `rx`), and
  every live `SubscriptionHandle` keeps the registry `Arc` — and therefore the
  `tx` — alive. On server EOF / I/O error / TCP reset, a consumer blocked in
  `handle.next().await` pends forever; the documented contract "`None` if the
  subscription was closed" (`subscription.rs:53`) never fires. Compounding it,
  `subscribe_push` has no `closed` check (error-handling lens's addition): on
  a dead connection it happily registers a new channel nobody will ever feed —
  `roundtrip` guards with `closed`; the subscription path is the one
  data-plane surface without an equivalent.
- Failure scenario: server restarts mid-subscription; the client's consumer
  loop `while let Some(env) = handle.next().await { .. }` hangs indefinitely —
  no `None`, no error, and any subsequently created handle hangs identically.
  CLAUDE.md is explicit: "Hangs and test-locks are BUGS — hunt and fix them,
  never tolerate."
- Suggested fix: on reader exit, `subscriptions.lock().clear()` (dropping all
  senders so every `rx.recv()` resolves to `None`) and clear the early buffer;
  make `subscribe_push` return an already-closed handle (or error) when
  `closed` is set. Add a Red test: reader_task EOF with a registered
  subscription must deliver `None` to `next()` (this also closes error-handling
  #7's untestable-as-written gap — see 6.2).

### 1.2 — high — `batch_has_refs` misses two ref carriers: `QueryEntry.when` guards and `BatchOp::ForEach`
- File:line: `crates/shamir-client/src/interner_cache_ops.rs:513-532`
  (`op_has_refs`; verified current: `:513-532` unchanged), cross-ref
  `shamir-query-types/src/batch/query_entry.rs:55-62` and
  `shamir-query-types/src/batch/for_each_op.rs:20-38`.
- Issue: `op_has_refs` scans only the op's own filter, treats
  `Batch(_) => true` unconditionally, and falls to `_ => false` for everything
  else. Two documented ref-bearing shapes escape it: (a) `QueryEntry.when:
  Option<Filter>` — the guard whose own doc says "only
  `$query`/`$fn`/`$param`/literals are meaningful" (its canonical use is "run
  this op iff `$query_ref_A >= $query_ref_B`"); (b) `BatchOp::ForEach`, whose
  `over: FilterValue` is canonically `over: $query @a[].id` and whose nested
  `batch.queries` body is never recursed — despite ForEach being documented as
  "structurally a sibling of SubBatchOp" and despite the #660 precedent in
  `query_entry.rs` that a flat walk silently skips what nested bodies touch.
- Failure scenario: on a v2 server, `execute_with_touch` of a batch whose ops
  are when-guarded on earlier aliases, or that loops `ForEach` over a query
  result, passes `batch_has_refs == false` → `result_encoding = Id` →
  intermediates become opaque `QueryRecord::IdBytes` (`as_value() == Null`) →
  `$query` path resolution silently breaks — the exact failure mode this guard
  was built to prevent (proven at engine level by
  `query_ref_does_not_resolve_under_id_encoding`). The existing e2e
  when/for-each tests all call `execute`, never `execute_with_touch`, so the
  broken combination is untested.
- Suggested fix: `batch_has_refs` should take `&QueryEntry` (not `&BatchOp`)
  and scan `entry.when` with `filter_has_refs`; add `BatchOp::ForEach(fe) =>
  fv_has_refs(&fe.over) || fe.batch.queries.values().any(op_has_refs)`
  (recurse). Add Red tests for both shapes in `batch_has_refs_tests.rs`, plus
  one e2e `execute_with_touch` when/for-each test.

### 1.3 — medium — `subscribe_push` / early-buffer routing race strands pushes (and reorders delivery)
- File:line: `crates/shamir-client/src/client.rs:240-270` (reader routing;
  current miss-path `:330-359`) vs `client.rs:722-750` (`subscribe_push`;
  current `:996-1024`).
- **Dedup: also flagged by concurrency-lockfree #4** (rated low there).
- Issue: the reader decides routing on the `subscriptions` map lookup, then
  (after releasing that lock) pushes to `early_buffer` on miss.
  `subscribe_push` inserts into the map and, in a SEPARATE critical section,
  drains the early buffer (verified current: insert `:998-1001`, flush
  `:1003-1022`). Interleaving: reader map-lookup miss → `subscribe_push`
  inserts → `subscribe_push` buffer-remove (empty) → reader buffer-push. The
  envelope is stranded in `early_buffer` forever — silently lost (only a later
  `subscribe_push` for the same id would flush it). A second interleaving
  flushes *older* buffered pushes after *newer* direct sends to the same
  channel (ordering violation within one sub). This contradicts the module's
  own "no loss" claim (`demux_tests.rs:312`).
- Failure scenario: a push arriving exactly during `subscribe_push` startup is
  silently lost or reordered; for change-feed consumers this is a missed event
  with no error surfaced.
- Suggested fix: route under one lock acquisition (reader holds the
  `subscriptions` lock across the miss → early-buffer append, with
  `subscribe_push` using the same lock order), or move the flush inside the
  map-insert critical section (or merge the two maps into one guarded
  structure / use `scc`'s `entry_sync`, per concurrency #4). Red test:
  concurrent push-then-subscribe loop asserting no envelope is stranded.

### 1.4 — medium — `dump_repo`: failed dump poisons the OnceCell as "populated", later calls never re-dump, and the `Result` lies
- File:line: `crates/shamir-client/src/interner_cache_ops.rs:180-223`
  (verified unchanged), `crates/shamir-client/src/interner_cache.rs:35-38,
  161-176`.
- **Dedup: also flagged by error-handling-lifecycle #3** (rated medium there).
- Issue: the `ensure_populated` closure swallows all errors (warn + resolve
  `()`, verified `:199-218`) so tokio's `OnceCell::get_or_init` initializes
  the cell even when the dump roundtrip or parse FAILED — contradicting the
  `FieldMap` doc ("`populated` is set once the first full interner_dump
  succeeds") and `is_populated()`. Worse, because the cell is now
  initialized, `get_or_init` never runs the closure again: `dump_repo`'s own
  doc ("Subsequent calls re-dump unconditionally") is false — every subsequent
  call is a silent `Ok(())` no-op. The inline comment ("the cell stays
  uninitialized and a later call retries", `:187-190`) describes behavior the
  code cannot have. The error-handling lens adds the API-contract framing: the
  `Result<(), ClientError>` signature promises what it cannot deliver — a
  caller doing `client.dump_repo(..).await?` cannot detect that the dump
  failed.
- Failure scenario: transient network error on the first `dump_repo` → cache
  empty, `is_populated() == true`, and `dump_repo` can never populate it; the
  failure surfaces much later as `encode_record_idmsgpack`'s misleading
  `"field 'x' not in FieldMap — touch_fields must be called first"` even
  though the caller did warm the cache — or as silent `resolve_field() ==
  None`.
- Suggested fix: use `get_or_try_init` (or an explicit flag set only after a
  successful merge) so failures leave the cell clean and retryable; return the
  error instead of swallowing it into `Ok(())` (or store last-error in the
  cell). Red tests: failed dump leaves `is_populated() == false` and a retry
  succeeds; second `dump_repo` after success actually re-dumps (or fix the
  doc).

### 1.5 — medium — Multi-repo v2 de-intern: first-match FieldMap can silently attach wrong field names
- File:line: `crates/shamir-client/src/interner_cache_ops.rs:695-736`
  (`try_deintern_repos`; verified current `:710+`).
- Issue: `try_deintern_repos` returns the FIRST repo whose map resolves every
  id. Interner ids are small monotonic integers per repo, so cross-repo id
  overlap is the common case, not the edge: in a multi-repo batch, repo B's
  row can be de-interned with repo A's names and returned to the caller as if
  correct. The comment acknowledges the assumption ("best-effort first-match")
  but the disambiguating data is available and unused — `execute_with_touch`
  walks `entry.op.table_ref()` already and could pass a per-alias (or
  per-result) repo mapping instead of a flat repo list.
- Failure scenario: batch touching `repo_a` and `repo_b`, both with id 1 =
  different names; `repo_b`'s returned rows carry `repo_a`'s field names —
  silent data mislabel.
- Suggested fix: resolve each result alias to its op's `table_ref().repo` and
  de-intern against that single map (falling back to the current probe only
  when unknown); keep the refresh-retry path.

### 1.6 — medium — Vacuous test: `atomic_u8_plumbing_stores_and_reads_correctly` tests std, not the client
- File:line: `crates/shamir-client/src/tests/wire_version_tests.rs:135-142`.
- **Dedup: also flagged by api-wire-protocol #12** (rated nit there).
- Issue: the test's doc claims to verify "a Client whose WireAuthOk carries
  `server_query_version=2` should expose that via `server_query_version()`",
  but the body constructs bare `std::sync::atomic::AtomicU8` values and
  asserts the standard library stores/loads them. It cannot fail due to any
  client bug — the exact vacuous-test anti-pattern CLAUDE.md's
  Red/Green/Refactor protocol exists to prevent. The real plumbing
  (`connect`/`resume` → `WireAuthOk.server_query_version` → `AtomicU8` →
  getter, `client.rs:573, 674` as audited) is covered only incidentally by e2e
  guards (`server_query_version() < 2` early-returns), never asserted
  positively.
- Failure scenario: a refactor of the version plumbing compiles and the suite
  stays green while every client silently misreports the negotiated version.
- Suggested fix: delete the test or replace it with one that drives
  `Client::connect` against a live server and asserts
  `server_query_version() == 2` (the harness already exists in
  `v2_passthrough_tests.rs`).

### 1.7 — low — `get_ddl_op_status`: comment/doc/behavior three-way contradiction, dead `DbResponse::Error` arm, zero coverage
- File:line: `crates/shamir-client/src/client.rs:913-953` (audited; verified
  current `:1187-1227`), dead-arm cross-ref `client.rs:1018-1024` as audited
  (current `:1293-1298`).
- **Dedup: also flagged by api-wire-protocol #9 (low), error-handling-lifecycle
  #6 (low), and security-crypto nits** (comment/behavior mismatch) — four
  lenses, one method, one contract problem.
- Issue: (a) the inline comment says `not_supported` is treated "as 'feature
  unavailable' rather than a hard error", but the code returns
  `Err(ClientError::Protocol(..))` for it — indistinguishable at the call site
  from a hard failure except by string matching the variant's payload, and
  both branches return `Err` anyway (api lens); (b) the method doc ("`None` if
  the operation is unknown (GC'd, never existed, or a pre-RFC op…)")
  suggests an old server should surface as `Ok(None)`; (c) the
  `DbResponse::Error { code, message } =>` match arm is semantically dead —
  `roundtrip` converts any in-band `DbResponse::Error` into
  `Err(ClientError::Db { .. })` and never returns `Ok(DbResponse::Error)`, so
  the intended `not_supported` → `Protocol` reshaping can never fire (callers
  see `ClientError::Db { code: "not_supported" }` instead), and clippy cannot
  flag a semantically-unreachable arm (error-handling lens); (d) no unit or
  e2e test exercises any branch of this public method.
- Failure scenario: a caller wanting "poll status; if unsupported, skip" must
  match `Err(ClientError::Protocol(m)) if m.contains("not supported by
  server")` — brittle — or is surprised by `Err(Db { code:
  "not_supported" })`; future editors waste effort "fixing" the dead arm.
- Suggested fix: match on `Err(ClientError::Db { code, .. })` from
  `roundtrip` for the reshaping — return `Ok(None)` for `not_supported`
  (matches the doc) or add a dedicated `ClientError::NotSupported` variant —
  delete the dead arm, fix the comment, and add tests for the found / unknown
  / not_supported branches.

### 1.8 — low — Duplicate `subscribe_push(sub_id)`: dropping the first handle closes the second
- File:line: `crates/shamir-client/src/client.rs:722-750` (current
  `:996-1024`), `crates/shamir-client/src/subscription.rs:59-65` (verified:
  `Drop` removes by `sub_id` unconditionally).
- Issue: a second `subscribe_push` for the same id overwrites the registry
  entry, dropping the map's last clone of the first `tx` (first handle cleanly
  sees `None`). When the FIRST handle is later dropped, its `Drop` removes the
  registry key — which now holds the SECOND handle's live sender. The second
  handle's channel closes prematurely (`next()` → `None`) and all subsequent
  pushes strand in the early buffer.
- Failure scenario: reconnect/reshSubscribe flows that register twice for a
  sub_id observe a mystery channel closure plus silent push loss whenever the
  stale handle is dropped after the fresh one is created.
- Suggested fix: guard against double registration (return an error / reuse
  the existing sender), or make removal conditional on identity (store an
  `Arc` token per handle and remove only if it is still the registered one).

### 1.9 — low — `collect_map_keys` skips `Value::Set` (asymmetric with `qv_has_fn_marker`)
- File:line: `crates/shamir-client/src/interner_cache_ops.rs:468-480` vs
  `490-503` (verified unchanged).
- Issue: the touch-collector recurses through `Value::Map` and `Value::List`
  only, while the `$fn` detector also recurses `Value::Set`. An INSERT record
  containing a set of maps registers the nested maps' keys neither for touch
  nor (therefore) for the v2 id-keyed encode.
- Failure scenario: `execute_with_touch` of a record with a set-of-maps value
  on a v2 server fails loudly with `Protocol("field '...' not in FieldMap —
  touch_fields must be called first")` despite the pre-touch pass having run.
- Suggested fix: add a `Value::Set(items)` arm to `collect_map_keys` mirroring
  `qv_has_fn_marker`, and a unit test for the shape.

### 1.10 — low — TDD-coverage gaps: untested public paths
- File:line: `crates/shamir-client/src/client.rs:588-677` (`resume`; current
  `:862-951`), `client.rs:722-750` (`subscribe_push` flush; current
  `:996-1024`), `client.rs:1032-1050` (close/Drop; current `:1306-1324`),
  `crates/shamir-client/src/tests/resume_wire_tests.rs:1-89`.
- Issue: (a) `Client::resume` has only frame serde round-trip tests — no
  live-server test in this crate drives the full TLS+resume+reader path (the
  happy-path claim of `ResumeOptions` is untested end-to-end; overlaps
  security #6 and error-handling #7); (b) the `subscribe_push` early-buffer
  flush path is never tested (demux tests insert senders into the map by
  hand); (c) the §B21 contract "reader aborted on close()/Drop" has no test
  (e.g. leak/liveness assertion after dropping a Client with an open
  connection); (d) the `when`-guard × `execute_with_touch` combination
  (finding 1.2) is untested. Kept separate from error-handling #7 (6.2) and
  concurrency #6 (2.3): same seam, different bundles.
- Suggested fix: add Red tests for each before fixing the corresponding
  behavior.

### 1.11 — nit — `connect_timeout_fires` depends on 10.255.255.1:9 being a silent black hole
- File:line: `crates/shamir-client/src/tests/timeout_tests.rs:26, 37-59`.
- Issue: on networks/VPNs that answer the black-hole SYN with fast
  EHOSTUNREACH / ENETUNREACH, `connect_tcp` returns an io error well before
  the 250 ms budget and the `elapsed >= budget` assertion fails — an
  environment-flaky test.
- Suggested fix: bind a local listener, accept nothing, and drop/never-read
  the socket (deterministic "accepted but silent" endpoint), or accept-and-hold
  the socket.

### 1.12 — nit — `close()` never sets the `closed` flag though `ConnectionClosed`'s doc lists "explicit close()" as a trigger
- File:line: `crates/shamir-client/src/client.rs:1028-1039` (current
  `:1306-1313` — verified: aborts reader + shuts down the write half, no
  `closed.store`), `crates/shamir-client/src/error.rs:41-44`.
- Issue: harmless today (`close` consumes `self`, so no caller can observe the
  flag afterward; waiters are released when the Client's `pending` map
  drops), but the doc and the flag disagree.
- Suggested fix: set `closed` in `close()` before shutdown (cheap, keeps the
  flag truthful for any future shared-handle design), or align the doc.

### 1.13 — nit — `next_request_id` u32 `fetch_add` overflow panics in debug builds / aliases silently in release
- File:line: `crates/shamir-client/src/client.rs:349, 985` (audited; verified
  current `:1259`).
- **Dedup: also flagged by concurrency-lockfree #8** (nit there, with the
  sharper consequence: after 2^32 requests a wrapped rid can alias an entry
  still present in `pending`, overwriting a live sender — one caller hangs,
  another gets a foreign response).
- Issue: practically unreachable; wrapping to rid 0 is harmless in the common
  case (rid is echoed opaquely and the pending map is keyed by it), but the
  debug-overflow panic and the theoretical alias are both avoidable.
- Suggested fix: at wrap, return a `Protocol` error or force connection close
  (`closed.store(true)`), turning a silent cross-delivery into an explicit
  lifecycle event; or `fetch_update` with a documented wrap.

## 2. concurrency-lockfree

### 2.1 — high — Reader-exit drain races pending registration → permanent hang of an in-flight request
- File:line: `crates/shamir-client/src/client.rs:966` (`closed.load` in
  `roundtrip`; current `:1240`), `client.rs:993-998` (register-into-pending;
  current `:1268-1272`), `client.rs:318-327` (reader's `closed.store` + drain;
  current `:407-416`).
- **Dedup: also flagged by error-handling-lifecycle #1** (rated high there).
  This is the second half of the reader-exit seam that three lenses converged
  on independently (see 1.1); the workspace catalog groups both under the
  headline hang class.
- Issue: `roundtrip` checks `closed` *before* inserting its oneshot sender
  into `pending`, and the reader task stores `closed = true` *then* drains
  `pending` as two unordered steps (verified current: `closed.store` `:408`,
  lock+drain `:409-415`). A caller can pass the `closed` check, have the
  reader store `closed` and drain (missing the caller's rid), and *then*
  insert its sender into the map. The reader task has exited; the map stays
  alive via `Client.pending`; the sender is never dropped and never sent.
- Failure scenario: server restarts / connection EOF at the moment the client
  issues a request, with `ConnectOptions::request_timeout = None` (the
  documented default, "preserves the prior unbounded-wait behaviour" — the
  error-handling lens notes the #520 timeout knobs exist precisely for this
  class but are `None` by default). The caller's `rx.await` never resolves —
  an un-killable task hang of exactly the class CLAUDE.md says must be hunted,
  never tolerated. Window is narrow but recurring under reconnect storms;
  `src/tests/demux_tests.rs` only covers EOF-drain with waiters
  pre-registered, so the race is untested.
- Suggested fix: order the two sides against each other: in `reader_task`,
  acquire the `pending` lock, then `closed.store(true, Release)` and drain
  while still holding it; in `roundtrip`, after `map.insert(rid, tx)` under the
  same lock, re-load `closed` (Acquire) and if set, remove the entry and
  return `ConnectionClosed`. Then either the insert precedes the reader's
  locked drain (drain catches it) or the re-check sees `closed == true`
  (caller self-cleans). Add a regression test that interleaves
  insert-after-drain (see 2.3/6.2).

### 2.2 — high — `std::sync::Mutex` on the demux hot paths (`PendingMap`, `SubscriptionMap`, `EarlyBuffer`) without a sanctioned-category justification
- File:line: `crates/shamir-client/src/client.rs:161` (`pub(crate) type
  PendingMap = Arc<StdMutex<TFxMap<u32, PendingSender>>>`; current `:250`),
  constructed at `client.rs:545, 648` as audited (current `:820-822,
  922-924`), locked per request at `client.rs:996` (current `:1268-1272`) and
  per response at `client.rs:302` (current `:389-393`); subscriptions/
  early-buffer aliases at `subscription.rs:28, 33`, locked per push frame at
  `client.rs:242, 260` as audited (current `:331, :349`), plus
  `client.rs:725/730` (`subscribe_push`, current `:999/:1004`) and
  `subscription.rs:62` (`SubscriptionHandle::drop`).
- **Dedup: one root cause, flagged by concurrency-lockfree #2 (high,
  PendingMap), concurrency-lockfree #3 (medium, SubscriptionMap/EarlyBuffer),
  and performance-hotpath #5 (medium, all three maps)** — counted once here.
- Issue: Pillar 1/5 mandate lock-free `scc::HashMap`/`DashMap` for shared
  registries, and CLAUDE.md states `std::sync::Mutex` is "banned in hot
  paths" outside the three sanctioned categories (dead scaffolding / DDL-only
  guard sets / first-touch-only population). The pending map is locked twice
  on *every* request/response round trip — the hottest path in the SDK — and
  every incoming push frame locks the subscriptions (and often early-buffer)
  maps; none of these fit a sanctioned category (not dead, not DDL, and every
  request touches a *new* rid / every unregistered push re-writes the
  buffer). The inline comments (`client.rs:301, 995` as audited; verified
  current `:390, :1269` — "std::sync::Mutex, no .await while held") address
  await-safety/poisoning, not the required contention-model argument. `scc`
  is already a dependency of this very crate (`interner_cache.rs`).
- Failure scenario: N tasks pipelining concurrent `execute` calls (the
  documented "fully supported" mode) serialize all rid registrations behind
  one global mutex contending with the single reader task's per-response
  removals; push-heavy subscriptions stall frame routing behind
  `subscribe_push` registrations and handle drops. Throughput degrades with
  fan-in — exactly what pillar 1 exists to prevent.
- Suggested fix: `scc::HashMap<u32, PendingSender, THasher>` for pending and
  `scc::HashMap<u64, PushSender, THasher>` / `scc::HashMap<u64,
  Vec<PushEnvelope>, THasher>` for registry/early-buffer (`insert_sync`/
  `remove_sync` are lock-free and no `.await` is involved; `or_default`+push
  maps naturally onto `entry_sync`) — or `DashMap::with_hasher(THasher::
  default())`. Finding 2.1's ordering fix then moves to a post-insert
  `closed` re-check + `remove_sync`. If the single-reader argument is
  considered load-bearing, it must be written as an inline contention-model
  comment per site per CLAUDE.md — but a lock-free primitive removes the
  need.

### 2.3 — low — Concurrency-claim test gaps: stampede guard and drain race never exercised concurrently
- File:line: `crates/shamir-client/src/tests/interner_cache_tests.rs` (no
  `tokio::spawn` anywhere in the file; `dump_repo` OnceCell guard tested only
  sequentially at ~line 194), `crates/shamir-client/src/tests/demux_tests.rs`
  (EOF-drain tests pre-register all waiters, lines 175-207).
- Issue: the module docs and `dump_repo`'s doc claim "concurrent first-callers
  share one dump roundtrip (stampede guard)", but no test spawns two
  concurrent `dump_repo` calls against one `FieldMap`, so the `OnceCell`
  dedup (and 1.4's swallow-error path) is only verified in sequence. Likewise
  nothing covers 2.1's insert-after-drain interleaving. (Overlaps error-
  handling #7 / 6.2 on the reader-exit test bundle; kept separate — the
  stampede half is unique to this finding.)
- Failure scenario: a future refactor of `ensure_populated`/`reader_task`'s
  drain can regress silently; the exact hang class the nextest timeouts exist
  to surface would only appear as an e2e `TIMEOUT`.
- Suggested fix: add (a) a multi-task test that fires N concurrent
  `dump_repo`s and asserts exactly one `interner_dump` roundtrip reaches the
  server, and (b) a demux test that registers a waiter after the reader has
  drained and asserts it resolves with `ConnectionClosed` (once 2.1 is
  fixed).

### 2.4 — nit — `CursorStream::cursor_id` cell could be a `OnceLock` instead of `StdMutex`
- File:line: `crates/shamir-client/src/cursor_stream.rs:221, 227, 250, 276`.
- Issue: `Arc<StdMutex<Option<CursorId>>>` is written exactly once (guarded by
  the `is_none()` fast path at `:251`, correctly commented) and read
  thereafter — a textbook first-touch-only cell. It qualifies as a setup-only
  fallback, but `std::sync::OnceLock<CursorId>` expresses the write-once
  invariant, removes the poison-handling boilerplate, and makes reads
  branch-free.
- Suggested fix: replace with `OnceLock<CursorId>` (keep the existing guard
  comment; it becomes the `OnceLock` doc).

## 3. security-crypto

### 3.1 — high — `Client::resume` hands the bearer ticket to an unverified peer; `pinned_hash` is dead bookkeeping on the resume path
- File:line: `crates/shamir-client/src/client.rs:588-677` (esp. 591-604,
  616-629, 663; current `:862-951` — `make_client_config_no_ca()` `:866`,
  exporter extracted-then-discarded `:877`, `pinned_hash:
  opts.pinned_hash` verbatim `:937`); `crates/shamir-client/src/
  wire_frames.rs:40-64` (verified: `WireResumeOk` carries no
  `server_pub_key`/`identity_sig`); TLS context:
  `shamir-transport-tcp/src/tls.rs:63-129`.
- **Dedup: one root defect, flagged by three lenses — security-crypto #1
  (high), correctness-tdd #3 (high, "correctness-of-invariants" framing), and
  api-wire-protocol #3 (medium, API-contract framing). The security lens's
  nit (TLS exporter extracted solely to prove availability, then discarded
  while `WireResumeInit.binding_mode` declares `TlsExporter`) folds into this
  fix.**
- Issue: `make_client_config_no_ca()` accepts *any* server certificate; all
  server authentication on `connect` lives in the SCRAM handshake
  (`process_auth_ok`: mutual-auth proof + pin + Ed25519 over the TLS
  exporter). `Client::resume` re-uses the accept-any-cert TLS config but
  performs none of those checks: `WireResumeOk` carries no
  `server_pub_key`/`identity_sig`, `ResumeOptions::pinned_hash` is stored
  verbatim into the client without ever being compared to anything, and the
  TLS exporter is extracted only to be discarded (`let _exporter`) while
  `WireResumeInit.binding_mode` still *claims* `TlsExporter`. The very first
  authenticated payload sent on this connection is the long-lived bearer
  ticket itself. The correctness lens adds the invariant framing: the
  `pinned_hash` field's documented invariant ("SHA256(server_ed25519_pub_key)
  … validated"; `ConnectOptions` doc: "refuses on mismatch (spec
  ServerIdentityChanged)") is silently violated, and `server_pub_key_pin()`
  afterwards reports a pin that was never checked this session — callers that
  persist the getter's output create a tautology.
- Failure scenario: an active MITM terminates the TLS connection (any cert is
  accepted), reads `WireResumeInit.ticket` in cleartext, and now owns the
  victim's credential — it can open its own authenticated session against the
  real server (the server-side first-use-wins counter bounds *concurrent*
  use, but the attacker acts first and the victim's resume loudly fails) or
  transparently proxy the session. The api lens adds the counterpoint that
  keeps this honest: the resumption ticket's exporter binding currently makes
  relay/impersonation fail server-side, so it is "not currently exploitable"
  *if* the server enforces the binding — but that binding is decorative on the
  client side, a MITM downgrade of `binding_mode` is indistinguishable to the
  caller, and any future loosening of the exporter check would leave nothing
  client-side to catch it. Server-side #512 analysis
  (`shamir-connect/src/server/resume.rs:324-361`) covers *already-stolen*
  tickets; it does not cover this path *creating* stolen tickets — `connect`
  would never disclose the ticket to a MITM, `resume` does.
- Suggested fix: authenticate the server before/at resume completion: extend
  `WireResumeOk` with `server_pub_key` + Ed25519 `identity_sig` over
  `(client_nonce, session_id, expires_at_ns, new-connection TLS exporter)` and
  verify strictly against `ResumeOptions::pinned_hash` before returning a
  usable `Client` (fail with a `ServerIdentityChanged`-style error on
  mismatch) — using the exporter for real, which subsumes the folded nit. If
  instead the design is "identity enforced server-side via ticket+exporter
  binding", rename/document `ResumeOptions.pinned_hash` as carry-through
  metadata only and stop advertising `server_pub_key_pin()` as verified for
  resumed sessions (api lens). Interim: document that `resume` must only run
  over networks where the server endpoint is otherwise authenticated. Add a
  Red test asserting a wrong-identity resume is rejected (see 3.5).

### 3.2 — medium — SDK silently discards `rotation_in_progress` and `kdf_upgrade_required` from `auth_ok` — orphan-recovery (spec §6.5) and KDF-upgrade hints (§13) are unreachable
- File:line: `crates/shamir-client/src/wire_frames.rs:66-86` (verified:
  `WireAuthOk` lacks both fields), `crates/shamir-client/src/client.rs:
  506-516` (audited; verified current `:797-798` — hard-codes
  `rotation_in_progress: None, kdf_upgrade_required: None`).
- Issue: the protocol and the connect crate support both signals:
  `AuthOkView` carries them (`shamir-connect/src/server/handshake.rs:99-103`),
  the server library can attach them (`with_rotation_in_progress` /
  `with_kdf_upgrade_required`, `complete_auth_ok`), and the client side of
  shamir-connect has `verify_rotation_in_progress` for exactly the
  orphan-client case. `shamir-client` neither deserializes nor forwards
  either field — msgpack decode drops them, and the values handed to
  `process_auth_ok` are hard-coded `None`.
- Failure scenario: server performs an identity-key rotation (the mechanism
  §6.5 exists for) → every Rust-SDK client (and the napi/Node binding wrapping
  this crate 1:1) that persisted a pin gets
  `ClientError::Handshake("ServerIdentityChanged")` on every connect, forever,
  with no in-band recovery even though the protocol defines one; similarly
  clients are never told to upgrade stale Argon2id parameters.
  Availability/operational impact, fail-closed rather than exploitable — but
  it turns routine key rotation into a manual pin-wipe for all SDK users.
- Suggested fix: add both fields to `WireAuthOk` with `#[serde(default)]`,
  pass them through to `ServerAuthOk`, and surface them on the SDK result
  (e.g. `Client::connect` returns/exposes the rotation payload after
  `verify_rotation_in_progress` succeeds, plus a `kdf_upgrade_required()`
  accessor).

### 3.3 — medium — Cleartext password persists in unzeroized serialization buffers after `create_scram_user`
- File:line: `crates/shamir-client/src/client.rs:975-989` (thread-local
  `REQ_BUF`, `req_bytes` clone, `envelope_bytes`; verified current:
  `REQ_BUF` `:1249-1257`, clone `:1256`, envelope `:1260-1263`),
  `crates/shamir-client/src/client.rs:828-842` (audited; verified current
  `:1091-1123` — cleartext copy into the request at `:1104`).
- Issue: `roundtrip` serializes the `DbRequest` — for `CreateScramUser` the
  wire field is the plaintext password — into (a) the **thread-local**
  `REQ_BUF`, which retains the bytes after the call (only `clear()`ed at the
  *next* request on that thread, i.e. persists until overwrite/thread exit),
  (b) `buf.clone()` → `req_bytes`, a plain `Vec`, and (c) `envelope_bytes`,
  another plain `Vec`. The `drop(req)` and its comment ("wipe ASAP") wipe only
  the typed `SecretString`; at least three unzeroized heap copies of the
  password outlive the call, defeating the zeroize discipline the surrounding
  code (and the crate's dependency posture: `Zeroizing` password, `Zeroizing`
  ticket) is written to provide. (Same buffers as perf finding 4.1 — the
  zero-copy rewrite is the natural place to add the zeroization.)
- Failure scenario: heap inspection / core dump / swap capture after a
  `create_scram_user` recovers the new user's cleartext password long after
  the request completed — precisely what the wipe-on-drop effort elsewhere is
  meant to prevent.
- Suggested fix: zeroize the occupied region of `REQ_BUF` after the clone (or
  immediately after `write_frame` completes), and route secret-bearing
  requests' `req_bytes`/`envelope_bytes` through `Zeroizing` (or explicit
  `.zeroize()` on all exit paths of `roundtrip`).

### 3.4 — low — No depth guard on nested msgpack decode of peer frames
- File:line: `crates/shamir-client/src/client.rs:121-135` (`decode_frame`;
  current `:194+`), `client.rs:240` (`PushEnvelope::from_slice`; current
  `:329`), `client.rs:1018` (`DbResponse` decode; current `:1292`).
- Issue: server-supplied frames (bounded at 16 MiB) are decoded via recursive
  serde deserialization into `DbResponse`/`PushEnvelope`, whose
  `QueryValue`/`Value` trees recurse per nesting level with no depth limit (no
  `recursion_limit`/depth check anywhere in the crate). A frame of deeply
  nested arrays/maps can exhaust the task's stack during decode. Threat
  requires an authenticated (or, per 3.1, MITM-substituted) server, and the
  exposure is symmetric server-side — but the client is the "untrusted-input
  handler" for everything the peer sends.
- Failure scenario: hostile peer sends a ~16 MiB frame of nested seqs → stack
  overflow → client process abort (DoS).
- Suggested fix: verify rmp-serde's current recursion behavior for the pinned
  version; if unguarded, add a pre-decode structural depth check (e.g.
  lightweight scanner rejecting nesting beyond a sane bound) or decode in a
  `spawn_blocking` with a large stack as mitigation.

### 3.5 — low — No negative-path coverage for pin verification or resume anywhere in the crate's suite
- File:line: all tests use `accept_new_host: true, trusted_pin: None`
  (`tests/smoke.rs:97-98`, `src/tests/cursor_stream_tests.rs:99-100`,
  `src/tests/interner_cache_tests.rs:98-99`,
  `src/tests/ambient_sync_tests.rs:101-102`,
  `src/tests/v2_passthrough_tests.rs:94-95`, all `tests/*_e2e.rs`);
  `Client::resume` has only serialization-level tests in-crate
  (`src/tests/resume_wire_tests.rs`) and a single happy-path e2e outside the
  crate (`shamir-server/tests/duplex_e2e.rs:239`).
- Issue: the SDK's own wiring of the security-critical knobs is untested: no
  test connects with `trusted_pin: Some(..)` (happy path), no test asserts a
  pin mismatch is refused with `ServerIdentityChanged`, no test asserts
  `trusted_pin: None + accept_new_host: false` fails closed, and the
  `pin_capture` callback + `.expect` (see 6.3) and the
  `ResumeOptions::pinned_hash` plumbing (which 3.1 shows is a no-op check)
  are never exercised.
- Suggested fix: add e2e tests: (a) pinned connect against the pinned server
  succeeds; (b) pinned connect after server key rotation (or against a second
  server instance) fails with the identity-changed error; (c) no pin + TOFU
  refused fails closed; (d) an in-crate resume e2e (happy path + a wrong-pin
  variant once 3.1's fix lands).

## 4. performance-hotpath

### 4.1 — high — `roundtrip` clones the whole serialized request per call and ignores the zero-copy envelope built for exactly this path
- File:line: `crates/shamir-client/src/client.rs:975-989` (clone at :982 as
  audited; verified current: `REQ_BUF` `:1249-1257`, `buf.clone()` `:1256`,
  owning `RequestEnvelope::new` `:1260`).
- Issue: The T-cl-1 thread-local `REQ_BUF` serializes `DbRequest` into a
  reused buffer, then immediately does `buf.clone()` — a fresh heap allocation
  + full-payload memcpy on **every** request, defeating the stated purpose of
  the reuse buffer. The clone is then moved into the owning
  `RequestEnvelope::new`, which additionally allocates a 32-byte `session_id`
  Vec per request (`session_id.to_vec()` in
  `shamir-connect/src/common/envelope.rs:43`), and `to_msgpack()` copies the
  request bytes *again* as the embedded `req` field. Net: 3 allocations + 2
  full-payload copies per request in the hottest client path. Meanwhile
  `RequestEnvelopeRef<'a>` (`shamir-connect/src/common/envelope.rs:92-110`)
  exists specifically as "zero-copy borrowed envelope for the client encode
  path … tight client-side request loops where the same `[u8; 32]` session id
  is sent on every request" — it is benched in
  `shamir-connect/benches/hot_paths.rs` but never used by this crate.
- Failure scenario: Pipelined/multi-MB batches pay 2× row-byte memcpy + 3
  allocs per request on the caller's task; throughput-bound clients (napi
  binding, repl `Pull` loops) eat constant-factor overhead on every op.
- Suggested fix: Move the `next_request_id.fetch_add` above the encode;
  serialize the request into the TLS `REQ_BUF`, then synchronously build
  `RequestEnvelopeRef { session_id: &self.session_id, request_id: Some(rid),
  req: &buf }` and call its `to_msgpack()` (single output allocation, zero
  copy of `req`/`sid`); drop the `REQ_BUF` borrow before the first `.await`
  (`self.write.lock().await`) exactly as the current comment already promises.
  Delete the `buf.clone()` and the owning `RequestEnvelope` use. (Fold 3.3's
  zeroization into this rewrite.)

### 4.2 — medium — v2 read path: per-row `ByteBuf` clone and per-row×repo `get_or_create` key allocations in de-intern
- File:line: `crates/shamir-client/src/interner_cache_ops.rs:652-661` (clone
  at :654; verified `deintern_query_result` at `:645+`), `:728-734`
  (`get_or_create` per record at :729; verified `try_deintern_repos` at
  `:710+`), `:207` (key construction).
- Issue: `deintern_query_result` clones every `IdBytes` row (`bytes.clone()`)
  solely so the borrow doesn't cross the de-intern `.await` — a full row-byte
  memcpy per result row. Then `try_deintern_repos` calls
  `client.interner_cache().get_or_create(db, repo)` **inside the per-record
  loop**, and `get_or_create` unconditionally builds
  `(db.to_string(), repo.to_string())` even on the fast-path hit — 2 String
  allocations + an `scc` read per row per repo.
- Failure scenario: A 10k-row `ResultEncoding::Id` response costs ~10k row-byte
  clones + ~20k String allocations + 10k scc lookups that resolve to the same
  `Arc<FieldMap>` every time — all in the read hot path of the flagship v2
  flow.
- Suggested fix: (a) Resolve `Vec<Arc<FieldMap>>` once per `QueryResult`
  (before the record loop) and pass the `Arc`s down (this also sets up 1.5's
  per-repo mapping fix); (b) take ownership of the row without cloning via
  `std::mem::replace(record, QueryRecord::IdBytes(ByteBuf::new()))` (cheap
  placeholder; the response is discarded on any `?` error anyway); (c)
  optionally add a borrowed-key `lookup(db, repo)` fast path so the common hit
  doesn't allocate the tuple key.

### 4.3 — medium — `early_buffer` key cardinality is server-controlled and unbounded for the life of the connection
- File:line: `crates/shamir-client/src/client.rs:260-269` (current
  `:348-359`); `crates/shamir-client/src/subscription.rs:30-33` (verified:
  `EARLY_BUFFER_CAP` bounds per-sub Vec only; module doc at `:11-15` promises
  "a stalled consumer can no longer balloon client memory unboundedly").
- **Dedup: also flagged by concurrency-lockfree #5 (low) and security-crypto
  #4 (low)** — one root cause, three lenses.
- Issue: Per-sub buffering is capped at 256 envelopes, but the early-buffer
  map gains one entry per *distinct unknown `sub_id`*, and the key comes
  straight off the wire; entries are never evicted unless `subscribe_push` is
  called for that exact id. A buggy/malicious (authenticated) server — or
  sub-id confusion after reconnect — balloons client memory within a single
  session: up to 256 envelopes × up to `MAX_FRAME_SIZE_DEFAULT` (16 MiB) each,
  per attacker-chosen sub id. The registered-subscription path is properly
  bounded by the mpsc cap; only the pre-registration buffer leaks growth. The
  `tracing::debug!` "early buffer full" line only fires *after* 256 pushes
  accumulate for the same id.
- Failure scenario: server pushes frames with garbage/rotated sub ids for a
  long-lived connection → unbounded `TFxMap` growth, client RSS balloons
  without any consumer-side cap firing, until OOM.
- Suggested fix: bound the whole structure, not just each entry: a global
  buffered-envelope budget (e.g. `AtomicUsize` mirror, per pillar 3) with
  drop-oldest/stop-buffering once exceeded, or cap distinct buffered sub ids
  (e.g. 64) with drop-and-`warn!` beyond it; and clear the buffer when the
  reader task exits (concurrency lens's addition — pairs with 1.1's fix).

### 4.4 — medium — Orphaned `pending` entries when the caller's future is cancelled (no drop guard)
- File:line: `crates/shamir-client/src/client.rs:993-998` (current
  `:1268-1272`), `:178-203` (cleanup only on the crate's *own* timeout;
  verified current `:267-292`).
- **Dedup: also flagged by error-handling-lifecycle #8 (low)**.
- Issue: `roundtrip` inserts the oneshot sender into `pending` and only
  removes it on: response arrival, send failure, its own `request_timeout`
  elapse, or connection close. If the *caller* drops the future mid-await (an
  outer `tokio::time::timeout`, `select!` racing, task abort), the `(rid, tx)`
  entry stays in the map. It is reclaimed only if the server eventually
  answers that rid or the connection dies — otherwise one map entry + one
  oneshot per cancelled request accumulates for the connection's lifetime.
- Failure scenario: A long-lived connection whose users wrap calls in their
  own timeouts against a server that occasionally drops/never answers a rid
  grows `pending` monotonically — precisely the "unbounded growth" class the
  theme targets; the demux drain at close then walks and fails every dead
  sender, and the dead-entry drift masks genuine in-flight state when
  debugging.
- Suggested fix: Return a small RAII guard from `roundtrip`'s registration
  site holding `(PendingMap, rid)` whose `Drop` removes the rid (mirroring
  `await_pending_response`'s timeout-path cleanup), so every exit path —
  cancellation included — leaves no orphan.

### 4.5 — medium — Ambient interner sync runs on every `execute` regardless of server version or cache state
- File:line: `crates/shamir-client/src/client.rs:779-784` (`distinct_repos`
  walk per execute; verified current `:1053-1058` — the
  `interner_epochs.is_empty()` gate is always true for builder-built
  batches); `crates/shamir-client/src/interner_cache_ops.rs:357-384`
  (collect + touch before the `>= 2` check at :389).
- Issue: On **every** `execute`, the client walks all queries via
  `distinct_repos` — O(ops) with an unconditional `tr.repo.clone()` per op
  (`shamir-query-types/src/batch/query_entry.rs:105`) — then calls
  `get_or_create` per repo (2 more String allocs) and inserts epochs into the
  request. None of this can matter on a pre-v2 server
  (`server_query_version() < 2` never id-key encodes/de-interns), yet it is
  sent anyway. Similarly `execute_with_touch` performs the full field-name
  collection and may fire real `interner_touch` **roundtrips** before the
  version check — wasted wire trips on v1, plus `all_repos`'s
  `tr.repo.clone()` per op.
- Failure scenario: A v1-server workload (or any workload against a repo with
  a cold/irrelevant cache) pays a hidden per-request O(ops) allocation + walk,
  plus extra roundtrips in `execute_with_touch`, for machinery whose results
  are never consumed.
- Suggested fix: Gate the epoch advertisement (client.rs:779 as audited) and
  the pre-touch/collection phase (interner_cache_ops.rs:357-384) on
  `self.server_query_version() >= 2`; short-circuit when the registry holds no
  map for `db`. Combined with 4.2's hoisted FieldMaps, the per-request ambient
  cost on warm v2 paths drops to a few atomics.

### 4.6 — low — `encode_record_idmsgpack` pays an avoidable full-record copy per INSERT (`Bytes::to_vec`)
- File:line: `crates/shamir-client/src/interner_cache_ops.rs:611-613`
  (verified `encode_record_idmsgpack` at `:599+`).
- Issue: `query_value_to_storage_bytes` already returns an owned `Bytes`
  (zero-copy from the internal `Vec`), but the client calls `.to_vec()` — an
  allocation + full-record memcpy per record — before wrapping in `ByteBuf`.
  The sibling `query_value_to_storage_bytes_into` scratch variant exists
  precisely because this "+1 alloc + memcpy per row" pattern caused a measured
  regression (see its doc in
  `shamir-types/src/codecs/interned/messagepack.rs:878-889`).
- Failure scenario: Large v2 INSERT batches copy every encoded record one
  extra time on the write hot path.
- Suggested fix: Use `bytes.into_vec()` (zero-copy when the `Bytes` is unique,
  which it is here) — or reuse one scratch buffer per batch via the `_into`
  variant and push `ByteBuf::from(scratch-copy)` only where ownership is
  required.

### 4.7 — low — `collect_field_names` clones every field-name key of every record before dedup
- File:line: `crates/shamir-client/src/interner_cache_ops.rs:468-480`
  (`k.clone()` per key; verified `collect_map_keys`/`collect_field_names` at
  `:444-480`), `:377-380` (sort/dedup after the fact).
- Issue: For each write op, every map key (recursively, including nested
  maps/lists) is cloned into a `Vec<String>` — including duplicates across
  records of the same batch — then sorted and deduplicated, and
  `touch_fields`/`missing_names` re-walks and clones only the genuinely
  missing ones anyway. The eager full-clone pass is redundant allocation
  proportional to total record keys per request. (Distinct defect from 1.9's
  missing `Value::Set` arm in the same walk.)
- Failure scenario: A 1k-record batch with 20 fields each allocates and
  discards ~20k Strings per `execute_with_touch` call.
- Suggested fix: Collect `&str` references (the batch outlives the touch loop)
  into a `TFxSet<&str>` per repo, and clone only the deduped unknown set — one
  allocation per *distinct unknown* field instead of per key occurrence.

### 4.8 — nit — Perf claims are unmeasured: no benches, no allocation-behaviour tests
- File:line: `crates/shamir-client/Cargo.toml` (no `[[bench]]`);
  `crates/shamir-client/src/tests/` (no allocation/growth assertions).
- Issue: The crate documents deliberate hot-path optimizations ("T-tcp-1"
  buffer reuse at client.rs:218-223, "T-cl-1" thread-local encode buffer at
  :970-977, the per-yield mutex-skip guard in cursor_stream.rs:237-244) but
  has no bench (workspace convention: `bench_scale_tool::Harness`) and no test
  observing allocation counts or map growth — so regressions like 4.1's clone
  (which silently nullifies T-cl-1) or 4.3/4.4's growth vectors are invisible
  to the suite. Behavioural coverage itself is good: demux
  ordering/garbage/EOF-drain, timeout cleanup, early-buffer full-drop, v2
  passthrough + refresh, ambient delta, cursor close/cancel are all tested.
- Suggested fix: Add one `benches/roundtrip.rs` (request encode + demux
  decode, per CLAUDE.md bench conventions; bench only with the isolated
  `CARGO_TARGET_DIR` per workspace rules) and a unit test asserting
  `pending`/early-buffer invariants after cancellation/misbehaving-server
  scenarios — that alone would have caught 4.1, 4.3, and 4.4.

## 5. api-wire-protocol

### 5.1 — high — `query_version: 2` stamped unconditionally — the `server_query_version` negotiation is parsed but never applied to the request version
- File:line: `crates/shamir-client/src/client.rs:786-790` (`Client::execute`;
  verified current `:1060-1064` — `query_version:
  CURRENT_QUERY_LANG_VERSION` at `:1061`); also
  `crates/shamir-client/src/cursor_stream.rs:126` (verified: `create_cursor`
  stamps `CURRENT_QUERY_LANG_VERSION`); contrast `client.rs:389` as audited
  where `server_query_version() >= 2` gates only the id-keyed encoding; the
  getter's own doc (`client.rs:980-989`, verified) says "emit v2 protocol only
  when `server_query_version() >= 2`".
- Issue: The client reads `auth_ok.server_query_version` /
  `resume_ok.server_query_version`, stores it, documents it, and gates the v2
  id-keyed write path on it — but every `DbRequest::Execute` and
  `DbRequest::CreateCursor` still carries `query_version:
  CURRENT_QUERY_LANG_VERSION` (2). The server
  (`shamir-server/src/db_handler/handler.rs:484`, `cursor_handlers.rs:1156` as
  audited) rejects unknown versions with `unsupported_query_version` before
  any DB work, and a pre-v2 server build's `SUPPORTED_QUERY_LANG_VERSIONS` is
  `[1]` only (`shamir-server/src/version.rs`).
- Failure scenario: Current client connects to an older deployed server:
  `server_query_version == 0` is correctly detected, `execute_with_touch`
  takes its "v1 path: send batch unchanged" branch — and then every request
  still fails with `ClientError::Db { code: "unsupported_query_version" }`,
  because the version stamp itself is the v2 opt-in the protocol docs say to
  gate. The entire graceful-degradation ladder (`#[serde(default)]` fields,
  "v1 path: send batch unchanged", `ResultEncoding::Name` fallback) is dead
  code against the exact server generation it was built for.
- Suggested fix: Stamp `query_version = min(CURRENT_QUERY_LANG_VERSION,
  server_query_version.max(1))` in `Client::execute` (and pass an explicit
  version to `create_cursor_with_version` from `CursorStream`, or overload
  `builder::cursor::create_cursor` with the client's negotiated version). Add
  a regression test connecting the current client to a
  `SUPPORTED_QUERY_LANG_VERSIONS = [1]`-only stub.

### 5.2 — medium — Handshake/resume frames are positional (array) msgpack while everything post-handshake is named-map — order is load-bearing, enforced only by duplicated struct definitions
- File:line: `crates/shamir-client/src/wire_frames.rs:13-86` (verified
  unchanged); encodes at `crates/shamir-client/src/client.rs:415, 464, 621`
  (`rmp_serde::to_vec`, positional) vs `client.rs:981`
  (`encode::write_named`) for `DbRequest` (audited); server-side mirror
  `shamir-server/src/connection/wire.rs:35-92` (whose comments warn
  "positional msgpack — omitting a field shifts array indices").
- Issue: `WireAuthInit`/`WireChallenge`/`WireClientProof`/`WireResumeInit`/
  `WireAuthOk`/`WireResumeOk` serialize positionally; all post-handshake
  traffic uses named map encoding. Positional correctness rests on field order
  staying identical between two independently-maintained struct definitions
  (client `wire_frames.rs`, server `connection/wire.rs`); nothing in
  `wire_frames.rs` carries the server side's "append new fields as trailing
  `#[serde(default)]`" warning. Additionally, the resume path has no version
  field at all (`WireResumeInit` lacks the `version: u8` that `WireAuthInit`
  carries at `:19` — verified), so a future resume-wire shape change has no
  negotiation axis.
- Failure scenario: A developer inserts a field mid-struct in one of the two
  mirrored definitions (or in the TS/napi clients, which must replicate exact
  positional order by hand). Decodes fail at handshake with an opaque
  rmp-serde error — or, with a trailing-but-optional field added on one side
  only, silently misparse.
- Suggested fix: Either switch handshake frames to `to_vec_named` (rmp-serde's
  `from_slice` already accepts both shapes on decode, so this is a
  one-sided-encode change — coordinate with the server), or at minimum copy
  the server's positional-compat warning into `wire_frames.rs` and add a
  version field to the resume frames for future evolution.

### 5.3 — medium — `ResumeOptions` has no `connect_timeout`/`request_timeout` — resumed clients silently revert to unbounded waits
- File:line: `crates/shamir-client/src/client.rs:596-598` (comment:
  "ResumeOptions carries no timeout knobs"; verified current `:870-872`),
  `:675` (`request_timeout: None`; verified current `:949`),
  `:95-105` (`ResumeOptions` struct; verified current `:99-108` — no timeout
  fields).
- **Dedup: also flagged by error-handling-lifecycle #4** (rated medium there).
- Issue: `ConnectOptions` grew `connect_timeout` and `request_timeout` (task
  #520, verified at `client.rs:85, 92`), but `ResumeOptions` was not given the
  knobs, so a client built via `resume()` always runs with unbounded connect
  *and* per-request waits — including a server that accepts the resumption and
  then never answers. The comment documents the choice, but the asymmetry is a
  trap: a caller who carefully bounded its initial connection gets an
  unbounded client after ticket resumption, with no compile-time or run-time
  signal. The napi binding (see shamir-client-node SUMMARY 3.1) hard-codes
  `None` too, so JS callers cannot opt out of this class at any layer.
- Failure scenario: An app that hardened its primary connections with
  `request_timeout = Some(5s)` resumes from a ticket (e.g. reconnect after
  network blip) and hangs forever on the first request against a wedged
  server; the fix for #520 silently does not apply.
- Suggested fix: Add `connect_timeout: Option<Duration>` and
  `request_timeout: Option<Duration>` to `ResumeOptions`, threading them
  exactly as `connect()` does.

### 5.4 — low — Envelope-level server errors are stringly-typed into `ClientError::Protocol` — spec §14 codes are not machine-readable
- File:line: `crates/shamir-client/src/client.rs:283-289` (as audited;
  verified current `:370-377` — `ClientError::Protocol(format!("server error
  envelope: {error}"))`); contrast the structured `ClientError::Db { code,
  message }` used for `DbResponse::Error` at `client.rs:1019-1024` as audited
  (current `:1293-1298`).
- Issue: `ErrorEnvelope.error` carries spec §14 codes (`session_expired`,
  `session_invalidated`, `authentication_failed`), which are exactly the
  events a client must react to programmatically (drop the client, re-auth,
  refresh ticket). The demux flattens them into an unstructured
  `Protocol(String)`. (Same stringly-typed-error theme as 6.1 — different
  sites, complementary fixes.)
- Failure scenario: A caller cannot implement "on `session_expired`, resume
  with the ticket" without `format!`-string matching against the error text,
  which breaks the moment the message wording changes.
- Suggested fix: Add a `ClientError::Session { code: String }` (or reuse
  `Db { code, message: String::new() }`) for envelope errors, matching on the
  known code vocabulary.

### 5.5 — low — Dead public API: `ClientError::RequestIdMismatch` is never constructed anywhere in the workspace
- File:line: `crates/shamir-client/src/error.rs:27-29` (verified unchanged).
- **Dedup: also flagged by error-handling-lifecycle #12 (nit) and
  security-crypto nits (nit)** — counted once.
- Issue: The variant documents "Server returned a request_id that doesn't
  match what we sent", but the demux routes purely by `rid` lookup
  (`client.rs:300-315` as audited; verified current `:389-404`); a
  mismatched/unknown rid is logged and dropped, never turned into this error.
  No caller in the workspace constructs it.
- Failure scenario: API consumers write `matches!(err,
  ClientError::RequestIdMismatch { .. })` arms that are unreachable dead code;
  the enum implies a demux behavior that does not exist (rid-correlation
  errors never surface — they are silently dropped, per
  `demux_late_response_for_unknown_rid_is_dropped`).
- Suggested fix: Remove the variant (the honest option), or wire it up if
  response-side rid validation is ever added.

### 5.6 — low — Frame demux is shape-sniffing with no discriminator — fragile routing plus up to three decode attempts per frame
- File:line: `crates/shamir-client/src/client.rs:121-135` (`decode_frame`:
  try `ResponseEnvelope`, then `ErrorEnvelope`; verified current `:194-207`,
  doc-confirmed strategy), `236-279` (fall through to `PushEnvelope`; current
  `:325-368`).
- **Dedup: performance-hotpath #9 (low) — "every non-response/non-error frame
  (i.e. every push) is deserialization-attempted three times; each failed
  attempt partially decodes and allocates (e.g. the 32-byte `sid` Vec) before
  erroring" — is the same discriminator-less design counted once here.** (The
  hang-on-undecodable-frame consequence is tracked separately as 6.5.)
- Issue: Incoming frames have no type tag; the reader identifies them by
  *trying* each serde shape in order. It works today only because `res` /
  `error` / `push`+`sub`+`seq` field names are disjoint. Any future
  server→client envelope that happens to contain a bytes field named `res`
  (streaming chunks, gossip frames, …) silently decodes as a regular response
  and is misrouted to a pending oneshot (or dropped as "frame without rid"),
  with no error anywhere. Push-heavy connections additionally burn 2× wasted
  parse work per frame; garbage frames pay 3×.
- Failure scenario: A v3 server adds a `progress` envelope `{ rid, res: bytes,
  pct }`; every such frame is demuxed as a `ResponseEnvelope` for that rid,
  corrupting the in-flight request's payload — decode then fails in
  `roundtrip` with an unrelated rmp-serde error.
- Suggested fix: Prepend a one-byte envelope kind tag (or a mandatory `t`
  string field) to every server→client frame and switch on it; until the wire
  format changes, at least reorder/document the sniff chain and add a test
  asserting a new-envelope-shaped frame is dropped, not misrouted; a cheap
  first-key peek via `rmp` decode primitives can route directly to
  Response/Error/Push without full deserialization.

### 5.7 — nit — Ambient interner-epoch advertisement is all-or-nothing per batch
- File:line: `crates/shamir-client/src/client.rs:779-784` (verified current
  `:1053-1058`).
- Issue: `Client::execute` populates `interner_epochs` for every distinct repo
  only when the caller left the map entirely empty. A caller that pre-fills
  one repo's epoch (e.g. because it just ran `refresh_repo` for a hot repo)
  silently disables ambient delta advertisement for every *other* repo in the
  same batch. (Semantics — distinct from 4.5's wasted-work finding at the same
  site.)
- Suggested fix: Insert per-repo epochs only for repos not already present in
  `batch.interner_epochs` (per-repo `entry` API instead of the whole-map
  guard).

### 5.8 — nit — `ConnectOptions` has no `Default` impl — every call site must spell out 8 fields
- File:line: `crates/shamir-client/src/client.rs:54-89` (verified current
  `:58-92`; 8 fields).
- Issue: `addr`, `server_name`, `username`, `password` genuinely have no
  default, but `accept_new_host`/`trusted_pin`/`connect_timeout`/
  `request_timeout` do (documented in-line). The absence of `Default` is what
  allowed the lib.rs example drift (7.1) and makes the napi/TS bindings'
  option plumbing noisier than it needs to be.
- Suggested fix: `#[derive(Default)]` (with `accept_new_host: true` needing a
  manual impl) plus struct-update syntax `ConnectOptions { addr, server_name,
  username, password, ..Default::default() }`.

## 6. error-handling-lifecycle

### 6.1 — medium — Stringly-typed `Handshake`/`Tls`/`Transport`/`Protocol` variants erase typed sources
- File:line: `crates/shamir-client/src/error.rs:9-21` (verified: `Tls(String)`,
  `Transport(String)`, `Handshake(String)`, `Protocol(String)`); conversion
  sites `client.rs:388, 403, 417, 422, 455, 466, 471, 537` as audited
  (verified current: e.g. `:812`
  `.map_err(|e| ClientError::Handshake(e.to_string()))`).
- Issue: `shamir-connect` produces a typed error enum (`Error::
  ServerAuthFailed`, `Error::ServerIdentityChanged`, `Error::
  ServerSignatureInvalid` — see
  `crates/shamir-connect/src/client/handshake.rs:261-293`), but the SDK
  flattens it via `.map_err(|e| ClientError::Handshake(e.to_string()))`. Per
  CLAUDE.md's thiserror discipline ("`#[from]` where natural"), these should
  preserve the source: programmatic handling (e.g. surfacing "server identity
  changed — possible MITM" differently from "bad password") currently requires
  matching on formatted English strings.
- Failure scenario: an SDK consumer writing `matches!(err,
  ClientError::Handshake(_))` + `contains("ServerIdentityChanged")` breaks on
  any wording change; no `source()` chain for diagnostics.
- Suggested fix: add `#[error("handshake: {0}")] Handshake(#[from]
  shamir_connect::client::Error)` (and analogous typed variants or
  `#[source]` fields for TLS/transport), keeping the `String` forms only for
  genuinely unstructured text. (The napi binding's error taxonomy finding —
  shamir-client-node SUMMARY 6.2 — compounds one layer up; fixing this
  unblocks the binding's stable `.code` story.)

### 6.2 — low — Push-subscription lifecycle and closed-client paths have no error-path tests
- File:line: `crates/shamir-client/src/tests/` (absent), findings 1.1/2.1's
  paths.
- Issue: the demux tests are otherwise exemplary (EOF drain, garbage frames,
  error envelopes, late responses, bounded channel, handle-drop registry
  cleanup), but there is no test for the post-connection-loss subscription
  lifecycle (currently a hang — untestable without first fixing 1.1), nor one
  asserting `roundtrip` returns `ConnectionClosed` when `closed` is already
  set (`client.rs:966-968` as audited), nor for `subscribe_push` on a closed
  client. (Overlaps correctness #11 / 1.10 and concurrency #6 / 2.3; kept
  separate — the closed-flag-guard bundle is unique here.)
- Failure scenario: regressions in 1.1/2.1 land silently; the closed-flag
  guard at `roundtrip` entry could be removed without any test failing.
- Suggested fix: after fixing 1.1/2.1, add: (a) reader-exit → `next()` yields
  `None`; (b) `roundtrip` on a closed client → `Err(ConnectionClosed)`;
  (c) `subscribe_push` on a closed client → closed handle.

### 6.3 — low — `.expect()` in library code rests on an unenforced cross-crate invariant
- File:line: `crates/shamir-client/src/client.rs:539-542` (verified current
  `:814-817` — `.expect("either trusted_pin pre-set or TOFU callback
  fired")`); contract in
  `crates/shamir-connect/src/client/handshake.rs:149-151, 266-276`.
- Issue: the `.expect` is currently unreachable: `build()` rejects
  `pinned_hash == None && !accept_new_host`, and `process_auth_ok` fires the
  callback exactly when `pinned_hash == None`, before any later `Err`. But the
  invariant lives in another crate's private control flow and is not
  statically enforced; a future handshake mode returning `Ok` without invoking
  the callback would turn user input into a panic in this library. CLAUDE.md
  reserves panics for genuine programmer-bug invariants. (Cited as the
  upstream panic site by the napi binding's `catch_unwind` finding —
  shamir-client-node SUMMARY 3.2.)
- Suggested fix: `.ok_or_else(|| ClientError::Handshake("pin capture missing
  after auth_ok".into()))?` — same semantics, no panic path, robust to
  upstream drift.

### 6.4 — low — `touch_fields` silently omits names the server failed to map
- File:line: `crates/shamir-client/src/interner_cache_ops.rs:260-264,
  280-283` (verified `touch_fields` at `:249+`).
- Issue: both return paths use `filter_map(|n| fm.id_of(n) ...)`: if the
  server's response omits a mapping for a name the client explicitly touched
  (a protocol violation), the missing name is dropped from the result with no
  error, and the failure resurfaces later as 1.4's misleading encode error.
  The early-return path (`unknown.is_empty()`) is fine by construction; the
  post-roundtrip path is not.
- Suggested fix: after merging, verify every input name resolves; return
  `ClientError::Protocol("interner_touch: server returned no mapping for
  '<name>'")` otherwise. Add an error-path test.

### 6.5 — low — Undecodable response frames are dropped at `debug` level; default-unbounded waiters hang
- File:line: `crates/shamir-client/src/client.rs:121-135` (`decode_frame` →
  `None`; current `:194+`), `:236-278` (log-and-`continue`; verified current
  `:360-367`).
- Issue: a frame that decodes as neither `ResponseEnvelope` nor
  `ErrorEnvelope` nor `PushEnvelope` is dropped with a `tracing::debug!`
  (demux_tests proves alignment survives). Defensible for frame alignment —
  but if that frame was the response to an outstanding rid, its waiter hangs
  forever under the default `request_timeout: None`, and the only trace is a
  debug-level line. (Demux-shape fragility itself is 5.6.)
- Suggested fix: at minimum raise the log to `warn` with the frame length;
  consider a counter that escalates to a fatal protocol error (mark closed +
  drain with `ClientError::Protocol`) after repeated decode failures on a live
  connection.

### 6.6 — nit — `ResumeOptions::ticket` and its wire copy are not `Zeroizing`
- File:line: `crates/shamir-client/src/client.rs:100-101, 616-620` as audited
  (verified current: `ResumeOptions.ticket: Vec<u8>` at `:105`, moved into
  `WireResumeInit` at `:891`); contrast `:347, 564` as audited
  (`resumption_ticket: Option<Zeroizing<Vec<u8>>>`, verified `:436`).
- Issue: the crate treats the resumption ticket as a secret everywhere except
  the `resume()` input path: `ResumeOptions::ticket: Vec<u8>` is moved into
  `WireResumeInit` and dropped un-wiped (the serialized copy in the request
  buffer likewise), while the Client's own copy is `Zeroizing`.
  Same-lifecycle inconsistency for the same credential. (`create_scram_user`'s
  reliance on `SecretString`'s conditional `crypto`-feature wipe is similarly
  feature-gated but documented.)
- Suggested fix: `pub ticket: Zeroizing<Vec<u8>>` in `ResumeOptions`
  (napi/TS wrappers already copy bytes, so the breaking change is contained).

## 7. style-claude-md

### 7.1 — low — Crate-level doc example no longer compiles against `ConnectOptions`; drift is invisible because doctests are disabled
- File:line: `crates/shamir-client/src/lib.rs:10-17` (verified: still 6 fields
  in the example) vs `crates/shamir-client/src/client.rs:54-89` (verified
  current `:58-92`, 8 fields), `crates/shamir-client/Cargo.toml:48-51`.
- **Dedup: also flagged by api-wire-protocol #8 (low)** — counted once.
- Issue: The `//!` illustration constructs `ConnectOptions` with six fields
  (`addr`, `server_name`, `username`, `password`, `accept_new_host`,
  `trusted_pin`), but the struct now has eight — `connect_timeout` and
  `request_timeout` were added and are absent from the example. `Cargo.toml`
  sets `[lib] doctest = false` ("Doctests are banned project-wide"), so the
  fenced `no_run` example is never type-checked and the drift is silent. The
  `Cargo.toml` comment explicitly sanctions keeping examples "as
  illustration", so the example itself is conforming — but an illustration
  that fails `E0063` if ever pasted defeats its purpose.
- Failure scenario: A user copies the documented connect snippet verbatim and
  gets a missing-field compile error; nobody on the maintenance side is ever
  alerted, because the banned-doctest setup guarantees the block is never
  built.
- Suggested fix: Add `connect_timeout: None, request_timeout: None` to the
  example's struct literal (or split the illustration so it only shows the
  stable core fields with a prose note that two optional timeout knobs exist);
  5.8's `Default` impl prevents recurrence.

### 7.2 — low — `use rand::RngCore;` inside the body of `Client::resume`
- File:line: `crates/shamir-client/src/client.rs:608-611` (verified current
  `:882-885`).
- Issue: CLAUDE.md "📦 Imports at the top" requires every `use` to live in the
  file header, with only three documented exceptions. None applies here: there
  is no `RngCore` name collision in scope (nothing else from `rand` is
  imported, and `rand` appears nowhere else in the file), there is no one-line
  comment stating a collision, and the block is not macro-generated or
  `cfg`-gated. The scoped `use` sits in an artificial `{}` block purely to
  limit trait scope.
- Failure scenario: None functional — pure style-conformance drift that the
  documented rule is meant to prevent (hidden mid-body dependency edges).
- Suggested fix: Hoist `use rand::RngCore;` to the import header next to the
  other external-crate imports and delete the enclosing braces.

### 7.3 — nit — Mid-body `use` statements in three `src/tests/` files
- File:line: `crates/shamir-client/src/tests/batch_has_refs_tests.rs:18`
  (`use shamir_query_types::read::ReadQuery;` inside helper `read_op`);
  `crates/shamir-client/src/tests/demux_tests.rs:408` (`use
  crate::subscription::SubscriptionHandle;` inside
  `subscription_handle_drop_removes_from_registry`);
  `crates/shamir-client/src/tests/wire_version_tests.rs:137` (`use
  std::sync::atomic::{AtomicU8, Ordering};` inside
  `atomic_u8_plumbing_stores_and_reads_correctly`).
- Issue: Same "Imports at the top" rule as 7.2. The `use super::*;` exception
  covers inline `#[cfg(test)] mod tests` blocks, not files in a `tests/`
  directory (which this crate correctly uses instead — so these files are
  ordinary modules and must keep imports in their headers). None of the three
  has a collision or a collision comment; all three hoist trivially
  (`batch_has_refs_tests.rs` already imports from `shamir_query_types::read`,
  so `ReadQuery` just joins that group).
- Failure scenario: None functional; each is a small, mechanical violation of
  the documented header-import rule in test code.
- Suggested fix: Move all three imports to the top of their files and delete
  the surrounding braces (`wire_version_tests.rs`'s import can merge into a
  header group with the other `std` imports).

---

## Non-findings (checked and clean, carried forward for the record)

- `connect` fail-closed: `HandshakeBuilder::build` errors when neither pin nor
  `accept_new_host` is set — the SDK cannot silently skip server auth on the
  full connect path.
- KDF-DoS: server-supplied Argon2id params are double-capped *before*
  allocation (`validate_client_limits` + `validate_client_kdf_safe`, ceiling
  512 MiB / 16 passes) inside `process_challenge` — a MITM cannot OOM the
  client via challenge params.
- Comparison hygiene: all secret comparisons (server signature, pin) use
  `constant_time_eq`; the client only computes HMAC tags and never compares
  them — no client-side timing oracle. Resume nonce generation is
  CSPRNG-backed.
- Untrusted-input size bounds: all frame reads enforce
  `MAX_FRAME_SIZE_DEFAULT` (16 MiB); fixed-size wire fields are
  `try_into`-checked with protocol errors on mismatch — no panics on
  malformed input. No `unsafe` in this crate.
- Builder-only query construction: zero `serde_json`/`json!` anywhere; every
  request is built via `shamir_query_builder`; §9.4 "ids only from server
  responses / names never parsed as numbers" discipline is consistently
  documented and implemented.
- Structure: `lib.rs` and `src/tests/mod.rs` re-export/manifest-only; zero
  inline `#[cfg(test)]` blocks; one-file-one-export holds across the crate;
  test-coverage claims in module docs match the tests present.
- `interner_cache.rs` is fully pillar-compliant (lock-free `scc::HashMap` +
  `THasher`, CAS-max epoch, documented sanctioned `OnceCell` dump guard,
  O(N)-ack'd `len()`); no lock anywhere is held across an `.await` except the
  sanctioned `tokio::sync::Mutex` write-half guard.

## Cross-crate note: shared root causes with the shamir-client-node binding review

The `shamir-client-node/SUMMARY.md` exemplar reviews the napi binding wrapping
this crate; its findings connect to this crate's at the following roots
(worth co-scheduling):

- **Unbounded waits as the only behavior** — node 3.1 (no
  `connectTimeoutMs`/`requestTimeoutMs` surfaced) is the binding-side face of
  this crate's default `request_timeout: None` (2.1's hang becomes permanent)
  and of 5.3 (`ResumeOptions` has no knobs at all). Threading the knobs here
  and in the binding is one coherent fix.
- **The `.expect()` panic path** — node 3.2 (`#[napi(catch_unwind)]` +
  "upstream-fix the core `.expect()`") points at 6.3.
- **Error taxonomy erased twice** — node 6.2 (infra errors lose `.code`)
  explicitly cites this crate's stringly-typed flattening (6.1) as the
  compounding upstream cause.
- **The "wraps 1:1" claim** — `lib.rs:26-28` (verified) still advertises 1:1
  parity while node 5.2 documents the binding's missing surface (`resume`,
  `server_query_version`, …). If 3.1's resume-identity fix and 5.1's
  version-gating land here, the binding must thread both or the parity gap
  widens; `lib.rs`'s claim should be corrected either way.
- **Serialization at both layers** — node 2.1 (binding holds one
  `tokio::sync::Mutex` across every round trip, serializing the demuxed
  client) sits on top of 2.2 (this crate's demux hot paths themselves lock a
  global `std::sync::Mutex` per request/frame): two avoidable serialization
  points in the same call path.
- **Repl's second error channel** — node 1.3 (repl errors resolve as success
  in JS) exists because `Client::repl` returns `Ok(ReplResponse::Error)` by
  documented design (`client.rs:1164-1171`, verified) — unlike `execute`,
  which converts `DbResponse::Error` into `ClientError::Db`. No lens of this
  review flagged the core-side asymmetry as a defect (it is documented API),
  but the asymmetry is the root the binding must handle; noted for
  cross-crate awareness, not counted as a finding here.

## Finding counts

Raw lens-tagged total: **69** (matches the workspace SUMMARY.md per-crate row:
0 crit / 9 high / 19 med / 26 low / 15 nit — pre-dedup). After deduplicating
the 22 findings that share a root cause across lenses (15 dedup groups):
**47 distinct defects**.

| Severity | Lens-tagged findings | Distinct defects | Deduped finding numbers (dedup groups in one row count once; "+ lens #n" = folded duplicates) |
|---|---|---|---|
| critical | 0 | 0 | — |
| high | 9 | 7 | 1.1 (+err #2), 1.2, 2.1 (+err #1), 2.2 (+conc #3, perf #5), 3.1 (+corr #3, api #3, sec nit), 4.1, 5.1 |
| medium | 19 | 13 | 1.3 (+conc #4), 1.4 (+err #3), 1.5, 1.6 (+api #12), 3.2, 3.3, 4.2, 4.3 (+conc #5, sec #4), 4.4 (+err #8), 4.5, 5.2, 5.3 (+err #4), 6.1 |
| low | 26 | 18 | 1.7 (+sec nit, api #9, err #6), 1.8, 1.9, 1.10, 2.3, 3.4, 3.5, 4.6, 4.7, 5.4, 5.5 (+sec nit, err #12), 5.6 (+perf #9), 6.2, 6.3, 6.4, 6.5, 7.1 (+api #8), 7.2 |
| nit | 15 | 9 | 1.11, 1.12, 1.13 (+conc #8), 2.4, 4.8, 5.7, 5.8, 6.6, 7.3 |
| **total** | **69** | **47** | |

Deduplicated defect census: **0 critical, 7 high, 13 medium, 18 low, 9 nit =
47 distinct defects** (69 lens-tagged findings).

Severity-rating notes: 1.1 was tagged medium by the error-handling lens and
re-rated high here (unconditional hang; the workspace catalog and P0 carry it
as high-class); 2.2 merges a high + medium + medium tagging (the `PendingMap`
half drives the rating); 1.6/1.7/5.5 take the highest lens rating of their
group.

## Fix Plan

**P0 — before anything else ships from this crate**

1. **Close subscription channels on reader exit.** In `reader_task`'s exit
   path, clear `subscriptions` (and the early buffer) so every
   `SubscriptionHandle::next()` resolves `None`; make `subscribe_push`
   respect `closed`. Red test: reader EOF with a registered subscription
   delivers `None`. This is workspace P0 #8 and the unconditional half of the
   three-lens reader-exit headline. Closes **1.1** (+err #2) and unblocks
   **6.2**(a).
2. **Order `roundtrip` registration against the reader drain.** Lock-ordered
   `closed.store` + drain in `reader_task`; post-insert `closed` re-check +
   self-clean in `roundtrip`. Red test: insert-after-drain resolves
   `ConnectionClosed`. Closes **2.1** (+err #1) and unblocks **6.2**(b),
   **2.3**(b).
3. **Authenticate the server on resume.** Extend `WireResumeOk` with
   `server_pub_key` + Ed25519 `identity_sig` over
   `(client_nonce, session_id, expires_at_ns, TLS exporter)` and verify
   against `ResumeOptions::pinned_hash` (or, if by design, document
   carry-through-only and stop advertising `server_pub_key_pin()` as verified
   for resumed sessions). Closes **3.1** (+corr #3, api #3, exporter nit).
4. **Gate the `query_version` stamp by negotiation.** Stamp
   `min(CURRENT, max(server_query_version, 1))` in `execute` and thread the
   negotiated version into `create_cursor`; regression test against a
   `[1]`-only server stub. Closes **5.1** — without it the whole v1-fallback
   ladder is dead code.
5. **Fix `batch_has_refs`.** Scan `QueryEntry.when` and recurse
   `BatchOp::ForEach` (over + nested queries); Red tests for both shapes plus
   one e2e `execute_with_touch` when/for-each test. Closes **1.2** — silent
   Id-encoding corruption on v2.

**P1 — soon**

6. **Replace the demux hot-path mutexes with `scc::HashMap<_, _, THasher>`
   (or add the mandated per-site contention-model comments).** Closes **2.2**
   (+conc #3, perf #5); fold 2.1's re-check into `insert_sync`/`remove_sync`.
7. **Make `dump_repo` honest.** `get_or_try_init` (or success-gated flag),
   propagate the error, fix the doc/comment; tests for failure→unpopulated→
   retry-succeeds. Closes **1.4** (+err #3) and **6.4**'s misleading-error
   surfacing path gets a real error to surface.
8. **Add the timeout knobs to `ResumeOptions`** (threading as `connect`
   does). Closes **5.3** (+err #4); also the prerequisite for the napi
   binding's timeout story (node 3.1).
9. **RAII drop-guard for the pending entry** so caller cancellation leaves no
   orphan. Closes **4.4** (+err #8).
10. **Bound the early buffer globally** (total-envelope `AtomicUsize` budget
    or distinct-sub-id cap; drop+warn beyond) and clear it on reader exit.
    Closes **4.3** (+conc #5, sec #4).
11. **Typed error taxonomy**: `#[from] shamir_connect::client::Error` (and
    TLS/transport sources), plus a structured variant for envelope-level §14
    codes. Closes **6.1** and **5.4**; unblocks the napi `.code` story (node
    6.2).
12. **Zero-copy `roundtrip` encode** via `RequestEnvelopeRef` (delete
    `buf.clone()`), with `REQ_BUF`/`req_bytes`/`envelope_bytes` zeroization
    for secret-bearing requests. Closes **4.1** and **3.3**.
13. **Hoist per-`QueryResult` FieldMaps and kill per-row allocations** in the
    v2 read path; resolve each alias against its op's `table_ref().repo`
    (fixing the first-match mislabel at the same time); gate ambient sync on
    `server_query_version() >= 2`. Closes **4.2**, **1.5**, **4.5**.
14. **Testing bundle**: replace the vacuous AtomicU8 test; add negative-path
    pin/resume e2e (pinned happy, rotation-refusal, fail-closed, in-crate
    resume happy+wrong-pin); closed-client `roundtrip`/`subscribe_push` tests;
    concurrent dump stampede test. Closes **1.6** (+api #12), **3.5**,
    **6.2**, **2.3**(a), and the test halves of **1.10**.

**P2 — backlog**

15. Atomic subscription/early-buffer handoff (single-lock routing or `scc`
    `entry_sync`). Closes **1.3** (+conc #4).
16. `get_ddl_op_status` contract: match `Err(Db{code})` for `not_supported`
    reshaping (or `NotSupported` variant), delete the dead arm, fix comment,
    add branch tests. Closes **1.7** (+api #9, err #6, sec nit).
17. Identity-checked (or rejected-double) `subscribe_push` registration.
    Closes **1.8**.
18. `Value::Set` arm in `collect_map_keys`. Closes **1.9**.
19. Remaining correctness nits: deterministic connect-timeout test endpoint
    (**1.11**); set `closed` in `close()` or fix the `ConnectionClosed` doc
    (**1.12**); rid-wrap policy (**1.13** +conc #8); per-repo epoch
    advertisement (**5.7**).
20. Depth guard (or verified-bounded recursion + `spawn_blocking`) for
    peer-frame decode. Closes **3.4**.
21. Wire-protocol hardening: named handshake encoding or the copied
    positional-compat warning + resume version field (**5.2**); envelope kind
    tag / first-key demux discrimination (**5.6** +perf #9); warn+escalation
    for undecodable frames (**6.5**).
22. Remaining perf: `Bytes::into_vec` per INSERT (**4.6**); `&str`-based
    field-name collection (**4.7**); one `benches/roundtrip.rs` + growth
    invariant tests (**4.8**).
23. API hygiene: `Default` for `ConnectOptions` (**5.8**); remove dead
    `RequestIdMismatch` (**5.5** +err #12, sec nit); `Zeroizing` resume
    ticket (**6.6**); `.ok_or_else` for the pin-capture `.expect` (**6.3**);
    `touch_fields` post-merge verification (**6.4**).
24. Style sweep: fix the lib.rs example fields (or adopt `Default`) and the
    three mid-body test imports; hoist `use rand::RngCore`. Closes **7.1**
    (+api #8), **7.2**, **7.3**, and the remainder of **1.10**'s coverage gaps
    (close/Drop reader-abort liveness, `subscribe_push` flush-path test).

</details>
