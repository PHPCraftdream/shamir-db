<!-- revalidation:2026-10-08 cycle:independent-2 source:e3765c935fc71655ee1ec0160cf180607935d89b -->
# shamir-server — SUMMARY independent revalidation (cycle 2)

Frozen source snapshot: `e3765c935fc71655ee1ec0160cf180607935d89b`. Independently revalidated 2026-10-08 by a fresh XS module review and parent acceptance. No compiler, build, test, benchmark or reproduction was run. Source-fixed means the specified mechanism was already removed at this snapshot; this cycle implements no source fix.

This section is authoritative for current decisions. Existing IDs and allegation titles are retained for traceability; a refuted title is not a current assertion. Prior-cycle decisions were rechecked, not used as proof. The first cycle is available in repository history at this snapshot. The original historical body below remains preserved once and is superseded, including its counts, severities and recipes. [Workspace scope and status definitions](../SUMMARY.md#status-definitions).

Unrelated external edits began in dependency/toolchain/CI metadata during collection; additional Rust-source edits appeared afterward. They were left untouched. Evidence and decisions are tied to the frozen commit, not those later changes or an installed toolchain. “Current” below means current at that snapshot; no re-audit of the modified working tree is implied.

Original mechanisms were rechecked against unchanged source, not accepted from earlier labels. Three original refutations remain valid. New source-derived defects affect response-budget ownership, bootstrap TTL checking and graceful-shutdown quiescence. Cryptographic comparison is now supported by exact archives.

## Current claim decisions

| Claim decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 27 | 19 | 0 | 0 | 8 | 0 | 0 |

These are duplicate-inclusive report decisions, not a unique-bug census. Pure style or structural optimization does not establish a runtime incident; unverified impact remains provisional.

<a id="review-1-1"></a>

### Claim 1.1 — Interactive-tx path bypasses the read-only-replica gate entirely

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Authorized writes through TxExecute/TxCommit lack the Execute NodeMode check. Reachability requires an explicitly ReadOnly handler; the stock launcher does not select that mode.

Evidence: [crates/shamir-server/src/db_handler/tx_handlers.rs:168](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tx_handlers.rs#L168); [crates/shamir-server/src/db_handler/tx_handlers.rs:247](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tx_handlers.rs#L247); [crates/shamir-server/src/server/server_launcher.rs:461](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L461).

Grouping/duplicate: [correctness-tdd.md#1](correctness-tdd.md#review-1). This is not an additional independent defect.

<a id="review-1-2"></a>

### Claim 1.2 — Dead follower-loop registry entries block resubscription after a journal gap

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

Terminal task exits leave registered handles; active-state fencing errors and early resume strand them. A reconcile that observes resync_required removes the entry, so ordinary gap recovery is not necessarily permanently stuck.

Evidence: [crates/shamir-server/src/replication/supervisor.rs:213](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/supervisor.rs#L213); [crates/shamir-server/src/replication/supervisor.rs:231](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/supervisor.rs#L231); [crates/shamir-server/src/replication/supervisor.rs:286](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/supervisor.rs#L286).

Grouping/duplicate: [correctness-tdd.md#2](correctness-tdd.md#review-2). This is not an additional independent defect.

<a id="review-1-3"></a>

### Claim 1.3 — `close_all()` / `attach_handle()` race can leak a bridge task past connection teardown

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Teardown can remove a placeholder before a live handle is attached; missing-key update drops/detaches it. The existing test observes count, not cancellation.

Evidence: [crates/shamir-server/src/subscriptions/registry.rs:134](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/registry.rs#L134); [crates/shamir-server/src/connection/request_loop.rs:413](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L413); [crates/shamir-server/src/subscriptions/tests/registry_tests.rs:48](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/tests/registry_tests.rs#L48).

Grouping/duplicate: [correctness-tdd.md#3](correctness-tdd.md#review-3). This is not an additional independent defect.

<a id="review-1-4"></a>

### Claim 1.4 — Follower loop busy-loops on an unexpected `Hello` reply (missing backoff)

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

A malformed source's non-regressing Hello response immediately retries without delay. Production impact requires such a source and depends on transport latency.

Evidence: [crates/shamir-server/src/replication/follower_loop.rs:281](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L281); [crates/shamir-server/src/replication/follower_loop.rs:287](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L287).

Grouping/duplicate: [correctness-tdd.md#4](correctness-tdd.md#review-4). This is not an additional independent defect.

<a id="review-1-5"></a>

### Claim 1.5 — Log-mask override matching is a raw substring/prefix test, not a namespace-segment match

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Raw longest-prefix matching is the documented contract, with an explicit wal_compact assertion. Segment-awareness would change behavior.

Evidence: [crates/shamir-server/src/logging.rs:135](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/logging.rs#L135); [crates/shamir-server/src/logging/tests/log_mask_tests.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/logging/tests/log_mask_tests.rs#L64).

Grouping/duplicate: [correctness-tdd.md#7](correctness-tdd.md#review-7). This is not an additional independent defect.

<a id="review-1-6"></a>

### Claim 1.6 — `try_join_next()` swallows non-panic `JoinError`s silently

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The silent branch remains, but no live-connection individual-abort producer was found. This is latent diagnostics debt, not demonstrated current request loss.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L240); [crates/shamir-server/src/connection/request_loop.rs:415](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L415).

Grouping/duplicate: [correctness-tdd.md#5](correctness-tdd.md#review-5). This is not an additional independent defect.

<a id="review-1-7"></a>

### Claim 1.7 — `Scheduler::shutdown()` regression test has no timeout bound on the documented race it guards against

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The test lacks a local bound, but runner bounds exist and production receivers are synchronously subscribed before spawn. The remaining improvement is fast failure localization.

Evidence: [crates/shamir-server/tests/scheduler.rs:156](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/scheduler.rs#L156); [crates/shamir-server/src/scheduler.rs:126](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/scheduler.rs#L126); [.config/nextest.toml:49](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/.config/nextest.toml#L49).

Grouping/duplicate: [correctness-tdd.md#8](correctness-tdd.md#review-8). This is not an additional independent defect.

<a id="review-1-8"></a>

### Claim 1.8 — `safe_run`'s panic-survival contract is never actually exercised by a panicking tick

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The catcher exists but no registered scheduler fixture forces an unwind and observes the next tick. Removing catch_unwind is not caught by the existing counting mocks.

Evidence: [crates/shamir-server/src/scheduler.rs:314](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/scheduler.rs#L314); [crates/shamir-server/tests/scheduler.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/scheduler.rs#L44).

Grouping/duplicate: [correctness-tdd.md#9](correctness-tdd.md#review-9). This is not an additional independent defect.

<a id="review-1-9"></a>

### Claim 1.9 — `/readyz` is never observed returning "not ready" through the real boot path

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The HTTP assertion covers only post-launch 200. Direct observability spawn can expose the untested false→503 branch; stock launch has already bound data listeners.

Evidence: [crates/shamir-server/tests/observability_http.rs:149](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/observability_http.rs#L149); [crates/shamir-server/src/observability.rs:557](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/observability.rs#L557); [crates/shamir-server/src/server/server_launcher.rs:878](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L878).

Grouping/duplicate: [correctness-tdd.md#10](correctness-tdd.md#review-10). This is not an additional independent defect.

<a id="review-1-10"></a>

### Claim 1.10 — Vacuous dead-code discard of `ConnectError::AuthFailed`

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The otherwise unused imported enum is still constructed and discarded; it has no demonstrated behavioral purpose or missing-error contract.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:431](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L431).

Grouping/duplicate: [correctness-tdd.md#6](correctness-tdd.md#review-6). This is not an additional independent defect.

<a id="review-2"></a>

### Claim 2 — Concurrency theme clean; all mutexes are admin/DDL/boot frequency

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Authentication audit locks and runtime shard/bucket locking contradict this inventory. Exact scc 3.8.4 source also disproves universal lock-free HashMap entry access.

Evidence: [crates/shamir-server/src/audit_appender.rs:691](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/audit_appender.rs#L691); [crates/shamir-server/src/conn_limiter.rs:238](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/conn_limiter.rs#L238); [Cargo.lock:3123](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L3123).

Grouping/duplicate: [concurrency-lockfree.md#Summary](concurrency-lockfree.md#review-summary). This is not an additional independent defect.

<a id="review-2-namespace-note"></a>

### Claim 2/namespace-note — Concurrent namespace override updates can clobber each other

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Independent load/clone/store operations can overwrite another concurrent caller's override; atomic publication is not atomic compound update.

Evidence: [crates/shamir-server/src/logging.rs:168](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/logging.rs#L168); [crates/shamir-server/src/logging.rs:171](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/logging.rs#L171).

Grouping/duplicate: [concurrency-lockfree.md#Notes/set_namespace_level](concurrency-lockfree.md#review-notes-set-namespace-level). This is not an additional independent defect.

<a id="review-3-1"></a>

### Claim 3.1 — `ReplicationConfig::replicator_password` is a plain `String` held for the server's lifetime, reachable via `Debug`

Status: `confirmed-open`. Current risk: `medium`.

Prior-cycle decision: `confirmed-open`.

Unredacted temporary configuration and a retained non-zeroizing Arc&lt;str&gt; remain. No current password-printing log call was established; Config itself is boot-local.

Evidence: [crates/shamir-server/src/config.rs:133](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L133); [crates/shamir-server/src/replication/prod_factory.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/prod_factory.rs#L47).

Grouping/duplicate: [security-crypto.md#1](security-crypto.md#review-1). This is not an additional independent defect.

<a id="review-3-2"></a>

### Claim 3.2 — Replication client uses trust-on-first-use with no leader-key pinning

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

New sources omit the saved Ed25519 identity pin, but password-derived server authentication and TLS-exporter binding prevent the asserted unconditional network-only impersonation.

Evidence: [crates/shamir-server/src/replication/prod_factory.rs:116](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/prod_factory.rs#L116); [crates/shamir-connect/src/client/handshake.rs:259](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L259); [crates/shamir-connect/src/client/handshake.rs:283](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L283).

Grouping/duplicate: [security-crypto.md#2](security-crypto.md#review-2). This is not an additional independent defect.

<a id="review-3-3"></a>

### Claim 3.3 — `bootstrap_password` accepted as a CLI argument

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The option remains, with local command-line/history exposure dependent on deployment permissions and launch behavior.

Evidence: [crates/shamir-server/src/main.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/main.rs#L55).

Grouping/duplicate: [security-crypto.md#3](security-crypto.md#review-3). This is not an additional independent defect.

<a id="review-3-security-guarantees"></a>

### Claim 3/security-guarantees — Cryptographic and timing defenses are verified clean

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `unverified`.

Fixed-size MAC comparison is now source-supported by exact archives, but challenge-padding placement and stale bootstrap expiry time contradict clean timing/lifecycle conformance. The manifest guard is lexical, not universal containment.

Evidence: [crates/shamir-query-types/src/hmac.rs:434](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-query-types/src/hmac.rs#L434); [crates/shamir-server/src/connection/handshake.rs:210](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L210); [crates/shamir-server/src/connection/handshake.rs:277](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L277); [crates/shamir-server/src/connection/handshake.rs:431](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L431); [docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:515](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md#L515); [Cargo.lock:1151](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L1151).

Grouping/duplicate: [security-crypto.md#Summary/auth defenses](security-crypto.md#review-summary-auth-defenses). This is not an additional independent defect.

<a id="review-4"></a>

### Claim 4 — Performance theme clean and universally bounded

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `unverified`.

The production byte guard is destroyed before writer handoff; uncapped manifests and malformed-reply backoff exceptions provide further counter-evidence. Quantitative performance and exact eviction bounds remain unmeasured/unproved.

Evidence: [crates/shamir-server/src/byte_budget.rs:339](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/byte_budget.rs#L339); [crates/shamir-server/src/connection/request_loop.rs:348](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L348); [crates/shamir-server/src/backup.rs:404](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/backup.rs#L404); [crates/shamir-server/src/replication/follower_loop.rs:265](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L265).

Grouping/duplicate: [performance-hotpath.md#Summary](performance-hotpath.md#review-summary). This is not an additional independent defect.

<a id="review-5-1"></a>

### Claim 5.1 — `/info` endpoint documented as human-readable but returns raw msgpack

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

The misleading module prose conflicts with the MessagePack handler, HTTP decode assertion and operations guide.

Evidence: [crates/shamir-server/src/observability.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/observability.rs#L26); [crates/shamir-server/src/observability.rs:598](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/observability.rs#L598); [crates/shamir-server/tests/observability_http.rs:180](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/observability_http.rs#L180).

Grouping/duplicate: [api-wire-protocol.md#1](api-wire-protocol.md#review-1). This is not an additional independent defect.

<a id="review-5-2"></a>

### Claim 5.2 — `server_query_version` wire field is `u8` but the version constant is `u32`

Status: `confirmed-open`. Current risk: `low`.

Prior-cycle decision: `confirmed-open`.

Both unchecked casts remain; version 2 currently fits. This is future compatibility risk requiring a ceiling or coordinated client/server widening.

Evidence: [crates/shamir-server/src/connection/handshake.rs:152](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L152); [crates/shamir-server/src/connection/handshake.rs:640](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L640); [crates/shamir-client/src/client.rs:987](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L987).

Grouping/duplicate: [api-wire-protocol.md#2](api-wire-protocol.md#review-2). This is not an additional independent defect.

<a id="review-5-3"></a>

### Claim 5.3 — `check_destructive_hmacs` re-derives the HMAC key on every destructive-op sweep

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

OnceLock and insertion-time warmup predate the review, as confirmed in commit 111df6b331d1171bb60d6b9d6d7abf9f85924067. Repeated calls copy the cached key.

Evidence: [crates/shamir-connect/src/server/session.rs:284](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/session.rs#L284); [crates/shamir-connect/src/server/session.rs:446](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/session.rs#L446).

Grouping/duplicate: [api-wire-protocol.md#3](api-wire-protocol.md#review-3). This is not an additional independent defect.

<a id="review-6"></a>

### Claim 6 — Error-handling theme clean; invariant expects, complete boot cleanup and exhaustive failure tests

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

OS signal setup is fallible; post-spawn launch errors can detach tasks; restore tests do not assert SwapPartialFailure. Successful shutdown also lacks accepted-connection quiescence before its final flush.

Evidence: [crates/shamir-server/src/runtime.rs:81](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/runtime.rs#L81); [crates/shamir-server/src/server/server_launcher.rs:529](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L529); [crates/shamir-server/src/tests/restore_tests.rs:203](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tests/restore_tests.rs#L203); [crates/shamir-server/src/server/server_handle.rs:133](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_handle.rs#L133).

Grouping/duplicate: [error-handling-lifecycle.md#Summary](error-handling-lifecycle.md#review-summary). This is not an additional independent defect.

<a id="review-7-1"></a>

### Claim 7.1 — `use shamir_query_types::hmac as canon;` repeated 4× mid-function

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Four unconditional helper-local imports remain without a documented exception; hoisting is style-only.

Evidence: [crates/shamir-server/src/db_handler/admin.rs:119](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L119); [crates/shamir-server/src/db_handler/admin.rs:294](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L294); [crates/shamir-server/src/db_handler/admin.rs:375](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L375); [crates/shamir-server/src/db_handler/admin.rs:642](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L642).

Grouping/duplicate: [style-claude-md.md#1](style-claude-md.md#review-1). This is not an additional independent defect.

<a id="review-7-2"></a>

### Claim 7.2 — `config.rs` bundles 16 public types in one file

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

These are a permitted closely coupled nested configuration schema, with no numerical limit in the rule.

Evidence: [crates/shamir-server/src/config.rs:72](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L72); [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505).

Grouping/duplicate: [style-claude-md.md#2](style-claude-md.md#review-2). This is not an additional independent defect.

<a id="review-7-structural-guarantees"></a>

### Claim 7/structural-guarantees — All manifests, test layouts and other local imports conform

Status: `refuted`. Current risk: `—`.

Prior-cycle decision: `refuted`.

Common integration helpers contain mod.rs logic, service uses tests.rs, and unconditional helper-local imports are not a general exception.

Evidence: [crates/shamir-server/tests/common/mod.rs:36](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/common/mod.rs#L36); [crates/shamir-server/src/service.rs:623](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/service.rs#L623); [crates/shamir-server/src/subscriptions/bridge.rs:116](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/bridge.rs#L116).

Grouping/duplicate: [style-claude-md.md#Summary/mod.rs](style-claude-md.md#review-summary-mod-rs). This is not an additional independent defect.

<a id="review-cross-crate-context-handshake"></a>

### Claim Cross-crate context/handshake — Positional handshake DTOs independently mirrored with shamir-client

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Independent definitions remain, but inspected positional fields currently agree. This is maintenance/drift risk, not demonstrated wire incompatibility; centralization is an optional design change.

Evidence: [crates/shamir-server/src/connection/wire.rs:35](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/wire.rs#L35); [crates/shamir-client/src/wire_frames.rs:67](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/wire_frames.rs#L67).

<a id="review-cross-crate-context-browser-binding"></a>

### Claim Cross-crate context/browser-binding — `BROWSER_CHANNEL_BINDING` re-hardcoded at consumption sites

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

The exported constant and server-local array both remain zero and satisfy the browser binding requirement. Duplication is maintenance debt, not present binding divergence.

Evidence: [crates/shamir-transport-ws/src/tls_exporter.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-ws/src/tls_exporter.rs#L25); [crates/shamir-server/src/server/server_launcher.rs:1495](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1495).

<a id="review-cross-crate-context-loopback"></a>

### Claim Cross-crate context/loopback — Duplicated normative loopback predicate with shamir-transport-tcp

Status: `confirmed-open`. Current risk: `nit`.

Prior-cycle decision: `confirmed-open`.

Both helpers delegate the same IPv4/IPv6 branches to standard-library classifiers. The server documents why it avoids the transport-profile validation API. No current policy difference is established.

Evidence: [crates/shamir-server/src/config.rs:873](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L873); [crates/shamir-server/src/config.rs:877](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L877); [crates/shamir-transport-tcp/src/listener.rs:51](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-transport-tcp/src/listener.rs#L51).

## Revalidated plan decisions

| Plan decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 17 | 14 | 0 | 0 | 1 | 0 | 2 |

Historical P0/P1/P2 numbering is an identifier, not a current release mandate. The reasons below include completion status, safety qualifications and discriminating acceptance requirements.

<a id="plan-p0-1"></a>

### Plan P0.1 — P0.1

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Gate and transaction-specific regression remain absent. Reject write-classified batches before staging while preserving version, ownership and ACL checks; assert the typed read_only_replica response and unchanged data through the real transaction dispatch path.

Evidence: [crates/shamir-server/src/db_handler/tx_handlers.rs:168](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tx_handlers.rs#L168); [crates/shamir-server/src/db_handler/tests/node_mode_tests.rs:157](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tests/node_mode_tests.rs#L157).

<a id="plan-p0-2"></a>

### Plan P0.2 — P0.2

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Task-liveness recovery remains absent. Name-only removal on each stream exit is unsafe for live siblings/replacement generations; distinguish generation and stream ownership, stop/join obsolete siblings, and test active-state terminal exit plus early resume.

Evidence: [crates/shamir-server/src/replication/supervisor.rs:275](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/supervisor.rs#L275); [crates/shamir-server/src/replication/supervisor.rs:286](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/supervisor.rs#L286); [crates/shamir-server/src/replication/supervisor.rs:333](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/supervisor.rs#L333).

<a id="plan-p1-3"></a>

### Plan P1.3 — P1.3

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Missing-slot abort and a complete admission/teardown barrier remain absent. A check-then-spawn closed flag alone can still race. Test a genuinely live task removed before attachment and activation concurrent with the final teardown sweep.

Evidence: [crates/shamir-server/src/subscriptions/registry.rs:134](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/subscriptions/registry.rs#L134); [crates/shamir-server/src/connection/request_loop.rs:413](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L413); [crates/shamir-server/src/connection/request_loop.rs:417](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L417).

<a id="plan-p1-4"></a>

### Plan P1.4 — P1.4

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Hello still retries immediately. Delay must remain cancellation-aware; move/reset backoff only for genuinely healthy progress if escalating malformed-response retries are intended. Test delay growth and prompt cancellation, not merely presence of a sleep call.

Evidence: [crates/shamir-server/src/replication/follower_loop.rs:265](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L265); [crates/shamir-server/src/replication/follower_loop.rs:281](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/follower_loop.rs#L281).

<a id="plan-p1-5"></a>

### Plan P1.5 — P1.5

Status: `not-applicable`.

Prior-cycle decision: `not-applicable`.

Raw prefix semantics are already documented/tested. Requiring :: would break underscore target matching; a collision example is optional clarification.

Evidence: [crates/shamir-server/src/logging.rs:135](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/logging.rs#L135); [crates/shamir-server/src/logging/tests/log_mask_tests.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/logging/tests/log_mask_tests.rs#L64).

<a id="plan-p1-6"></a>

### Plan P1.6 — P1.6

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Redacted configuration and zeroizing retained credentials are absent. Preserve Ktav parsing, cloning and reconnect lifetime; redaction and zeroization must cover intermediate copies as well as the final outbound buffer, without claiming live-memory secrecy.

Evidence: [crates/shamir-server/src/config.rs:133](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L133); [crates/shamir-server/src/replication/prod_factory.rs:47](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/prod_factory.rs#L47); [crates/shamir-server/src/replication/prod_factory.rs:112](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/prod_factory.rs#L112).

<a id="plan-p1-7"></a>

### Plan P1.7 — P1.7

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Misleading prose remains. Correct observability's header to match MessagePack; changing the handler to JSON would instead alter the documented/tested representation.

Evidence: [crates/shamir-server/src/observability.rs:26](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/observability.rs#L26); [crates/shamir-server/src/observability.rs:598](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/observability.rs#L598); [crates/shamir-server/tests/observability_http.rs:180](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/observability_http.rs#L180).

<a id="plan-p1-8"></a>

### Plan P1.8 — P1.8

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Unchecked casts remain. Guard the actual imported query-types constant at compile time or coordinate every advertised-version consumer. Silent wrap/clamp is not a semantically safe capability advertisement.

Evidence: [crates/shamir-server/src/connection/handshake.rs:25](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L25); [crates/shamir-server/src/connection/handshake.rs:152](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L152); [crates/shamir-client/src/client.rs:456](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L456); [crates/shamir-client/src/client.rs:987](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-client/src/client.rs#L987).

<a id="plan-p1-9"></a>

### Plan P1.9 — P1.9

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

All three proposed test changes remain absent. Use scheduler's actual tick seam for panic-then-recovery, a local shutdown bound for localization, and direct HTTP observability spawn for false→503→ready→200. Integration targets require full runner selection.

Evidence: [crates/shamir-server/tests/scheduler.rs:156](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/scheduler.rs#L156); [crates/shamir-server/tests/scheduler.rs:44](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/scheduler.rs#L44); [crates/shamir-server/tests/observability_http.rs:149](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/observability_http.rs#L149); [scripts/test.sh:177](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/scripts/test.sh#L177).

<a id="plan-p1-10"></a>

### Plan P1.10 — P1.10

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Four imports remain. Hoisting is behavior-preserving style cleanup and should not be treated as urgent runtime remediation or mixed with unrelated formatting.

Evidence: [crates/shamir-server/src/db_handler/admin.rs:119](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L119); [crates/shamir-server/src/db_handler/admin.rs:642](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/admin.rs#L642).

<a id="plan-p2-11"></a>

### Plan P2.11 — P2.11

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Additional logging is absent and remains optional latent diagnostics work. Intentional teardown cancellation should not be converted into misleading operational errors; no current individual-abort request-loss witness was established.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:242](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L242); [crates/shamir-server/src/connection/request_loop.rs:415](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L415).

<a id="plan-p2-12"></a>

### Plan P2.12 — P2.12

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Persisted upstream Ed25519 pinning remains absent. Preserve SCRAM/exporter checks and define trusted bootstrap, rotation and replacement behavior; silently accepting a replacement defeats persistence.

Evidence: [crates/shamir-server/src/replication/prod_factory.rs:116](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/replication/prod_factory.rs#L116); [crates/shamir-connect/src/client/handshake.rs:264](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/client/handshake.rs#L264).

<a id="plan-p2-13"></a>

### Plan P2.13 — P2.13

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

The unused discard remains. Removing it and its import is sufficient; wiring a new error path requires an independently established contract, not historical speculation.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:42](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L42); [crates/shamir-server/src/connection/request_loop.rs:431](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L431).

<a id="plan-p2-14"></a>

### Plan P2.14 — P2.14

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Only optional alternative bootstrap-password input remains actionable. Session caching already exists and config splitting is not mandated. An environment-variable alternative is not automatically confidential.

Evidence: [crates/shamir-server/src/main.rs:56](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/main.rs#L56); [crates/shamir-connect/src/server/session.rs:284](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/session.rs#L284); [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505).

<a id="plan-p2-14-bootstrap-password"></a>

### Plan P2.14/bootstrap-password — P2.14/bootstrap-password

Status: `confirmed-open`.

Prior-cycle decision: `confirmed-open`.

Plaintext CLI input remains supported. A protected input source can reduce argv exposure, but permission and logging behavior must be specified; preserve noninteractive deployment needs.

Evidence: [crates/shamir-server/src/main.rs:55](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/main.rs#L55).

<a id="plan-p2-14-hmac-cache"></a>

### Plan P2.14/HMAC-cache — P2.14/HMAC-cache

Status: `refuted`.

Prior-cycle decision: `refuted`.

OnceLock caching and ID-stamping warmup predate the original review. A second cache adds state/invalidation obligations without resolving an existing defect.

Evidence: [crates/shamir-connect/src/server/session.rs:284](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/session.rs#L284); [crates/shamir-connect/src/server/session.rs:446](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-connect/src/server/session.rs#L446).

<a id="plan-p2-14-config-split"></a>

### Plan P2.14/config-split — P2.14/config-split

Status: `not-applicable`.

Prior-cycle decision: `not-applicable`.

The schema is a permitted coupled group. Splitting is optional organization work; any chosen split must preserve public paths and deserialization behavior.

Evidence: [crates/shamir-server/src/config.rs:72](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/config.rs#L72); [CLAUDE.md:505](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/CLAUDE.md#L505).

## Additional observations

| Observation decisions | Open | Source-fixed | Partial | Refuted | Unverified | N/A |
|---:|---:|---:|---:|---:|---:|---:|
| 4 | 4 | 0 | 0 | 0 | 0 | 0 |

Existing observation IDs remain stable. New cycle-2 rows follow them; cross-module duplicates and extra triggers are grouped explicitly. None is an implemented fix.

<a id="new-1"></a>

### Observation NEW.1 — SCRAM client-proof stage has no deadline after the bounded AuthInit read

Status: `confirmed-open`. Current risk: `high`.

Prior-cycle decision: `confirmed-open`.

A peer can send a syntactically valid AuthInit, receive the challenge and withhold proof indefinitely. The proof read is awaited without a deadline or state-GC registration. Admission/rate caps bound occupied slots, not their duration. Correction: ServerHandle shutdown does not cancel these accepted connection tasks; they end on disconnect or runtime/process teardown, not necessarily on shutdown(). Parent acceptance also checks AUTH_PROTOCOL's separate HANDSHAKE_TIMEOUT requirement and the production TCP/WS caller awaits. Current risk is high for indefinite unauthenticated admission-slot occupation; the new TCP report restates this existing root rather than adding another unique defect.

Evidence: [crates/shamir-server/src/connection/handshake.rs:291](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L291); [crates/shamir-server/src/connection/handshake.rs:718](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L718); [crates/shamir-server/src/connection/handshake.rs:771](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L771); [crates/shamir-server/src/server/server_launcher.rs:1137](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1137); [crates/shamir-server/src/server/server_handle.rs:104](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_handle.rs#L104); [docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:507](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md#L507).

Grouping/duplicate: `shamir-transport-ws/SUMMARY.md#3.4`. This is not an additional independent defect.

<a id="observation-new-2"></a>

### Observation NEW.2 — Production response-budget guard is destroyed before writer handoff

Status: `confirmed-open`. Current risk: `high`.

Additional observation in this independent cycle; it may overlap an existing root.

request_loop awaits run_with_guard_slot(dispatch_request_view(...)), then calls take_stashed_guard outside that scope. Tokio 1.49.0 TaskLocalFuture restores the previous task-local value after each poll and drops its owned slot when the scoped future ends; consequently the stashed guard drops before WriterMsg creation and try_with returns no guard. Responses waiting in the channel/socket therefore escape the promised global write-residency budget even with the default finite cap. Existing registered helpers take the guard inside the scope, so they exercise a different ownership seam. Exact source: https://docs.rs/crate/tokio/1.49.0/source/src/task/task_local.rs. The discriminating oracle must use the real request loop with a blocked writer, verify bytes remain reserved, then verify release on write completion/error. Retrieve ownership inside the scope or return it explicitly with dispatch output; retain writer-side release semantics.

Evidence: [crates/shamir-server/src/connection/request_loop.rs:340](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L340); [crates/shamir-server/src/connection/request_loop.rs:348](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/request_loop.rs#L348); [crates/shamir-server/src/byte_budget.rs:339](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/byte_budget.rs#L339); [crates/shamir-server/src/byte_budget.rs:372](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/byte_budget.rs#L372); [crates/shamir-server/src/db_handler/handler.rs:660](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/handler.rs#L660); [crates/shamir-server/src/db_handler/tests/byte_budget_exhaustion_tests.rs:126](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/db_handler/tests/byte_budget_exhaustion_tests.rs#L126); [Cargo.lock:4195](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/Cargo.lock#L4195).

<a id="observation-new-3"></a>

### Observation NEW.3 — Bootstrap-token TTL check uses authentication-start time after awaiting proof

Status: `confirmed-open`. Current risk: `high`.

Additional observation in this independent cycle; it may overlap an existing root.

run_handshake captures now_ns before sending the challenge, waits for proof, then passes the old timestamp to read_bootstrap_token_state. For t_init &lt; token_expiry &lt;= t_proof, metadata is readable and still active, and no intervening rotation occurred, the expiry predicate is false despite expiry before the session grant. A holder of the bootstrap secret can start before expiration and finish afterward; the unbounded proof wait widens the window, but the timestamp defect also exists with a finite wait. This is not passwordless or arbitrary-principal access. The scheduler does not perform the boot-only token rotation sweep. Existing caller-copy unit tests and an already-expired-before-login e2e case do not discriminate this boundary. Refresh time at the post-proof security decision and test expiry between challenge and proof through the actual handshake.

Evidence: [crates/shamir-server/src/connection/handshake.rs:210](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L210); [crates/shamir-server/src/connection/handshake.rs:291](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L291); [crates/shamir-server/src/connection/handshake.rs:431](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/handshake.rs#L431); [crates/shamir-server/src/server_meta.rs:509](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server_meta.rs#L509); [crates/shamir-server/src/server_meta.rs:524](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server_meta.rs#L524); [crates/shamir-server/src/tests/connection_tests.rs:240](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/tests/connection_tests.rs#L240); [crates/shamir-server/tests/bootstrap_token_lifecycle_e2e.rs:372](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/bootstrap_token_lifecycle_e2e.rs#L372).

<a id="observation-new-4"></a>

### Observation NEW.4 — Graceful shutdown flushes without quiescing already accepted connection tasks

Status: `confirmed-open`. Current risk: `medium`.

Additional observation in this independent cycle; it may overlap an existing root.

Accept loops spawn detached connection tasks that retain ConnectionContext and database/store owners but receive no root shutdown token. ServerHandle cancels and joins accept loops, then stops maintenance and performs flush_all, while existing authenticated connections can still dispatch writes. Thus the 'no new writes can arrive' prerequisite is false; on a surviving library runtime, sessions/resources can outlive shutdown and the instance lock, and process-exit ordering can leave writes occurring after the intended final flush. No exact loss window or observed corruption is claimed. runtime_serve's test opens no active connection, so it cannot catch this. A discriminating test must retain an authenticated connection across shutdown and verify admission/drain and flush ordering; do not simply abort arbitrary in-progress commits without defining their cancellation/durability semantics.

Evidence: [crates/shamir-server/src/server/server_launcher.rs:1137](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1137); [crates/shamir-server/src/server/server_launcher.rs:1165](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_launcher.rs#L1165); [crates/shamir-server/src/connection/connection_context.rs:30](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/connection/connection_context.rs#L30); [crates/shamir-server/src/server/server_handle.rs:104](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_handle.rs#L104); [crates/shamir-server/src/server/server_handle.rs:129](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/src/server/server_handle.rs#L129); [crates/shamir-db/src/shamir_db/shamir_db/db_management.rs:632](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-db/src/shamir_db/shamir_db/db_management.rs#L632); [crates/shamir-server/tests/runtime_serve.rs:64](https://github.com/PHPCraftdream/shamir-db/blob/e3765c935fc71655ee1ec0160cf180607935d89b/crates/shamir-server/tests/runtime_serve.rs#L64).

## Evidence and recipe corrections

- No source changes occurred between the prior report snapshot and the frozen HEAD in the inspected crate/dependency/instruction paths. Revalidated counter-evidence must not be called a newly landed fix.
- The original three refutations remain 1.5, 5.3 and 7.2; numbered claim counts are not a unique runtime-bug census.
- Fixed-size HMAC comparison is now supported by exact cached archives. The earlier source-unavailability statement is false for this review environment.
- Clean timing/security and universally bounded-performance assurances have positive source counterexamples, not merely missing measurements.
- Add production response-guard ownership, expiry-after-proof and accepted-connection quiescence to follow-up priorities; they are distinct from the existing NEW.1 deadline observation.
- Lower current diagnostic/test-localization entries 1.6 and 1.7 to nit, and treat mirrored handshake definitions as nit-level maintenance risk rather than current incompatibility.
- A proof deadline must respect the normative client-KDF allowance; reusing the five-second AuthInit timeout blindly is not the documented full-handshake policy.
- The current NEW.1 phrase 'until disconnect/shutdown' overstates shutdown cancellation: accepted tasks do not receive the root token.
- Parent acceptance raises the existing proof-stage observation to high after the protocol's separate timeout contract and production caller paths are checked. It is shared with TCP/WS, not another newly discovered root.

## Module scope and limitations

Coverage: 8 assigned documents, 77 current claim rows, 17 plan rows, 1 pre-existing observation rows; 3 added observation rows in this cycle. Counts are calculated from the accepted rows.

Assigned documents: [correctness-tdd.md](correctness-tdd.md); [concurrency-lockfree.md](concurrency-lockfree.md); [security-crypto.md](security-crypto.md); [performance-hotpath.md](performance-hotpath.md); [api-wire-protocol.md](api-wire-protocol.md); [error-handling-lifecycle.md](error-handling-lifecycle.md); [style-claude-md.md](style-claude-md.md); [SUMMARY.md](SUMMARY.md).

- All eight assigned documents were read completely, including current ledgers, corrections, parent refinements and collapsed historical material. Historical repetitions were not counted again.
- HEAD matched the required base at the beginning and end; the inspected worktree was clean. No files were written and no programs, tests, builds, reproductions or agents were launched.
- Test registration and assertions were inspected, not executed. Windows filesystem fault-forcing assumptions and scheduling-dependent watcher behavior were not experimentally verified.
- Exact cached dependency archives were inspected without extraction to disk. SHA-256 hashes of hmac 0.12.1, digest 0.10.7 and scc 3.8.4 archives matched Cargo.lock.
- Source-level constant-time comparison is supported; generated machine-code timing, timing-oracle exploitability and quantitative CPU, memory or latency effects were not measured.
- The exact worst-case eviction complexity of concurrent scc TreeIndex range removal remains unverified; its implementation was available and inspected.
- This was bounded claim revalidation, not an exhaustive fresh audit of every crate path or transitive dependency. The declared Rust 1.94.0 toolchain was treated only as metadata.

## Guarantee checks

- **An explicitly ReadOnly handler rejects client writes before engine execution.** — `diverges`. Execute checks NodeMode, but TxExecute stages authorized writes and TxCommit applies them without that check. Stock launcher configuration remains ReadWrite. Reference: crates/shamir-server/src/db_handler/config.rs:40.
- **Active replication subscriptions converge to running follower loops.** — `diverges`. Registry presence substitutes for task liveness. A terminal epoch-fencing error leaves an active catalogue row with a dead registered task; reconciliation skips restarting it. Reference: crates/shamir-server/src/replication/supervisor.rs:4.
- **Every subscription bridge dies with its granting connection.** — `diverges`. Connection teardown can remove a pending placeholder before attachment, causing a live JoinHandle to be detached. Dispatch can also activate subscriptions after the initial close_all sweep. Reference: docs/guide-docs/client-server-protocol-spec/SUBSCRIPTIONS.md:38.
- **Response-byte reservations survive until the socket write finishes.** — `diverges`. The dispatch task takes the guard after run_with_guard_slot has returned and destroyed its scoped storage. The production writer receives no reservation for those responses. Reference: crates/shamir-server/src/connection/request_loop.rs:87.
- **Unproved handshakes expire after HANDSHAKE_TIMEOUT.** — `diverges`. TLS/upgrade and AuthInit have deadlines, but the subsequent client-proof read has none and no handshake-state reaper covers this local state. Reference: docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:501.
- **An expired outstanding bootstrap credential is rejected before granting a session.** — `diverges`. The expiry check receives a timestamp captured before awaiting the proof. A credential expiring during that wait can still be accepted. Reference: crates/shamir-server/src/connection/handshake.rs:399.
- **Challenge and negative authentication responses receive the normative latency padding.** — `diverges`. The challenge is written inside run_handshake before the outer padding calculation; successful AuthOk is also written before that calculation. No measured enumeration advantage is asserted. Reference: docs/guide-docs/client-server-protocol-spec/AUTH_PROTOCOL.md:515.
- **Valid fixed-size HMAC tags are compared using content-independent, subtle-backed comparison.** — `supported`. Exact published archives show verify_slice checking public length and then calling ct_eq; subtle folds all equal-length slice elements without content-dependent early termination. Hex-format rejection is separate and may exit early. Reference: hmac 0.12.1; digest 0.10.7 src/mac.rs::verify_slice; subtle 2.6.1 src/lib.rs.
- **scc HashMap provides universally lock-free entry access and len is iter().count().** — `diverges`. The exact implementation serializes entry access with bucket read/write locks. HashMap::len sums bucket lengths across arrays; TreeIndex::len uses iterator counting. Both are scans, but not the same implementation. Reference: CLAUDE.md:331; Cargo.lock:3123; scc 3.8.4 src/hash_map.rs and src/hash_table.rs.
- **Graceful final flushing happens after writes have stopped.** — `diverges`. Stopping accept loops does not stop already accepted connection tasks, which receive neither the shutdown token nor a joined lifetime. They can execute writes after the final flush. Reference: crates/shamir-server/src/server/server_handle.rs:129.
- **Manifest verification rejects lexical absolute/parent traversal paths and duplicate entries.** — `supported`. These checks precede hashing. They do not establish symlink containment, a manifest-size ceiling or fixed memory for the whole operation. Reference: crates/shamir-server/src/backup.rs:420.
- **Current server/client positional handshake fields agree.** — `supported`. The inspected AuthOk and resume fields currently agree in order, defaults and widths. Independent definitions and unchecked future version narrowing remain maintenance risks. Reference: crates/shamir-server/src/connection/wire.rs:35; crates/shamir-client/src/wire_frames.rs:67.
- **The named unit and integration tests are registered, with integration selection requiring the full runner mode.** — `supported`. Nested test manifests are connected to the library; Cargo auto-discovers tests/scheduler.rs and tests/observability_http.rs. The default wrapper selects --lib, whereas --full includes integration targets. Reference: crates/shamir-server/src/lib.rs:54; crates/shamir-server/src/tests/mod.rs:1; scripts/test.sh:126.
- **Closely coupled configuration types may share a source file.** — `supported`. The sixteen configuration types form one nested parsing/validation schema, and the rule imposes no numerical type limit. Reference: CLAUDE.md:505; crates/shamir-server/src/config.rs:72.

## Reviewer's prior-cycle comparison

These are the independent reviewer's comparisons before parent refinements; the accepted ledgers above govern final decisions and counts.

- security-crypto.md Summary/HMAC constant-time: unverified → not-applicable as a supported non-finding. Exact hmac/digest/subtle archives establish the fixed-size comparison chain; their absence was not a valid limitation here.
- security-crypto.md Summary/auth defenses and SUMMARY.md 3/security-guarantees: unverified → refuted for the blanket conformance assurance. Challenge output precedes normative padding, and bootstrap expiry is checked using time captured before proof waiting.
- performance-hotpath.md Summary and SUMMARY.md 4: unverified → refuted. The actual production task-local lifetime loses the response reservation before the writer, which existing helper tests do not model.
- correctness-tdd.md 5/8 and SUMMARY.md 1.6/1.7 remain open observations but severity low → nit: current individual cancellation is not established, and the scheduler test already has runner-level hang detection.
- SUMMARY.md Cross-crate context/handshake remains a duplication observation but severity low → nit: current fields agree; no incompatibility witness was found.
- Existing NEW.1 is confirmed, but its shutdown termination assurance is inaccurate because accepted connection tasks do not receive root cancellation.
- The original prefix, HMAC re-derivation and config-type-count refutations are independently supported. None represents a post-review source fix.
- References checked include root AGENTS.md/CLAUDE.md, crate manifest and test roots, protocol README, AUTH_PROTOCOL §§5.3/8/9, SUBSCRIPTIONS §1.1, CURSORS pagination/limit sections, IMPLEMENTATION_GUIDE persistence/shutdown sections, SECURITY_MODEL adversary scope, operations observability/logging documentation, cache-introduction history and exact locked dependency source.

## Current follow-up order

1. Correct bootstrap expiry at the post-proof decision and bound the complete proof stage using the documented KDF-compatible handshake policy.
2. Repair production response-guard transfer and add a real blocked-writer ownership oracle; existing scoped helper tests miss the seam.
3. Establish accepted-connection admission/drain before final flush, and cancellation cleanup for post-spawn launch failures.
4. Fix the explicitly ReadOnly transaction write bypass while preserving ACL and ownership checks.
5. Repair generation-safe, multi-stream supervisor liveness and subscription admission/teardown barriers.
6. Correct malformed-reply backoff/reset placement; then address credential redaction/zeroization and persisted Ed25519 pin policy.
7. Add panic-recovery and readiness-negative tests, guard future query-version width, correct /info prose and repair namespace atomic updates.
8. Leave optional diagnostic, style and duplication cleanup below demonstrated production lifecycle defects.

---

<details>
<summary>Historical report — preserved for provenance; not current status or instructions</summary>

# shamir-server — Synthesized 7-lens review (consolidated from the 2026-08-14 cross-crate review)

Crate: `crates/shamir-server/` (the client-facing DB server: SCRAM/TLS connection boundary in
`connection/`, request dispatch + query/tx/admin/cursor handlers in `db_handler/`, the replication
follower/supervisor in `replication/`, the subscription push subsystem in `subscriptions/`, boot
orchestration in `server/`, plus config/logging/scheduler/observability/backup infrastructure).

Review basis — synthesized from the seven 2026-08-14 lens reports in this directory:
`correctness-tdd.md`, `concurrency-lockfree.md`, `security-crypto.md`, `performance-hotpath.md`,
`api-wire-protocol.md`, `error-handling-lifecycle.md`, `style-claude-md.md`. Structure/tone
calibrated against the already-consolidated exemplars
`../shamir-client-node/SUMMARY.md` and `../shamir-transport-ipc/SUMMARY.md`; per-crate context
(the lens-tagged row and the "Per-Crate Health Scorecard" entry) taken from the workspace-wide
`../SUMMARY.md`. Read-only synthesis — no build, no tests, no source modifications. Key file:line
references were spot-verified against the working tree during synthesis (including the critical's
gate/dispatcher lines); minor verified corrections are noted inline where cited lines drifted.

## Executive summary

`shamir-server` is otherwise the cleanest crate in the 23-crate sweep — three of seven lenses
(concurrency, performance, error-handling) returned zero findings, and the code carries dense
incident citations, RAII-disciplined resource lifecycle, and pillar-conformant concurrency — but it
carries the workspace's ONE server-side **critical**: **the interactive-transaction path
(`TxBegin`/`TxExecute`/`TxCommit`) never checks `NodeMode`, so any authenticated client can write
through a node configured as a read-only replica via the tx API, writes that `execute()` rejects
with `read_only_replica` go straight to the engine — durable local writes on the replica, silent
split-brain divergence from the leader** (`db_handler/tx_handlers.rs` vs the gate at
`db_handler/handler.rs:529-541`; spot-verified: zero `node_mode`/`NodeMode` references in
`tx_handlers.rs`, unguarded dispatch at `handler.rs:369-393`). Fix that first — mirror the gate
into `tx_execute()` and extend the (explicitly `execute()`-scoped) `node_mode_tests.rs` with a Red
test — then the HIGH follower-supervisor liveness bug that permanently strands a repaired
subscription behind a dead registry entry (`replication/supervisor.rs:286-337`). Everything else is
medium-and-below; the notable remainder is secret hygiene on the replication password
(`config.rs`), the `/info` doc-vs-msgpack mismatch, and a small cluster of TDD gaps on exactly the
negative paths the crate documents (read-only gate at tx level, `safe_run` panic survival, `/readyz`
503).

---

## 1. correctness-tdd

### 1.1 — critical — Interactive-tx path bypasses the read-only-replica gate entirely *(the workspace's one server-side critical)*
- File:line: `crates/shamir-server/src/db_handler/tx_handlers.rs:73-205` (`tx_execute`;
  `tx_begin` at :12, `tx_commit` at :211 — neither references `node_mode`), compare the gate at
  `crates/shamir-server/src/db_handler/handler.rs:529-541`; unguarded dispatch at
  `handler.rs:369-393`.
- Issue: `ShamirDbHandler::execute()` rejects any batch containing a write op when
  `self.node_mode == NodeMode::ReadOnly` (`handler.rs:529-541`). `tx_execute()` copies every other
  per-batch gate from `execute()` verbatim — the version check, the query-limits clamp
  (`tx_handlers.rs:88-100`), the admin/superuser gate (`tx_handlers.rs:102-116`), and the
  destructive-HMAC gate (`tx_handlers.rs:118-124`) — but has no equivalent check against
  `node_mode`/`is_write()`. `tx_begin()`/`tx_commit()` likewise never reference `node_mode`/`NodeMode`
  (synthesis spot-check re-confirmed: zero `node_mode|NodeMode|ReadOnly` matches in
  `tx_handlers.rs`), and the top-level dispatcher (`handler.rs:369-393`) routes `TxBegin`/`TxExecute`/
  `TxCommit` straight to these methods with no outer gate either.
- Failure scenario: a node is started with `NodeMode::ReadOnly` (a replica follower per
  `config.rs`'s documented invariant "a replica follower runs ReadOnly and rejects client writes").
  A client that would be rejected via `Execute` with `code: "read_only_replica"` instead opens
  `TxBegin`, stages an `insert`/`upsert`/`delete` via `TxExecute`, and calls `TxCommit` — the write
  goes through the engine and is applied locally on the replica, silently diverging replica state
  from the leader and violating the single-writer invariant the whole read-only-replica feature
  exists to enforce. Impact is *persistent silent corruption*: durable local writes on the replica,
  split-brain divergence, reads serving phantom data, resync required.
- Suggested fix: add the same `if self.node_mode == NodeMode::ReadOnly { ... }` write-rejection
  block to `tx_execute()` (mirroring `handler.rs:529-541`), and add a `node_mode_tests.rs` case
  that opens a `TxBegin`/`TxExecute` against a `ReadOnly` handler and asserts rejection.

### 1.2 — high — Dead follower-loop registry entries block resubscription after a journal gap
- File:line: `crates/shamir-server/src/replication/supervisor.rs:286-337` (spawn block never clears
  the registry on task exit) interacting with `reconcile()` at `supervisor.rs:229-237` and
  `is_running`/`contains_sync` at `supervisor.rs:182-184, 231`.
- Issue: when a spawned follower-loop task terminates (`JournalGap` or any other `ReplError`), the
  `tokio::spawn` closure only logs a warning (`supervisor.rs:294-320`, spot-verified: mark-
  `resync_required` + `warn!`, no registry cleanup) — it never calls
  `self.registry.remove_sync(&sub_name)`. The `SubHandle` (now-dead `JoinHandle`s and a
  `CancellationToken` nobody will ever cancel) stays in `registry` indefinitely. `reconcile()`'s
  start-step skips any subscription that `self.registry.contains_sync(&sub.name)`
  (`supervisor.rs:231`), so once a loop dies this way, no subsequent `reconcile()`/`notify_changed()`
  call can ever restart it for that subscription name — even after the underlying gap condition is
  repaired and the row is flipped back to `active`. The only two paths that clear the entry are
  `stop_all` on full shutdown, or a rebind-to-different-profile detected by `reconcile`'s stop-step.
- Failure scenario: a follower hits a journal gap, the loop exits and marks the subscription
  `resync_required`; an operator repairs the gap (e.g. re-seeds via snapshot) and flips the row back
  to `active` — `reconcile()` runs but does nothing, because the stale registry entry still
  satisfies `contains_sync`. The subscription is permanently stuck until process restart or
  delete-and-recreate under a different name/profile — i.e. the resume-after-repair path this whole
  mechanism exists for is dead.
- This gap is self-admitted in the test suite, not just inferred:
  `crates/shamir-server/src/replication/tests/supervisor_tests.rs:398-401` reads verbatim: *"The dead
  loop task leaves a stale registry entry (pre-existing loop-liveness gap, out of scope for this
  task) — but a fresh `reconcile()` must NOT start a new loop..."* (spot-verified verbatim) — the
  test explicitly works around the bug rather than covering the operationally relevant
  resume-after-repair path.
- Suggested fix: have the spawned closure call `self.registry.remove_sync(&sub_name)` (or a shared
  "mark dead" helper) on every exit path so `reconcile()` can restart a repaired subscription; add a
  regression test that flips the row from `resync_required` back to `active` and asserts a fresh
  loop is spawned.

### 1.3 — medium — `close_all()` / `attach_handle()` race can leak a bridge task past connection teardown
- File:line: `crates/shamir-server/src/subscriptions/registry.rs:109-138` (`reserve_pending` →
  spawn → `attach_handle` race vs concurrent `close_all` at `registry.rs:150-153`) and caller
  `crates/shamir-server/src/db_handler/subscribe_handler.rs:86-106`; teardown at
  `connection/request_loop.rs:413`.
- Issue: `activate_subscriptions` calls `registry.reserve_pending(sub_id)`, then
  `tokio::spawn(bridge::bridge_task(...))`, then `registry.attach_handle(sub_id, handle)` — no
  `.await` between these three steps, but they run as a distinct task from connection teardown's
  `registry.close_all()` (`request_loop.rs:413`), so on a multi-thread runtime the two can genuinely
  race across OS threads. If `close_all()`'s `retain_sync(|_, _| false)` removes+drops the
  `reserve_pending`'d placeholder (which has `bridge_handle: None`) in the window before
  `attach_handle` runs, `ActiveSubscription::Drop` (`registry.rs:20-25`) has nothing to abort.
  `attach_handle`'s `update_sync` on the now-missing key is documented as a no-op
  (`registry.rs:129-133`): "the task has already finished, so `handle` is simply dropped" — but in
  this race the bridge task has *not* finished; the freshly-spawned `JoinHandle` is dropped directly
  (not wrapped in an `ActiveSubscription`), and a bare `JoinHandle::drop` on a running task
  **detaches rather than aborts** it per tokio's documented semantics.
- Failure scenario: a client disconnects at the exact moment a `Subscribe` batch entry is being
  activated; the bridge task keeps running detached, holding `Arc<dyn PushSink>`/`Arc<ShamirDb>`
  clones, defeating the very close_all-before-drop ordering that `request_loop.rs:410-413`'s comment
  says exists specifically to let `conn`/`tx` be dropped safely afterward. No test in
  `subscriptions/tests/` or `db_handler/tests/subscribe_handler_tests.rs` calls `close_all()` at all
  — the race is entirely uncovered.
- Suggested fix: either hold a short lock/generation-token across `reserve_pending`→`attach_handle`
  so `close_all` can't observe the half-attached state, or have `attach_handle`'s no-op path
  re-check-and-abort by having the caller retain the handle and abort it itself when `attach_handle`
  reports "slot gone."

### 1.4 — medium — Follower loop busy-loops on an unexpected `Hello` reply (missing backoff)
- File:line: `crates/shamir-server/src/replication/follower_loop.rs:278-285` (spot-verified: the
  `ReplResponse::Hello` arm does a bare `continue` at :284-285).
- Issue: every other degenerate-reply branch in the pull-response dispatch calls
  `sleep_backoff(&cancel, &mut backoff_ms).await` before retrying — e.g. the `ReplResponse::Error`
  branch at :267-277. The `ReplResponse::Hello` branch (an unexpected reply shape to a `pull`
  request) instead does a bare `continue` with a "skip this iteration" comment and no backoff call.
- Failure scenario: a misbehaving or regressed `ReplSource` that keeps replying `Hello` to `pull`
  requests causes this branch to spin as fast as the transport allows, with no rate limiting — a
  busy loop consuming CPU and hammering the source instead of degrading gracefully like every
  sibling error branch.
- Suggested fix: add the same `sleep_backoff(&cancel, &mut backoff_ms).await` call before `continue`
  in the `Hello` arm.

### 1.5 — medium — Log-mask override matching is a raw substring/prefix test, not a namespace-segment match
- File:line: `crates/shamir-server/src/logging.rs:137-146` (spot-verified:
  `target.starts_with(prefix.as_str())` with no `::` boundary check).
- Issue: `LogMask::allows` picks the override via `starts_with` with no check for a `::` boundary
  after the matched prefix. An override registered for a short namespace constant (e.g.
  `ns::TX = "tx"`) also matches any future target that merely starts with the same characters
  (`"tx_replication"`, `"txn_metrics"`), silently inheriting that override's verbosity even though it
  is a semantically unrelated module.
- Failure scenario: an operator raises verbosity for namespace `"wal"` intending to cover only
  WAL-internal targets; a target literally named `"walnut"` (or any future module whose name happens
  to start with the same substring) unintentionally inherits the same level, because
  `"walnut".starts_with("wal")` is `true`.
- TDD gap: `logging/tests/log_mask_tests.rs`'s prefix tests (`wal`/`wal_sync`/`wal_compact`) only
  exercise cases where the substring collision is the *intended* behavior (an underscore-joined
  sub-namespace) — none constructs a genuinely-unrelated-but-textually-colliding target.
- Suggested fix: either document `allows()` as an intentional raw-prefix match (not segment-aware)
  so future `ns::` constants are chosen to avoid collisions, or require the byte immediately after
  the matched prefix to be `:` (or the prefix to be the full target) before accepting the match; add
  a test for the non-`::`-delimited collision case either way.

### 1.6 — low — `try_join_next()` swallows non-panic `JoinError`s silently
- File:line: `crates/shamir-server/src/connection/request_loop.rs:240-249` (spot-verified: the drain
  loop branches only on `e.is_panic()` at :242).
- Issue: any non-panic `JoinError` (e.g. a cancelled task, or any future `JoinError` cause) falls
  through with no log line, no metric, and no client-visible signal — the dispatch task's outcome
  (and the request it was handling) simply vanishes. The workspace SUMMARY flags this as a provable
  miss inside error-handling-lifecycle territory that theme declared a "clean bill of health" (see
  §6).
- Failure scenario: if a future change introduces a code path that aborts an individual dispatch
  task (rather than the whole connection), the client that issued that request gets neither a reply
  nor an error — it just times out with no server-side trace of why.
- Suggested fix: log the non-panic `JoinError` case too (even at `debug!`/`warn!`), rather than
  silently discarding it.

### 1.7 — low — `Scheduler::shutdown()` regression test has no timeout bound on the documented race it guards against
- File:line: `crates/shamir-server/src/scheduler.rs:98-105` (doc rationale for choosing `broadcast`
  over `Notify`) vs `crates/shamir-server/tests/scheduler.rs:145-157`.
- Issue: the scheduler's own doc explains why `broadcast` was chosen over `tokio::sync::Notify`
  specifically to close the "shutdown fires before the spawned task reaches its `select!`" race —
  the same bug class CLAUDE.md calls out project-wide. The only test that shuts down immediately
  after spawn (`spawn_creates_tasks_then_shutdown_joins_them`) asserts only that `shutdown().await`
  eventually returns, with no `tokio::time::timeout` bound — if this race were reintroduced (e.g. a
  future task type using `Notify`), the regression would manifest as an indefinite hang caught only
  by nextest's 180s slow-timeout, not as a fast, readable assertion failure.
- Failure scenario: a reintroduced shutdown race hangs CI for 180 s per offender instead of failing
  in 2 s with a named assertion.
- Suggested fix: wrap the `shutdown().await` call in that test with
  `tokio::time::timeout(Duration::from_secs(2), ...)` and assert `Ok(())`.

### 1.8 — low — `safe_run`'s panic-survival contract is never actually exercised by a panicking tick
- File:line: `crates/shamir-server/src/scheduler.rs` (`safe_run` / `catch_unwind` wrapper around
  each periodic task) vs `crates/shamir-server/tests/scheduler.rs`.
- Issue: the scheduler wraps each periodic tick in a panic-catching guard so one bad tick doesn't
  kill the whole periodic task, but no test ever makes a GC/checkpoint stub `panic!()` inside a tick
  to prove the scheduler survives it and fires again on the next interval. All existing tests only
  prove each task type fires at least once under normal (non-panicking) conditions.
- Failure scenario: a refactor that drops the `catch_unwind` wrapper (or scopes it wrong) ships
  green; the first panicking tick in production kills the periodic task permanently.
- Suggested fix: add a scheduler test with a tick closure that panics on its first invocation and
  asserts a second successful invocation still occurs afterward.

### 1.9 — low — `/readyz` is never observed returning "not ready" through the real boot path
- File:line: `crates/shamir-server/src/observability.rs:13-17` (documented boolean-ready contract)
  vs `crates/shamir-server/tests/observability_http.rs` (`endpoints_return_expected_codes_and_content`).
- Issue: the existing HTTP test only asserts `/readyz` returns 200 *after* `launcher.launch().await`
  has fully completed and `mark_ready()` has unconditionally run — the "should be 503 before
  listeners are bound" half of the documented contract is never driven through an actual in-flight
  boot via HTTP; it's only indirectly inferable from `ObservabilityState::new()`'s default `false`.
- Failure scenario: a regression that flips the ready flag early (or marks ready on bind rather than
  full launch) ships green — the exact failure mode the 503 contract exists to catch is unobserved.
- Suggested fix: add a test that starts the observability HTTP server before the rest of the launch
  sequence completes (or with `mark_ready()` intentionally not yet called) and asserts a 503 from
  `/readyz` in that window.

### 1.10 — nit — Vacuous dead-code discard of `ConnectError::AuthFailed`
- File:line: `crates/shamir-server/src/connection/request_loop.rs:431` (spot-verified verbatim:
  `let _ = ConnectError::AuthFailed;`).
- Issue: `let _ = ConnectError::AuthFailed;` constructs a value purely to discard it;
  `ConnectError` is otherwise unreferenced in this file. Leftover scaffolding from an incomplete
  refactor (an error path that was meant to construct/propagate this variant but doesn't). No test
  would catch its removal; no behavioral effect — but it signals a call site whose original logic
  obligation is now silently unmet.
- Failure scenario: none at runtime; the hazard is the unmet obligation the line papers over.
- Suggested fix: either wire this into the actual error path it was meant to represent, or remove
  the line and the now-unnecessary import.

**Also verified clean under this lens:** `cursor_registry.rs`, `tx_registry.rs`, `byte_budget.rs`,
`registry.rs`'s cardinality tracking, `access_tree.rs`, `backup.rs`/`restore.rs`, `bootstrap.rs`,
`config.rs`, `server_meta.rs`, `tables_registry.rs`, `tls.rs`, `user_directory.rs`, `version.rs`,
`conn_limiter.rs`, `framer.rs`, `in_flight_guard.rs`, `push_sink.rs`, `connection_context.rs`,
`user_state_lookup.rs`, `wire.rs`, and the `server/` boot-orchestration files — dense doc comments
citing specific prior incidents (F-12, F-19, N-6, W-5, CR-A6, F-38, #439, #513, #527, …) and tests
that force real failure conditions (corrupted msgpack, Windows sharing-violation locks, hand-crafted
principal64 collisions) rather than tautologies.

## 2. concurrency-lockfree

**No findings for this theme — clean.** Every hot-path structure (subscription registry/bridge,
decode/deliver caches, cursor/tx registries, connection limiters, byte budget, request loop) uses
`scc::HashMap`/`scc::TreeIndex`/`DashMap` with `THasher`, atomics, or `ArcSwap`, with inline
justifications closing prior races (#1073, #1077, F-9/F-20 cursor reap races). The
`std::sync::Mutex`/`parking_lot::Mutex` instances that exist are all admin/DDL/boot frequency
(`ServerMetaStore`, `FjallUserDirectory`, `TablesRegistry`, `FjallAuditAppender`) and fit CLAUDE.md's
sanctioned-exception categories. No lock-across-`.await` violations, no un-acked O(N) `scc::*::len()`
on any hot path. Reviewer notes (explicitly not reportable findings, recorded for completeness):

- `tables_registry.rs:139-154` holds a `parking_lot::Mutex` across a synchronous temp-file
  write+rename (`write_atomic`) — DDL-frequency only, sanctioned category, not worth a fix.
- `logging.rs:168-172` (`set_namespace_level`) does a load-clone-with_override-store RCU without a
  CAS retry loop, so two concurrent callers can clobber each other's override — operator-facing
  SIGHUP knob, not a data-path concern.
- `supervisor.rs:176-179` (`active_count`) uses `scc::HashMap::len()` (O(N)) with the required
  `#[allow(clippy::disallowed_methods)] // O(N) ack: test/telemetry, not hot path` comment —
  correctly acked.
- `cursor_registry.rs:705-708` (`by_session_len`) uses `DashMap::len()` (not on the banned list) and
  is `#[cfg(test)]`-gated — correctly scoped.

*(Workspace-level coverage caveat: the sweep's SUMMARY flags this crate's triple "no findings" as
statistically the most suspicious zero in the corpus for a 113-file crate that also produced the one
server critical; the perf/concurrency zeros are judged defensible given the crate's audit density,
but see §6 for the error-handling zero's provable counter-example — finding 1.6.)*

## 3. security-crypto

**The connection/handshake/auth boundary is unusually well hardened:** constant-time HMAC
verification (`hmac::Mac::verify_slice`, `subtle`-backed), explicit latency padding closing the
SCRAM timing oracle, per-pair exponential lockout backoff, fail-closed bootstrap-token metadata
handling, a documented pre-auth frame-size ceiling (`MAX_PRE_AUTH_FRAME`) before any Argon2id work,
and a manifest-path traversal guard on backup/restore. No `unsafe` blocks anywhere in the crate.
Findings are secret-hygiene gaps around the replication-follower password plus one known/tracked
TOFU gap:

### 3.1 — medium — `ReplicationConfig::replicator_password` is a plain `String` held for the server's lifetime, reachable via `Debug`
- File:line: `crates/shamir-server/src/config.rs:133` (field, spot-verified:
  `pub replicator_password: Option<String>`), `config.rs:71` / `:120` (`#[derive(Debug, ...)]` on
  `Config` and `ReplicationConfig`); propagation: `server_launcher.rs:500` → `prod_factory.rs:58/67`
  (`Arc<str>` in `ReplicatorCreds`, cloned into every `LazyWireSource`).
- Issue: the follower-replication password is deserialized straight into `Option<String>` and stored
  on `Config` for the whole process lifetime. Unlike every other credential path in this crate
  (`bootstrap.rs`'s `Zeroizing<Vec<u8>>`, `db_handler/admin.rs`'s `Zeroizing` password buffer,
  `access_tree.rs`'s `Zeroizing`), it is never wrapped in `Zeroizing`/`SecretString` until
  `prod_factory.rs:112` builds the outbound `ConnectOptions` — by which point it has already been
  copied through `Config` (Debug-derived), `ReplicationConfig` (Debug-derived), and `Arc<str>`
  (immutable, cannot be zeroized even if desired) across however many subscriptions exist.
- Failure scenario: (a) a future `tracing::debug!(?config)` / `anyhow::Context` chain that includes
  the `Config`/`ReplicationConfig` value prints the plaintext replicator password into
  logs/telemetry — the same leak class the codebase explicitly defends against elsewhere
  (`observability.rs`'s M5 gate on `/metrics`, HMAC-only user-hash logging in `handshake.rs`).
  (b) the password lives in ordinary (non-`mlock`, non-zeroizing) heap memory for the server's
  entire uptime; a heap/core dump or swap write captures it in plaintext long after the credential
  was last needed, unlike the SCRAM bootstrap/admin paths which minimize the plaintext window via
  `Zeroizing`.
- Suggested fix: change `replicator_password` to a `Debug`-redacted wrapper (e.g. a `SecretString`-
  like newtype printing `"<redacted>"`, mirroring `shamir_query_types::auth::SecretString` already
  used in `db_handler/admin.rs:103`), and thread `Zeroizing`/secret-typed values through
  `ReplicatorCreds` instead of `Arc<str>`.

### 3.2 — low — Replication client uses trust-on-first-use with no leader-key pinning *(known, tracked)*
- File:line: `crates/shamir-server/src/replication/prod_factory.rs:113-116` (spot-verified:
  `accept_new_host: true, trusted_pin: None` with the "future work (#388)" comment).
- Issue: `LazyWireSource::connected()` unconditionally sets TOFU when a follower dials its leader —
  no persisted pin of the leader's TLS/identity key across reconnects. The code's own comment flags
  this honestly ("Persisting a leader pin is future work (#388)"), so it is a known, tracked gap
  rather than an oversight.
- Failure scenario: a network-positioned attacker who can intercept the very first follower→leader
  connection (or any reconnection after a state reset) presents a different TLS identity and the
  follower accepts it silently — MITM of the replication stream (read access to replicated data, or
  injection of a spoofed leader's stream).
- Suggested fix: no action beyond what's already tracked (#388); flagged for visibility as a
  legitimate MITM surface on the replication data path.

### 3.3 — nit — `bootstrap_password` accepted as a CLI argument
- File:line: `crates/shamir-server/src/main.rs:56` (spot-verified: `bootstrap_password:
  Option<String>` with `#[arg(long, ...)]`).
- Issue: the bootstrap superuser password can be supplied via `--bootstrap-password <PASSWORD>` on
  the command line. Process command lines are visible to other local users via
  `/proc/<pid>/cmdline` (Linux) or process-listing tools, and typically persist in shell history.
- Failure scenario: a co-resident local user or monitoring agent captures the plaintext bootstrap
  password from `ps` output or shell history at server-start time.
- Suggested fix: common, low-severity CLI ergonomics tradeoff — the tool already defaults to a safer
  random-token mode when the flag is omitted, and the `--bootstrap-token-path` doc already
  recommends tmpfs. No change required unless the project wants to push operators toward an env-var
  or stdin-prompt alternative.

**Also verified clean under this lens:** the SCRAM/Argon2id handshake, HMAC "did-you-mean-it"
destructive-op gating, TLS material lifecycle, session resumption ticket handling, and
pre-auth frame-size/rate-limit/lockout defenses — all consistent with the documented spec sections
they cite.

## 4. performance-hotpath

**No findings for this theme — unusually mature.** All 113 `.rs` files read plus an independent
second-pass agent scan found no hidden O(N)/O(N²) hot-path defect, no
per-iteration-allocation-that-should-be-hoisted, and no unbounded in-memory buffer. Verified
highlights (recorded for calibration parity):

- **Subscription fan-out** (`subscriptions/decode_cache.rs`, `deliver_cache.rs`,
  `target_match.rs`): migrated off `DashMap`/linear-scan to `scc::TreeIndex` with CV-first keys for
  O(log N) lookups and O(evicted + log N) bounded range-remove eviction; per-bridge `TargetIndex`
  built once at subscribe time replaces O(T) scans. `DeliverMode::Batch`/`Call` re-executing a
  per-subscriber query (`reactive.rs`) is inherent to bind-variable semantics, not a regression.
- **Connection request loop** (`connection/request_loop.rs`, `framer.rs`): back-pressure via
  `Semaphore` + bounded `mpsc`; `encode_prereserved`/`write_frame_prereserved` avoid the extra
  memcpy of naive length-prefixing.
- **Cursor pagination** (`db_handler/cursor_handlers.rs`, `cursor_registry.rs`): every retry loop
  capped by `cursor_limits.max_cursor_page_size`; the one full-scan probe
  (`order_by_column_contains_null`) is a documented one-time `create_cursor` cost, not per-page.
- **Registries/limiters** (`tx_registry.rs`, `cursor_registry.rs`, `conn_limiter.rs`,
  `subscriptions/registry.rs`): live-counts are `AtomicUsize`/`AtomicU32` mirrors at each mutation
  site (never `.len()` scans); entries pruned back to zero on release.
- **Byte budget** (`byte_budget.rs`): lock-free CAS-loop fast path, `Notify`-parking only on
  contention, upfront-reserve-then-shrink avoids double-acquire.
- **User directory / backup / replication / scheduler**: hot-path ticket lookup is an O(1)
  in-memory cache; backup hashing streams through a fixed buffer (verified, not assumed); pull loop
  bounded by `DEFAULT_PULL_LIMIT = 1000` with idempotent bookmark advancement and exponential
  backoff; supervisor catalogue reconciliation re-reads admin-managed tables on a 10s tick, not
  per-request; GC/metrics work correctly off the request hot path.

*(Same workspace coverage caveat as §2 applies — see the note at the end of §2.)*

## 5. api-wire-protocol

**The wire surface is unusually well-documented:** every positional-msgpack struct carries explicit
"always present on the wire" / field-order-matters comments; version negotiation (`version.rs`)
cleanly separates the handshake-protocol axis (`u8`) from the query-language axis (`u32`). No
builder-only query-construction violations — the one `serde_json` use is a test asserting against an
already-produced JSON string (documented exception), and the subscription `EventData`/`KeysData`
payload structs are server-push DTOs, not query construction.

### 5.1 — medium — `/info` endpoint documented as human-readable but returns raw msgpack
- File:line: `crates/shamir-server/src/observability.rs:26` (doc: "pretty-printed server info for
  curl-debugging by an operator", spot-verified) vs `:583-602` (`info_handler` serializes `InfoBody`
  via `rmp_serde::to_vec_named` at :595 and sets `Content-Type: application/msgpack` at :598);
  repeated in `config.rs:155` cross-reference and the crate `//!` header; the crate's own
  integration test (`tests/observability_http.rs:172-181`) decodes the response with
  `rmp_serde::from_slice` and labels it "msgpack `/info` endpoint" — directly contradicting the doc.
- Issue: the module doc promises curl-readable output; the implementation returns opaque binary,
  while every sibling endpoint (`/healthz`, `/readyz`, `/metrics`) returns plain text.
- Failure scenario: an operator runs `curl http://127.0.0.1:9090/info` (the exact use case the doc
  advertises) and gets raw binary msgpack dumped to their terminal — no debugging value, actively
  confusing.
- Suggested fix: either (a) emit human-readable output (JSON via `serde_json` is the documented
  exception for non-query surfaces) matching the doc's promise, or (b) fix the doc in
  `observability.rs` (and the `config.rs` cross-reference) to say "msgpack-encoded" and stop
  promising curl-friendliness.

### 5.2 — low — `server_query_version` wire field is `u8` but the version constant is `u32`
- File:line: `crates/shamir-server/src/connection/handshake.rs:152` and `:640` (both
  `CURRENT_QUERY_LANG_VERSION as u8`; synthesis spot-check — the lens file cited :151/:639, the
  `as u8` casts sit at :152/:640); `version.rs:57` (`pub const CURRENT_QUERY_LANG_VERSION: u32 = 2`).
- Issue: the constant is `u32` specifically because, per `version.rs`'s own doc, "the query-language
  version is much more likely to evolve… easier to bump for a long time without overflowing." Both
  handshake response paths (`wire::AuthOk::server_query_version` and
  `wire::ResumeOkWire::server_query_version`, both `u8` in `connection/wire.rs`) narrow it via
  `as u8`. The moment the constant crosses 255 the cast silently wraps instead of failing to compile
  or panicking, defeating the stated rationale — while the request-side `query_version: u32` field
  (`DbRequest::Execute`) has no such ceiling, so the advertised max-supported version a client sees
  over the handshake could silently disagree with what the dispatch version check accepts.
- Failure scenario: a future bump past 255 wraps (256 → 0); clients gating v2+ behavior on the
  advertised field silently downgrade to v1 (or misread the wrapped low value as "server predates
  negotiation," per the `0` sentinel doc on `AuthOk::server_query_version`), even though the server
  actually supports the newer version.
- Suggested fix: either narrow `CURRENT_QUERY_LANG_VERSION` (and the supported-set) to `u8`
  explicitly, documenting the intentional `u8`-bounded ceiling, or widen both wire fields to
  `u16`/`u32` with a fallible conversion (`u8::try_from`, `tracing::warn!` on overflow) at the
  population site instead of silent `as u8` truncation.

### 5.3 — nit — `check_destructive_hmacs` re-derives the HMAC key on every destructive-op sweep across `execute` and `tx_execute`
- File:line: `crates/shamir-server/src/db_handler/admin.rs:637-653`; call sites `handler.rs:549` and
  `tx_handlers.rs:119`.
- Issue: the lazy-derivation closure memoizes the session HMAC key *within* one
  `check_destructive_hmacs` call, but both `execute` and `tx_execute` call it independently per
  request, so repeated small destructive batches re-derive `session.hmac_key()` once per request
  rather than caching session-wide. Not a wire-correctness issue (cheap HKDF, not Argon2id); the
  memoization pattern just reads as though it solved a cross-call cost it doesn't.
- Failure scenario: none observed — minor efficiency/readability note only.
- Suggested fix: no action required; documented so a future reviewer doesn't assume `key_opt`
  amortizes across requests.

## 6. error-handling-lifecycle

**No findings for this theme — a clean bill of health as filed.** Verified strengths (kept for
calibration parity): every fallible production path returns `Result<T, thiserror-derived E>`;
`anyhow`/`Box<dyn Error>` confined to `main.rs`/CLI boundary; resource lifecycle (file locks,
redb/fjall handles, background tasks, MVCC snapshot guards, TLS acceptors) handled with thorough
RAII discipline naming the exact race each cleanup step closes (`backup.rs`/`restore.rs` staged
temp-dir cleanup + atomic-swap rollback with dedicated fault-injection tests for both swap-failure
sub-cases, `tx_registry.rs`/`cursor_registry.rs` reaper-driven RAII abort,
`server_handle.rs::shutdown` ordered drain); `subscriptions/bridge.rs`'s `let _ = try_push(...)` is
the documented at-most-once push contract backed by `PushKind::Gap`; `db_handler/admin.rs:572`'s
`let _ = finalize_change_password(...)` discards a `u64` timestamp, not a `Result` (false lead,
confirmed against the callee).

Two synthesis notes:

- **Coverage caveat with concrete counter-evidence (workspace SUMMARY):** the correctness lens
  filed `try_join_next()` silently swallowing non-panic `JoinError`s (finding **1.6**) inside exactly
  the territory this theme declared clean — `request_loop.rs:240-249` is an error-handling defect by
  definition. The theme's `.expect()` audit itself was sound: all sites
  (`runtime.rs:81`, `doctor.rs:238`, `access_tree.rs:176`, `cursor_handlers.rs:1331`,
  `server_launcher.rs:404`) are invariant-proven, not reachable from untrusted input.
- **Boot-time cleanup via drop is correct here, not a gap:** `server_launcher.rs::launch`'s early-
  `?` unwinds run `Drop` on `data_dir_lock` (:114-133), `ServerMetaStore`, `FjallUserDirectory`,
  `FjallConsumedCounters`, `FjallAuditAppender` deterministically — the correct RAII-first pattern.
- `request_loop.rs`'s teardown (:410-431) is a reference-grade ordered-release sequence: `close_all()`
  before dropping `conn`, `join_set.abort_all()` + drain before dropping `tx`, conditional writer
  await, and dispatch-panic converted to connection teardown via `is_panic()`.

## 7. style-claude-md

**Largely disciplined:** every `mod.rs` (crate root `lib.rs`, `connection/`, `db_handler/`,
`replication/`, `server/`, `subscriptions/`, every `tests/mod.rs` manifest) is re-export-only with no
logic; the `tests/` directory layout (one dir, topic-split files, `mod.rs` manifest, wired via
`#[cfg(test)] mod tests;`) is followed everywhere including nested cases (`logging/tests/`,
`db_handler/tests/`, `replication/tests/`, `subscriptions/tests/`); no inline `#[cfg(test)] mod
tests { ... }` bodies anywhere in the crate. Two real gaps:

### 7.1 — medium — `use shamir_query_types::hmac as canon;` repeated 4× mid-function instead of hoisted to file top
- File:line: `crates/shamir-server/src/db_handler/admin.rs:119,294,375,642` (spot-verified: exactly
  these four occurrences).
- Issue: the same import appears inside four separate function bodies (`create_scram_user`-style
  handler, `set_superuser`, a third admin op, and the `check_destructive_hmacs`-adjacent helper at
  :642). None are `cfg`-gated, and none collide with another `hmac`-named trait at the file's top
  (lines 1-20 have no conflicting `hmac` import) — so neither of CLAUDE.md's two documented
  exceptions ("cfg-gated bodies", "trait collision") applies. Plain avoidable duplication.
- Failure scenario: not a runtime bug — a maintainability/consistency issue. A future editor adding a
  5th HMAC-gated admin op has three prior mid-body instances to copy from, reinforcing the drift.
- Suggested fix: move `use shamir_query_types::hmac as canon;` to the file's top-level `use` block
  (next to the existing `use shamir_query_types::auth::SecretString;` at :16) and delete the four
  local copies. *(The workspace SUMMARY's P2 sweep list already includes shamir-server in the
  imports-at-top sweep.)*

### 7.2 — low — `config.rs` bundles 16 public types in one file
- File:line: `crates/shamir-server/src/config.rs` (850 lines): `Config`, `ReplicationConfig`,
  `ObservabilityConfig`, `AuditConfig`, `SecurityConfig`, `TxLimitsConfig`, `CursorLimitsConfig`,
  `QueryLimitsConfig`, `ConnectionSecurity`, `LoggingConfig`, `KdfConfig`, `ListenerConfig`,
  `ListenerKind`, `ProfileKind`, `TlsConfig`, `ConfigError` (at :72, :121, :157, :190, :219, :276,
  :305, :355, :437, :486, :532, :545, :569, :579, :596, :605).
- Issue: CLAUDE.md's "one file = one primary export" allows a "closely-coupled group", and these do
  form a single nested config-schema tree — the strongest argument for the allowance — but 16
  top-level public types stretches it further than any other file in the crate (next-largest
  multi-type file, `user_directory.rs`, has 3). `git blame` on this file mixes unrelated concerns
  (a TLS profile rename touches the same file as a cursor-limits default change).
- Failure scenario: N/A (structural/maintainability, not a runtime defect).
- Suggested fix: if the file grows further, split along the doc comment's existing "Schema"
  boundaries (e.g. `config/listener.rs` for `ListenerConfig`/`ListenerKind`/`TlsConfig`/`ProfileKind`,
  `config/limits.rs` for the three limits types), keeping `config.rs` as `Config` + `ConfigError` +
  `from_file`/`validate`. Nit-to-low priority; not urgent.

**No other findings for this theme:** imports-at-top elsewhere, `mod.rs` re-export discipline,
test-directory layout, and comment discipline are all consistent with CLAUDE.md (the remaining
mid-body `use` sites in `bootstrap.rs`, `framer.rs`, `main.rs`, `runtime.rs`, `service.rs`, `tls.rs`,
`server_launcher.rs:115/127`, `tx_registry.rs`, `subscriptions/bridge.rs`, `subscriptions/payload.rs`,
`subscriptions/decode_cache.rs`, `subscriptions/deliver_cache.rs`, and `tests/restore_tests.rs:540`
are all genuinely `cfg`-gated or scoped to single non-reusable helper blocks).

---

## Finding counts

| Severity | Lens-tagged findings | Deduped distinct defects | Finding numbers (all singleton dedup groups) |
|---|---|---|---|
| critical | 1 | 1 | 1.1 (read-only-replica tx write bypass) |
| high | 1 | 1 | 1.2 (dead follower-loop registry entries) |
| medium | 6 | 6 | 1.3 (bridge-task leak race), 1.4 (Hello busy-loop), 1.5 (log-mask prefix), 3.1 (replicator_password hygiene), 5.1 (`/info` doc vs msgpack), 7.1 (imports-at-top) |
| low | 7 | 7 | 1.6 (swallowed JoinError), 1.7 (unbounded shutdown test), 1.8 (`safe_run` panic untested), 1.9 (`/readyz` 503 untested), 3.2 (TOFU leader pin, tracked #388), 5.2 (u8/u32 version width), 7.2 (config.rs 16 types) |
| nit | 3 | 3 | 1.10 (dead `ConnectError` discard), 3.3 (CLI bootstrap password), 5.3 (HMAC re-derivation) |
| **total** | **18** | **18** | 1 critical · 1 high · 6 medium · 7 low · 3 nit |

Dedup note: unlike the exemplar crates, this crate's seven lens reports contain **no cross-lens
duplicate filings** — each of the 18 lens-tagged findings is a distinct defect filed exactly once,
so the deduped count equals the raw count (matches the workspace SUMMARY's per-crate row: 1/1/6/7/3
= 18). The concurrency/performance/error-handling "no findings" reports each documented their
verification evidence; the workspace SUMMARY nonetheless flags the triple zero as the corpus's most
suspicious coverage gap, with the one provable miss (1.6) filed under correctness but living in
error-handling territory (see §6), and recommends a perf spot re-check.

Cross-crate context (tracked in the workspace SUMMARY, not counted here): shamir-server also appears
in three *pair* findings — positional handshake frames mirrored as independent struct definitions
with shamir-client (med), `BROWSER_CHANNEL_BINDING` re-hardcoded at consumption sites with
shamir-transport-ws (low), and the duplicated normative loopback predicate with
shamir-transport-tcp (low).

## Fix Plan

**P0 — before anything else ships from this crate**
1. **Close the read-only-replica tx write bypass (CRITICAL).** Mirror the `handler.rs:529-541`
   `NodeMode::ReadOnly` write-rejection block into `tx_execute()` (and confirm `tx_begin`/`tx_commit`
   can never stage/apply a write); write the Red test first per CLAUDE.md TDD — a
   `node_mode_tests.rs` case running `TxBegin`/`TxExecute` (write op) against a `ReadOnly` handler
   and asserting `code: "read_only_replica"`. The current suite is documented as covering `execute()`
   only. Closes **1.1**.
2. **Restore follower-supervisor resubscription liveness.** On every follower-loop exit path in
   `supervisor.rs:286-337`, call `registry.remove_sync(&sub_name)` (or a shared "mark dead" helper)
   so `reconcile()` can restart a repaired subscription; add the regression test that flips
   `resync_required` → `active` and asserts a fresh loop spawns (replacing the current
   self-admitted workaround at `supervisor_tests.rs:398-401`). Closes **1.2**.

**P1 — soon**
3. **Fix the `reserve_pending → spawn → attach_handle` vs `close_all` race** (generation-token/short
   lock across attach, or caller-side abort when `attach_handle` reports "slot gone") plus a
   `close_all`-during-subscribe test. Closes **1.3**.
4. **Add backoff to the follower loop's `Hello` arm** (`follower_loop.rs:278-285`) — one
   `sleep_backoff` call. Closes **1.4**.
5. **Make `LogMask::allows` segment-aware** (require `::` boundary or exact-target match) or document
   raw-prefix semantics as intentional, with a colliding-target test either way. Closes **1.5**.
6. **Replication password secret hygiene:** Debug-redacted type for
   `ReplicationConfig::replicator_password`, `Zeroizing`/secret-typed threading through
   `ReplicatorCreds` instead of `Arc<str>`. Closes **3.1**.
7. **Align `/info` doc and behavior:** emit human-readable output or reword the doc in
   `observability.rs` (+ `config.rs:155` cross-reference). Closes **5.1**.
8. **Guard the query-version handshake width:** explicit `u8` ceiling or widened wire field with a
   fallible conversion replacing both `as u8` casts (`handshake.rs:152, :640`). Closes **5.2**.
9. **Close the TDD gaps on documented contracts:** `tokio::time::timeout` bound on the scheduler
   immediate-shutdown test; a panicking-tick `safe_run` survival test; a `/readyz`-503-before-ready
   HTTP test. Closes **1.7, 1.8, 1.9**.
10. **Hoist the 4× mid-function `hmac` import in `admin.rs`** to the file top (one-sweep, also in
    the workspace P2 imports sweep). Closes **7.1**.

**P2 — backlog**
11. Log non-panic `JoinError`s in `request_loop.rs`'s drain loop. Closes **1.6**.
12. Persist the leader TLS/identity pin for replication (land with tracked **#388**). Closes **3.2**.
13. Remove or wire the `let _ = ConnectError::AuthFailed;` scaffolding. Closes **1.10**.
14. Optional hardening/doc: env-var/stdin alternative for `--bootstrap-password`; session-wide HMAC
    key caching only if a profiling pass ever justifies it; `config.rs` split if it grows further.
    Closes **3.3, 5.3 (no action required), 7.2**.

</details>
