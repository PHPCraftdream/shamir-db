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
