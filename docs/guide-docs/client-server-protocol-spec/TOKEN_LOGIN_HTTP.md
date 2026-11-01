# TOKEN-AUTH0: Login/password sign-in over HTTP

**Status:** normative contract of a feature that is not implemented yet. These routes verify a SCRAM client proof and issue an ordinary reusable TOKEN_API bearer token—not an HTTP session, cookie, Basic credential or database session. The password never crosses the network. No shared SCRAM `TransportKind`/`BindingMode` enum or AUTH_PROTOCOL messages change. Binding mode `0x03` is protocol-local and never parsed through `BindingMode`. Publish test vectors before implementation.

## 1. Endpoints and exact wire contract

The `kind:http` routes are `POST /shamir/v1/auth/begin` and `POST /shamir/v1/auth/finish`. They accept no API bearer token and create no database session. HTTP/1.1 only; no pipelining or compression. Content-Type exactly `application/msgpack`, no parameters. Any Content-Encoding header, including an empty value, returns 415; absence is allowed. Each body is one MessagePack map, not an array. Reject duplicate, missing, unknown, mistyped keys and trailing bytes. Integer fields accept positive fixint and applicable uint8/16/32/64 markers when the value fits range; reject signed integer markers, including nonnegative signed markers. Accept non-shortest unsigned encodings. Transcript encoding is fixed-width. `bin(N)` is MessagePack binary exactly N bytes; strings are strict UTF-8. Every response MUST include `Content-Type: application/msgpack`, `Cache-Control: no-store`, `Pragma: no-cache`, `X-Content-Type-Options: nosniff`, and `Connection: close`. No CORS, cookies or `WWW-Authenticate: Basic`. TLS responses MUST include HSTS (TOKEN_API §9); class 0x02 proxy adds HSTS at the external edge.

### 1.1 Begin request

Exact required request keys:

```text
{
  "protocol_version": u8 (=1),
  "user": str,
  "client_nonce": bin(32),
  "binding_mode": u8 (=0x03),
  "token": {
    "label": str,
    "mode": "read_only" | "read_write",
    "channels": "rest" | "ws" | "both",
    "databases": null | [str]
  }
}
```

`user` is 1..255 UTF-8 bytes. AUTH_PROTOCOL §15.3 UsernameCaseMapped and NFC (Unicode 15.1); the SDK MUST apply case mapping and NFC locally before sending. The server rejects any noncanonical username with 400 and MUST never normalize it; this is a client bug, not authentication failure. Client nonce is 32-byte nonzero CSPRNG output; the server MUST reject an all-zero nonce with 400. Label is 1..128 UTF-8 bytes, no C0/DEL, byte-preserved, and compared exactly. Databases: array length 1..4 when supplied (empty array is 400), each 1..255 bytes, NFC, strictly bytewise sorted, unique and valid database syntax; total scope encoding <=512 bytes or 400. All keys are required on the wire; no defaults. SDK recommended scope is label `http-login`, read_only, channels `rest`, databases null. Explicit read_write requires the approved global flag.

Begin success is 200 with exact challenge keys:

```text
{
 "protocol_version": u8 (=1), "binding_mode": u8 (=0x03),
 "security": u8,
 "listener_id": bin(16), "handshake_id": bin(32), "salt": bin(16),
 "kdf": "argon2id", "memory_kb": u32, "time": u32, "parallelism": u32,
 "argon2_version": u8 (=0x13), "server_nonce": bin(32)
}
```

`listener_id` is a stable random configured identity for the logical listener, unique and stable across restart, never derived from Host/forwarding/client. Client derives expected security from configured URL and out-of-band deployment policy: HTTPS direct expects 0x01; HTTPS through configured TLS-terminating proxy expects 0x02; HTTP expects 0x00 only for loopback. Reject mismatch; never trust challenge class alone. Optional out-of-band listener pin MUST match if configured. TLS cert/hostname authenticates host only; listener ID separates transcript, not identity. Never follow cross-scheme/host/port/listener redirects. Requests are not TLS-session-bound; relay remains possible.

Listener security classes and login/plaintext startup validation are defined solely in §5.1; bearer behavior is unchanged.

`login_enumeration_hardening: bool` defaults true. It is HTTP-only and does not upgrade tuples or change native listeners. `kdf_override` is forbidden on login listeners. At enablement audit global/default and bootstrap-assigned listener KDF configuration; with hardening enabled, any account whose stored tuple differs from the global default is individually ineligible. Do not impose global login shutdown or require native bootstrap configuration changes. Any raising of `kdf_params_current` can make every existing account whose stored tuple differs ineligible for HTTP login; check the pre-enable ineligible-user count and require explicit operator acceptance. Hardening false allows known client-usable stored tuple verified from metadata, accepting KDF-metadata leak. Other unsupported tuples are ineligible in either setting. Count ineligible users at enablement and in protected operator metric with no IDs. Eligible/ineligible/unknown branches do equivalent snapshot work; pad BEGIN 50–75 ms independent of source/gate/state-existence. Ineligible and unknown users use deterministic fake salt/stored_key/server_key derived with the existing fake-blob rules and current default KDF; eligibility remains false and issuance is forbidden even if proof math matches the fake verifier. Native TCP/WS still leaks stored KDF metadata; HTTP helps only if HTTP is sole exposed auth surface. Fake salts may change on server-secret rotation and real salts on password change; accepted leakage. Password change keeps stored tuple; AUTH_PROTOCOL §13.4 says recreation is required, losing user ID, tokens and resource ownership. Do not claim password change upgrades KDF.

### 1.2 Finish request and success

Exact finish request: `{ "protocol_version": u8 (=1), "handshake_id": bin(32), "client_proof": bin(32) }`. Scope/label are frozen and proof-bound (§2). Success exact keys:

```text
{
 "protocol_version": u8 (=1), "server_signature": bin(32),
 "token": str, "id": bin(16), "label": str, "owner": str,
 "mode": "read_only" | "read_write", "channels": "rest" | "ws" | "both",
 "databases": null | [str], "created_at_ns": u64, "expires_at_ns": u64
}
```

The returned `id` is the 16-byte token ID from `CreateApiToken`; it MUST match the decoded token ID and lets the holder revoke by ID. It does not change `T`. It adds 21 MessagePack bytes (3-byte key plus 2-byte bin header plus 16-byte value); the map remains fixmap with 11 keys. Token string remains 71 characters. Staging buffer is 4,096 bytes.

Client MUST constant-time verify `server_signature=HMAC-SHA256(server_key,T)` before accepting/displaying/using token; mismatch means discard token and fail. It confirms verifier possession, not server identity; no Ed25519 app signature/pin. TLS cert validation is HTTPS identity baseline; native SCRAM pin guarantee does not apply. Server cannot know whether client verified. Token is reusable until revoked/expired/scope denied; store only TOKEN_API MAC record, never secret. Never put token in error/header/URL/redirect.

## 2. Exact transcript, encodings and proof

`U8/U16/U32/U64` are fixed-width unsigned big-endian; overflow rejected. `B(x)` is raw bytes; no terminator/implicit length. UTF-8 strict; user canonical normalized UTF-8. Transcript excludes MessagePack encoding/map order.

```text
scope_bytes = U16(label_len) || label
           || mode_byte                 // 0x00 read_only, 0x01 read_write
           || channels_byte             // 0x00 rest, 0x01 ws, 0x02 both
           || U8(database_count_byte)       // 0xFF null/all; otherwise 1..4
           || for each database: U16(database_len) || database_utf8
```

`databases` is null (all) or a nonempty allowlist; count 1..4; empty 0x00 is invalid and returns 400. Scope bytes, including tags and length fields, MUST be <=512 bytes; null uses count 0xFF and no database entries. Label is 1..128 UTF-8 bytes; each database is 1..255 bytes, NFC and strictly bytewise sorted. Reject a maximum field-length sum exceeding 512 with 400. Begin body is at most 4,096 bytes.

See §5.1 for the sole normative `external_tls` profile.

```text
T = ASCII("SHAMIR-HTTP-LOGIN-v1")  // 20 bytes, no NUL
 || U8(protocol_version)          // 0x01
 || U8(binding_mode)              // local 0x03
 || U8(security)                  // 0x00, 0x01, 0x02
 || B(listener_id)                // 16
 || B(handshake_id)               // 32
 || U16(user_byte_len) || user    // 1..255 UTF-8 bytes
 || B(client_nonce)               // 32
 || B(server_nonce)               // 32
 || B(salt)                       // 16
 || U32(memory_kb) || U32(time) || U32(parallelism)
 || U8(argon2_version)            // 0x13
 || B(scope_bytes)
```

Fixed fields total 166 bytes; exact length `166 + user_byte_len + scope_bytes_len`. Scope 6..512, user 1..255; T length 173..933.

Client MUST reject locally before Argon2 (and send no finish) any tuple outside version 0x13, memory 19,456..262,144 KiB, time 2..8 iterations, parallelism 1..8. Hardened clients SHOULD require exact configured tuple. `time` is iteration count, not seconds. Key schedule: `salted_password=Argon2id(password,salt,memory_kb,time,parallelism,0x13)`; `client_key=HMAC-SHA256(salted_password, ASCII("Client Key"))`; `server_key=HMAC-SHA256(salted_password, ASCII("Server Key"))`; `stored_key=SHA256(client_key)`; `client_signature=HMAC-SHA256(stored_key,T)`; `client_proof=client_key XOR client_signature`. Server verifies stored-key proof and returns `HMAC-SHA256(server_key,T)`. Hashes/HMACs 32 bytes; constant-time compare. T binds endpoint/domain, protocol/version/mode/security/listener/id/user/nonces/salt/KDF/scope. Prevents alteration/reuse, not live relay. Separate builder and vectors required; existing `buildAuthMessage` MUST NOT be reused.

## 3. Pending lifecycle, admission and resource limits

IDs are OS-CSPRNG 32 bytes, collision-checked; retry three generations then 503. Server nonce is OS-CSPRNG 32 bytes, nonzero and distinct from client nonce; retry three then 503. Volatile process-local state; restart discards. Store verifier keys (real/fake), never password, token secret or proof.

Pending payload <=1,536 bytes; charged entry <=2,048 bytes including map/key/index/allocator overhead. Store scope once, <=512 bytes, and only bounded identity snapshot <=64. BEGIN retains UID, `stored_key`, `server_key` (needed for finish signature), threshold and eligibility; no roles vector. Roles are not retained at BEGIN; the ordinary TOKEN_API `roles` snapshot is taken at token creation. Immediately before and after durable creation, recheck UID, stored key, threshold and privilege; compensate-revoke on change.

Admission tables are bounded and charged as specified below; no unbounded shared limiter or uncharged allocation.

**Deadlines and framing.** Request headers are at most 8,192 bytes and 64 fields. Header deadline is 5 seconds, body deadline 3 seconds, total request deadline 120 seconds, response-write deadline 5 seconds. Begin body is at most 4,096 bytes; finish body at most 256 bytes. Reject requests containing both Transfer-Encoding and Content-Length; reject unsupported transfer codings and malformed framing before body. Every terminal path (success, error, timeout, cancellation, I/O failure or shutdown) releases its input slot exactly once.

**Challenge-credit gate and input slots.** Dedicated HTTP-only ARRIVAL and CREDIT tables each contain 4,096 charged 128-byte entries. Both procedures are specified in §4; retain tier values here: IPv4 /24 10/10, /16 160/160; IPv6 /64 10/10, /56 40/40, /48 160/160 per minute for CREDIT and same tiers per second for ARRIVAL. Source input-slot caps: IPv4 /24 16, /16 64; IPv6 /64 16, /56 64, /48 64 (pending-entry caps are separate, see below). Full global input pool returns 503 `server_busy` with `Retry-After: 1`; no state is allocated and existing pending state is untouched. Debits are not refunded. Both hierarchies enforce per-parent key quotas; depletion returns 429. Missing child state has its own-parent charge, never an unrelated source. Each applicable distinct hierarchy bucket is debited once (own-parent charge); CREDIT debit across tiers is atomic, with no partial debit on depletion. While its parent bucket is resident, one /48 or /16 permits at most 160 initial credits plus 160/minute (9,760 in the first hour); BEGIN and FINISH both spend credits, so this is an upper bound on BEGINs, not completed logins. Table churn can reset the bound. No username-dependent state is allocated before canonical parse and pair gate. Native admission does not use HTTP credit tables.

Each /48 permits at most 32 /56 children and 32 /64 children (65 hierarchy keys including parent); each IPv4 /16 permits 32 /24 children (33 including parent). Thus one /48 uses at most 65/4,096 ~=1.59% of a table; one /16 at most 33/4,096 ~=0.81%. Mixed child counts/quota metadata live in charged entries. Excess child uses its own parent bucket. Tables reclaim a full bucket with no outstanding handshake; otherwise evict fullest then least-recently-used eligible entry. Parents with outstanding descendants are pinned. Transfer residual debt only to that source's resident parent before child removal. Evict only a root's unpinned subtree; if no safe candidate, drop only that source's accounting, allowing fresh burst. Never transfer across unrelated sources. Churn can weaken per-source limits and evict honest sources, but cannot globally deny admission through a shared bucket. At most 3,072 entries can be pinned (1,024 pending × three hierarchy entries).

Global pending entries cap 1,024; per-source pending caps: IPv4 /24 16, /16 256; IPv6 /64 16, /56 64, /48 256. Concurrent input-slot caps: /24 and /64 16, /56 64, /48 and /16 64. Thus one /16 or /48 can occupy at most 64/1,024 = 6.25% of global input slots (16 parent prefixes are needed to fill the pool); /24 or /64 16/1,024=1.5625%; /56 64/1,024=6.25%. The same fair-share eviction principle as for pending entries does not apply to slots (a slot is released at its terminal path within the 3 second body deadline); unlimited Sybil prefixes can still exhaust the pool, a disclosed residual. Each pending entry is 2,048 charged bytes. Fairness principals are /16 or /48. At global pending capacity, choose uniformly at random with CSPRNG from UNCLAIMED entries whose principal occupancy is at least ceil(1,024/active-principals); claimed entries never evicted. Candidate bookkeeping is charged within source indexes. No candidate means 429. q attacker and h honest principals target q/(q+h) of 1,024 slots, rounded up at threshold; fair-share target, not deterministic guarantee. For M eligible entries, a particular entry is selected with probability 1/M; survival over k selections is (1-1/M)^k only while eligible set/size remain unchanged. An attacker with A eligible entries takes an eviction with probability A/M. Unlimited Sybil principals can arbitrarily degrade availability.

**Exact memory budget.** Pending payload/map/index: 1,024 × 2,048 = 2,097,152 bytes. ARRIVAL and CREDIT: each 4,096 × 128 = 524,288. Pending source index: 1,024 × 64 = 65,536; request/source task index: same. Five charged prefix indexes: 1,024 × 64 × 5 = 327,680. Fixed table/map reserve: 65,536 bytes. LockoutStore: 8,192 × 128 = 1,048,576 bytes (1 MiB). Total 4,718,592 bytes = 4.5 MiB (2,097,152 + 524,288 + 524,288 + 65,536 + 65,536 + 327,680 + 65,536 + 1,048,576). Separate input/output/audit budgets are exact maxima: 1,024 × (4,096-byte body + 8,192-byte headers) = 12,582,912 bytes input; 1,024 × 4,096 = 4,194,304 bytes output; 1,024 × 1,024 = 1,048,576 bytes audit. All associated metadata fits charged entries/reserve; no uncharged budget metadata. Index, counter, allocator and fair-selection metadata fit charges; reject before allocation otherwise. Decoding/scratch is bounded by guarded body buffers; no uncharged body copy. The 1,024 login tasks are separate from bearer processing; no finish-response workers are budgeted.

**Shared LockoutStore (native and HTTP).** Exactly 8,192 fixed 128-byte pair slots (1 MiB). Retained failure pair key uses unchanged native username digest `HMAC-SHA256(lockout_secret, canonical_username)[..16]` and normalized IPv4 /24 or IPv6 /64 prefix. All hashes are keyed with `lockout_secret`. Keep at most 64 pairs per base prefix (/24 IPv4, /64 IPv6) and 512 per parent (/16 IPv4, /48 IPv6). Active hour-lock pairs count against both owner quotas and are never evicted. On an exhausted source quota, evict the lowest-count then oldest unlocked pre-threshold pair within every exhausted owner quota; at global table capacity when no source quota is exhausted, evict the globally lowest-count then oldest unlocked pre-threshold pair. Never evict an active lock. If no eligible candidate, DROP accounting for the new pair; do not deny or claim proof was verified. Existing retained-pair updates continue when full. Check pair quotas by bounded scan of fixed slots; no separately allocated quota map/index. Per-slot source/quota metadata must fit 128 bytes per slot and existing fixed reserve. 8,192/512=16 parents are needed to fill table; one parent at most 512/8,192=6.25%, one base prefix at most 64/8,192=0.78125%.

Pair behavior: every verified bad proof increments the pair count; backoff is 100 ms × 2^(count-1), capped at 30 seconds, beginning with count 1. The 50th failure locks that pair for one hour. Five minutes idle resets count. `unlockUser` clears that user's pair failures and active locks across all subnets; it clears only pairs. If an untracked pair cannot be retained under quota/table pressure, failure accounting is dropped; brute force against it is limited by HTTP source CREDIT only. Guessing bound: BEGIN credits allow about 9,760 BEGIN requests in the first hour per /48 or /16 while the source state is resident (160 initial plus 160 per minute); FINISH spends credit too, so completed guesses are about half of that; there is no per-account cap across different subnets, and a source that fills its 512 per-parent pair quota leaves its new pairs with source credits only. Behind a NAT, one member can arm the pair of a specific (user, /24) it fails against and can consume the /24 shared CREDIT (10 per minute) and its 16 input slots for everyone behind that NAT; this is inherent to source-keyed limits and is accepted. Likewise one subscriber of an IPv4 /16 can deplete the /16 parent CREDIT (160 per minute) shared by unrelated children (a residual, accepted). Bounded shared representation is a prerequisite for future implementation, not a claim about current native code. Existing native persistence mechanism snapshots failures and active locks using `LockoutSnapshot` about every 60 seconds and rehydrates at boot; the bounded HTTP proposal applies caps 8,192 global / 64 per base prefix / 512 per parent on load, loading active locks first then highest-count pairs and dropping overflow. Active locks and failures for the same key consume one slot.

Native active-pair pre-challenge rejection and post-proof padding remain unchanged. HTTP pre-backoff is new; native adoption is a separate AUTH-PRE decision.

Handshake TTL is fixed 60 seconds, monotonic and non-sliding. BEGIN captures `begin_time` from the same checked Unix-nanosecond clock used for LockoutStore `last_fail`; TTL uses monotonic time, not Argon2 iteration count. Expire before claim/issuance; GC removes expired unclaimed entries and restart drops all pending state. Expired, replayed, unknown or evicted IDs return generic 401, or source-arrival 429 if limited. Correct-source/subnet/listener/security match claims atomically once; wrong-source finish does not consume or refresh state. Claimed counters remain held until task exit. Release outstanding/prefix counters exactly once on success, failure, I/O cancellation, expiry or eviction; spent credits are never refunded. Store listener_id/security class in pending state and require the same logical listener/class at finish. Cross-prefix or IPv4/IPv6 source change returns 401 and requires fresh BEGIN; normalize mapped IPv4.

Canonical source processing: normalize mapped IPv4 and bind to effective /24 or /64; socket caps use actual peer. For trusted peer, join duplicate XFF lines and walk right-to-left, skipping trusted proxies to first valid untrusted address; ignore malformed attacker prefix left of it. Malformed entry before valid client, missing header or all-trusted chain =>400, never substitute the proxy peer. Apply the proxy-peer profile in §5.1; ignore XFF outside that profile. Proxy strips incoming forwarding headers and sets its own. Never trust X-Forwarded-Host/Proto. Exactly one Host; missing/duplicate/malformed =>400, unlisted =>403 `authority_forbidden`. Lowercase hostname, normalize HTTPS 443/HTTP 80 default ports; reject URI/path/userinfo/fragment/whitespace; exact configured authority only, no DNS resolution. Origin rules §6; TLS/proxy/plaintext rules §5.1.

## 4. Authentication failures and timing

Admission and failure ordering is normative and stated here once. At request head validate framing and resolve source; debit ARRIVAL, then CREDIT, then reserve input slot; only then send 100 Continue/read body. Body-read/slot deadlines and sizes are in §3. Full global input pool returns 503 `server_busy` with `Retry-After: 1` and no new state; source caps/credit exhaustion returns 429; all debits remain spent. After body, canonical parse and scope validation, then body-dependent policy, then retained-pair active lock/backoff gate, then pending allocation/challenge. Pair gate 429 has ceil `Retry-After` clamped 1..=3600. FINISH follows the same request-head sequence, then parses and claims; no credits are refunded on any outcome.

At FINISH, atomically claim live matching state and recheck retained pair. If the pair is actively locked, in backoff, or `last_fail` is strictly greater than captured `begin_time`, skip proof and return generic 401 without registering a failure and without advancing `last_fail`. Equal timestamps do not satisfy strict `>`. A proof that fails against the real verifier or against the fake verifier of an unknown or ineligible user registers exactly one pair failure, identically for known, unknown and ineligible users; pair state, BEGIN gating and `Retry-After` therefore never depend on user existence or eligibility, and fake-proof paths never authenticate. A valid proof followed by policy denial does not register a pair failure. Success clears only matching pair after durable issuance. Pair failures/locks are best-effort under bounded-state admission: when a new pair cannot be retained under table/quota pressure, its failure accounting is dropped, not represented as a lockout denial; source credits still bound requests. Same checked Unix-nanosecond clock source; clock failure fails closed. Pad finish from body receipt to 50 ms plus uniform integer jitter 0..25 ms. Every terminal finish, expiry and eviction releases pending-prefix counts exactly once; wrong-source finish cannot release another attempt's counts.

**Metrics and audit.**

| Metric | Type | Labels / bounds |
|---|---|---|
| `shamir_login_http_total` | counter | `endpoint`, `result` only; fixed vocabularies |
| `shamir_login_http_pending` | gauge | no source/user labels; <=1,024 |
| `shamir_login_http_evictions` | counter | fixed reasons only; no identity labels |
| `shamir_login_http_accounting_dropped{reason}` | counter | fixed reasons: `pair_quota`, `pair_table_full`, `arrival_table_full`, `credit_table_full`; no identity labels |
| `shamir_login_http_pair_slots` | gauge | pair-table occupancy count only; <=8,192; no identity labels |
| `shamir_login_http_begin_duration_seconds` | histogram | fixed buckets; no identity labels |
| `shamir_login_http_finish_duration_seconds` | histogram | fixed buckets; no identity labels |
| `shamir_login_http_kdf_ineligible_users` | protected operator gauge | count only; no IDs |

Audit queue is bounded at 1,024 events/1 MiB; overflow drops/samples. It never contains password, proof, token secret/MAC, nonce, raw body, handshake ID, username or address. Protected metric is count-only; public metrics expose no victim-specific state. No delayed finish-response workers. Retry-After varies 1–30 seconds for backoff and may be ~3,600 seconds for pair lock; victim-activity oracle is accepted.

## 5. Token issuance, configuration and persistence

### 5.1 Listener and login configuration

`browser_origin_allowlist` is the same setting as `listener.browser_origin_allowlist` for TOKEN_API WS; empty by default, with no global, other-listener, or admin UI inheritance. `trusted_proxies` is the same setting as `listener.trusted_proxies` for TOKEN_API §11; empty by default, with no global, other-listener, or admin UI inheritance. Both referenced keys are not owned by login. With login enabled: actual TLS requires `external_tls=false` and is class 0x01; plaintext with `external_tls=false` is class 0x00 only when bind and socket peer are loopback and `allow_plaintext_remote=false`; plaintext with `external_tls=true` is class 0x02 only with loopback bind, nonempty `trusted_proxies` CIDRs, explicit nonempty authority allowlist, and nonempty HTTPS-only Origin list. For class 0x02, an untrusted socket peer is 403 `proxy_forbidden` before body regardless of XFF; valid XFF chain is required (bad/missing/all-trusted chain is 400). Public edge MUST accept HTTPS only and reject HTTP before forwarding; loopback plaintext backend is trusted. Actual TLS with `external_tls=true` is invalid at startup. With login enabled, plaintext listener configured with `allow_plaintext_remote=true` is a startup error; enabled direct nonloopback plaintext is also a startup error. Login disabled retains 404 behavior. These rules apply only to login; bearer behavior is unchanged.

| Name | Type | Default | Scope | Validation / semantics |
|---|---|---|---|---|
| `login.enabled` | bool | `false` | per listener | Disabled routes return 404 pre-body for every method; enabled wrong method returns 405. |
| `external_tls` | bool | `false` | per listener | Uses the sole normative listener profile above. |
| `login_enumeration_hardening` | bool | `true` | global | HTTP-only; exact global current KDF tuple required when true. Mismatching users individually ineligible; preflight affected-user count and require operator acceptance when raising tuple. |
| `login.authority_allowlist` | array of canonical authorities | loopback bound aliases | per listener | Loopback aliases `127.0.0.1`, `localhost`, `[::1]` at actual bound port; exact match, no DNS. Direct nonloopback TLS and class 0x02 require explicit nonempty list. |
| `login.listener_id` | string | generated once | per logical listener | CSPRNG 16-byte ID persisted in server metadata, stable across restart. Supplied value MUST be exactly 32 lowercase hex characters; reject duplicates among distinct logical listeners in deployment/transcript-trust scope. Replicas share only the same logical listener ID with identical policy. Configuration changes invalidate pending handshakes, not native tickets; never path-derived. |
| `tokens.login_ttl` | u32 seconds | 86400 | global | 1..=86400, capped by `tokens.max_ttl_days`; checked conversion/arithmetic; invalid configuration fails startup. |
| `tokens.login_allow_read_write` | bool | `false` | global | False: parsed read_write BEGIN returns 403 `login_write_forbidden` before lookup/pair accounting/challenge/state. True still uses ordinary rights/listener constraints. |
| `browser_origin_allowlist` | array of serialized origins | same listener value, otherwise empty | per listener | Reuses `listener.browser_origin_allowlist` from TOKEN_API WS; no global/other-listener/admin_ui inheritance. Absent Origin accepted; present exact-match only. Profile requirements above apply. |
| `trusted_proxies` | array of CIDRs | same listener value, otherwise empty | per listener | Reuses `listener.trusted_proxies` from TOKEN_API §11; no global/other-listener/admin_ui inheritance. Profile requirements above apply. |
| `allow_plaintext_remote` | bool | `false` | per listener | Referenced existing key, not owned by login; profile requirements above apply, bearer behavior unchanged. |
| `tokens.max_per_user` | u32 | 20 | global | Referenced existing shared atomic active-token cap, not owned by login; applies to all creation paths. |

All budget, admission, body/header, deadline, KDF, padding and byte-charge limits are fixed constants, not configuration knobs. `tokens.max_per_user` is shared global cap, default 20. Bound aliases/default ports are exact canonical forms.

### 5.2 Token issuance and persistence

At finish verify the client proof against retained `stored_key` (the retained `server_key` is only for the server signature); reload current roles and require a regular non-superuser/nonreplicator account. Before lookup/challenge/state/CSPRNG/pair accounting, reject parsed `read_write` with 403 `login_write_forbidden` when global `tokens.login_allow_read_write=false`. Check self-service and read-only-replica denial before challenge; known privileged account receives normal challenge/proof then `login_account_denied`. Recheck global policy and rights at finish. Mode/channels/database scope intersect current ordinary rights. UID is immutable owner. Password/role/flag changes and restore invalidate through `tickets_invalid_before_ns`; deletion makes owner unknown. `RevokeUserTokens` revokes token records without raising threshold or invalidating SCRAM tickets (TOKEN_API §5). Before creation require current UID/stored_key/threshold equal BEGIN snapshot and eligibility true; otherwise generic 401. Roles are not retained at BEGIN; roles snapshot is taken at token creation per TOKEN_API §3.

Create token with exact BEGIN-bound scope and ordinary owner rights, privileged flags false. Require `created_at_ns > tickets_invalid_before_ns`; equality/invalid/unavailable clock fails closed (unavailable clock 503). `created_at_ns` is checked u64 Unix-epoch nanoseconds; arithmetic overflow fails closed. Durably write, re-read UID/stored key/threshold/privilege, durably revoke if changed, then publish/respond. Active-only label uniqueness applies to unexpired/unrevoked tokens for every creation path; `tokens.max_per_user` is the shared atomic cap (default 20), not a hard-coded login cap. Label collision 403 `token_label_exists`; cap 403 `token_limit_reached` after valid proof. Owner immutable UID; response owner canonical username. Secret exactly 71 characters (`shk1.` + 22-character ID + `.` + 43-character secret), returned once and never logged. Write failure 503/no token; failed compensation means no secret and critical audit. Response-write uncertainty leaves token active; revoke out-of-band through SCRAM token listing, not bearer admin API.

`tokens.login_ttl` is u32 seconds, default 86,400, range 1..=86,400, capped by `tokens.max_ttl_days*86,400`; invalid config fails startup. Expiry is `created_at_ns + min(login_ttl_seconds,max_ttl_days*86400)*1_000_000_000`; Argon2 `time` is iterations, not seconds. Browser SDK SHOULD default channels `rest`; Node SDK SHOULD prefer native exporter-bound SCRAM plus `CreateApiToken`. HTTP lacks mode-0x02 Ed25519/transport identity and is weaker than browser mode 0x02; <=24-hour TTL is compensating control, not equivalence.

## 6. Origin, browser and CSRF

Same-origin browser use is supported, but there is no CORS; browser UI is hosted on a separate listener and may use a same-origin reverse proxy. No cookies or ambient credentials are used, so CSRF-style ambient-session attacks do not apply. XSS remains a material risk: keep the returned bearer in memory only and never persist/log it. JavaScript strings cannot be reliably zeroized; best-effort lifetime minimization is not a guarantee. There are no server-secret cookies.

Origin absent is accepted. When present, exactly one serialized origin tuple (scheme, host, port) must match `browser_origin_allowlist`; duplicate, `null`, malformed or unlisted =>403 `origin_forbidden`. Allowed scheme/profile requirements are defined solely in §5.1. Host validation in §3 applies even when Origin is absent. Origin/User-Agent never establish native identity. A local observer of either plaintext loopback leg captures both `(T,proof)` and the reusable bearer token; SCRAM does not encrypt the finish response.

## 7. HTTP statuses and exact bodies

403 `login_write_forbidden` is emitted before lookup, pair accounting, challenge or state iff `tokens.login_allow_read_write=false` and parsed BEGIN requested read_write. `login.enabled=false` is 404 pre-body regardless of method; only enabled routes return 405 for wrong method. 400 `invalid_request`: malformed framing/MessagePack/fields, unsupported version/mode, noncanonical username, scope >512, empty database array, all-zero client nonce, malformed XFF/authority. 401 `authentication_failed`: bad/unknown/ineligible proof, locked-pair finish, expired/unknown/replayed/claimed ID, wrong source/listener/security. 403 codes: `origin_forbidden`, `authority_forbidden`, `proxy_forbidden`, `token_self_service_disabled`, `login_read_only_replica`, `login_write_forbidden`, `login_account_denied`, `token_label_exists`, `token_limit_reached`. 404 `not_found`: unknown path or disabled route. 405 `method_not_allowed` with `Allow: POST`. 408 `request_timeout`. 413 `request_too_large`. 415 `unsupported_media_type` for missing/wrong Content-Type or any present Content-Encoding. 429 `rate_limited`; 431 `request_headers_too_large`; 503 `server_busy` for full global input pool or temporary storage, clock, shutdown or randomness failure (not permanent read-only). Every error body exactly `{ "error": code }`, except 429/503 exactly `{ "error": code, "retry_after": u32 }`; `Retry-After` equals body, ceil-rounded and clamped 1..=3600. No `WWW-Authenticate: Basic`. Read-only replica permanent 403. Invalid/unavailable clock 503; threshold equality or stale UID/key generic 401. Locked/backoff/depleted BEGIN returns 429 without state. Finish backoff since BEGIN skips proof and takes common 401 path. Header checks precede body; user-dependent checks follow body and precede challenge/state. All paths close the connection; if an error occurs before headers, return the specified response, while timeout after headers closes without replacing the response.

`Expect: 100-continue` gate ordering follows §4; request head, credits and slot precede body, and pair gate follows canonical parsing.

## 8. Threat model and limits

TLS 1.3 and no 0-RTT per TOKEN_API §11; passive observer cannot see proof/token. Captured `(T,proof)` enables offline password guessing, so client KDF floor is mandatory. Trusted TLS inspection root may alter challenge; client bounds limit downgrade but do not prevent relay. Trusted TLS proxy sees plaintext proof/token; loopback backend can be observed by local processes. Any local process able to reach the loopback backend can forge XFF; trust is valid only when the backend is isolated/protected. Recommend authenticated backend transport or a future ACL-protected Unix-domain socket; TOKEN v1 has no mTLS proxy-authentication knob. Per-IP accept caps apply to actual socket peer; exempt trusted proxy peers from per-IP caps only, never global connection/task limits; effective-client prefix caps apply after XFF parsing. Direct loopback plaintext has same host/process boundary. Active attacker may relay/alter proof, causing victim-pair failures/lockout. No Ed25519 pin/exporter binding; proxy compromise, endpoint compromise, XSS and relay remain. Remote plaintext forbidden for LOGIN only; bearer behavior is unchanged. Transcript class is not channel binding.

## Acceptance checklist (AUTH1/AUTH3; future CI, not run)

- Wire vectors: transcript prefix/domain is exactly 20 bytes and T length is 173..933; positive fixint and unsigned widths including non-shortest encodings accepted, signed markers rejected; signature verification failure discards token; scope/listener/security/version alterations fail; client KDF floor/max/version rejection occurs before Argon2; exact hardened tuple; CSPRNG collision/retry behavior.
- AUTH1 accounting: native checks active lock pre-challenge and pads after proof; HTTP-only pair gate/backoff. Pair curve 100 ms × 2^(count-1), begins at count 1, capped 30 seconds; 50th locks one hour; five-minute idle reset; unlockUser clears user's pairs across subnets. Per-quota eviction vs global lowest-count/oldest eviction, never active lock, drop accounting if no eligible pair. BEGIN Retry-After oracle accepted; finish `last_fail > begin_time` skips proof without accounting. HTTP credits bound untracked-pair guessing; native admission separate.
- AUTH1 uniform pair state: a failed proof against a known, unknown or ineligible user registers exactly one pair failure; BEGIN gating and `Retry-After` after a failed FINISH are identical for all three (test with a fake-verifier mismatch).
- AUTH1 capacity: pair table 8,192 slots/1 MiB; 64 pairs per base and 512 per parent; at least 16 parents to fill. Input pool 1,024: IPv4 /24=16, /16=64; IPv6 /64=16, /56=64, /48=64; each /16 or /48 can occupy at most 64/1,024 = 6.25% of global input slots. Full input returns 503 with Retry-After 1, allocates no new pending state and preserves existing pending state. Body deadline 3 seconds and input slot released on all paths. CREDIT per minute: v4 /24 10, /16 160; v6 /64 10, /56 40, /48 160. ARRIVAL same tiers per second. Test pre-100 ARRIVAL/CREDIT; atomic distinct own-parent charge, no partial debit/refunds; pair table 8,192 slots, 1 MiB and total 4,718,592 bytes/4.5 MiB; source-key quota 32 children (/48=65 keys, /16=33 keys); fair-share unclaimed eviction and all-claimed 429; table churn residual limits.
- AUTH1 capacity/security checks: restore active locks first then highest-count pairs, cap and drop overflow; metrics expose fixed reasons and pair occupancy only, no identity; client canonical Unicode 15.1, empty database array 400, all-zero client nonce rejected server-side with 400; server nonce must be nonzero and distinct; returned token id matches decoded ID, adds 21 bytes and leaves T unchanged; literal amendment text.
- AUTH3 deployment/policy/persistence: direct TLS vs class-2-only `external_tls`, same-origin UI on separate listener; duplicate XFF lines joined, right-to-left trust walk, malformed/missing/all-trusted rejection, Host authority/DNS-rebinding and untrusted-peer behavior; absent Origin accepted, strict present Origin allowlist, class-2 HTTPS; remote plaintext refused. Same-subnet versus cross-prefix/family source and fresh-BEGIN behavior; replay/parallel/expiry/restart. Begin self-service/read-only/write denials allocate no state; privileged-account challenge/proof then denial; active-only label/cap and concurrent atomic limit; ordinary permissions/scope. Stored-key/threshold recheck, durable revocation and no-secret failure; expiry, RevokeUserTokens, password/role/flag/delete/restore invalidation. Hardening true/false eligible/ineligible/unknown, fake-path never authenticates, count metric privacy, native TCP/WS leak, five-minute lockout counter and mapped-IPv4 prerequisite.

## 9. Implementation prerequisites and source-grounded constraints

This feature is not implemented; no build or tests are claimed. Prerequisites: normalize mapped IPv4 in `crates/shamir-connect/src/server/limits/lockout.rs` `subnet_of` (currently handles IPv4 and IPv6 separately); implement bounded shared LockoutStore preserving native pre-challenge rejection in `crates/shamir-server/src/sync/connection/handshake.rs`; add client lower KDF bounds (current validation only upper bounds); forbid login-listener `kdf_override` in `crates/shamir-server/src/boot/config.rs`. AUTH_PROTOCOL §13.4 requires recreation to change historical tuple, losing UID/resource/token continuity. The existing pair backoff curve is 100 ms × 2^(count-1), starting at failure 1 and capped at 30 seconds; failure 50 activates a one-hour pair lock. Native handshake checks only an active pair lock before challenge; after proof it pads latency to max(target constant-time duration, backoff duration) (handshake.rs:212–237, 925–939). HTTP's pre-challenge pair backoff gate is new HTTP-only behavior; native adoption is a separate AUTH-PRE decision. HTTP residual: an untracked pair is bounded only by HTTP source credits; native admission remains its own mechanism. Raising current KDF parameters may make every mismatching existing user individually ineligible; preflight and disclose affected count. Positive fixint 0x00..=0x7f is unsigned; negative fixint and signed markers reject.

## 10. Neighboring documents

The amendments that this specification requires in the neighboring normative documents were applied in the same change to `AUTH_PROTOCOL.md`, `IMPLEMENTATION_GUIDE.md`, `TOKEN_API.md`, `SECURITY_MODEL.md`, `ADMIN_UI_HOSTING.md`, `TRANSPORT_WS.md` and `README.md`. The per-amendment audit is in `docs/dev-artifacts/audits/2026-11-01-token-spec2/NOTES.md`. Where this document and a neighboring document differ, the neighboring document (as amended) is authoritative for its own subject and this document must be corrected.

## Owner decisions (binding; decided by owner's delegate)

- D1 — APPROVED WITH CHANGE: protocol-local 0x03, separate transcript, client KDF floor; noncanonical username 400; vectors before implementation.
- D2 — REJECTED: no remote plaintext opt-in; classes loopback plaintext, direct TLS, trusted-proxy TLS. Remote clients require TLS direct or through trusted proxy.
- D3 — APPROVED WITH CHANGE: TTL 1..86,400 seconds, read-only default, active-only label/cap, distinct after-proof errors; browser SDK REST default.
- D4 — APPROVED WITH CHANGE: fresh-proof exception weaker than 0x02, 24-hour compensating TTL, refuse privileged accounts, policy checks at begin.
- D5 — APPROVED WITH CHANGE: absent Origin allowed; present exactly one syntactically valid Origin matching existing allowlist; null/duplicate/malformed/unlisted 403; exact Host allowlist. HTTPS-only Origins required for class 0x02, not universally.
- D6 — APPROVED WITH CHANGE: bounded pair state, HTTP BEGIN pair gate, source credits, stacked caps, unclaimed eviction, no global admission off-switch.
- D7 — APPROVED WITH CHANGE: per-user ineligibility, no login-listener `kdf_override`, count metric, no password-change upgrade claim, native TCP/WS leak disclosed.
