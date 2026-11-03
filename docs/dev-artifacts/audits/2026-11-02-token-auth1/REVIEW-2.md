# TOKEN-AUTH1 review 2: revision 1 (WP-A / WP-B / WP-C)

Scope: `fix/p34-auth1-login-server` at `bcbf39226` (revision-1 work `46eb77dd0`,
master merge `fd869792b`, cooldown drop `960045e46`, CI path fix `bcbf39226`),
diffed against `origin/master`. Checked against the briefs
`auth1-login-http-server.md` and `auth1-revision-1.md`, the defect list and
WP plan of `REVIEW-1.md`, the executor note `DESIGN.md`, the spec
`TOKEN_LOGIN_HTTP.md` and the TS client (`origin/master:crates/shamir-client-ts/src/login/{protocol.ts,core.ts}`).

Method: nothing was compiled. Every changed `.rs` file was read as rustc,
clippy (pinned 1.95, edition 2021, workspace `absolute_paths` max 5,
`too_many_arguments` max 7), rust-guard `modules` and the layout checker would.
Only these tools were run, all read-only: `rustfmt --edition 2021 --check`
on every changed `.rs` file, which passes for all of them;
`python -I scripts/ci/check_layout.py`, which reports OK; real `ls -A` entry
counts; and a Python recomputation of all three transcript vectors from the
spec section 2 layout, where all three match byte for byte (173, 179 and 197
bytes) and the JSON and TOML copies agree.

## 0. Verdict in one paragraph

WP-A, WP-B and WP-C were essentially done. Every REVIEW-1 defect C1-C4, W1,
T1-T5, B1 and G2 is fixed, and so are P2-1, P2-2, P2-4, P2-5 and P2-6. Exactly
**one compile error remains**, in `shamir-connect`'s test target (F1). I
found no compile error in `shamir-server` (config, boot, meta, tests) or in
`shamir-transport-http` (lib, tests), and no clippy `-D warnings` hit. All
the risky spots were traced by hand: Send-ness of the generic connection
futures, RPITIT plus `async fn` impls, `may_dangle` borrows of `Vec<&HeaderValue>`
across `into_body`, coercions, path visibility, absolute-path segment counts,
`too_many_arguments` (`handle_login`, `serve_with_login` and `serve_inner`
take exactly 7 arguments), unused imports and re-exports, and dead code.
Nothing is dead: every new item is `pub` in an exported module or used. The
remaining findings are three test oracles that cannot fail for the reason
they claim to test (F2, F3), missing WP-C tests (F4), two API-visibility
problems that will block revision 2's unit tests (F5), one ordering
improvement (F6), the G1 layout done through `#[path]` instead of the
manifest form (F7), and small hygiene items.

## 1. WP-A / WP-B / WP-C checklist against REVIEW-1

| REVIEW-1 item | State | Evidence |
|---|---|---|
| C1 `ns::MILLISECOND` | FIXED | `limits/lockout.rs:610-640` uses `ns::SECOND / 1_000` and `checked_add`. The 50-failure trace in the test is correct. |
| C2 `serde_json` | FIXED | TOML mirror plus `include_str!("../../../../../test-vectors/http_login_transcript_v1.toml")`. The path resolves to `crates/shamir-connect/test-vectors/`. Only `serde`, `toml` and `hex` are used, all existing dependencies. |
| C3 trait import / duplicate test | FIXED | The duplicate test was dropped. |
| C4 `PairStatus` import | FIXED | `tests/security/lockout_tests.rs:2-6` |
| W1 unused re-export | FIXED | `limits/mod.rs` has no item re-export. |
| T1 vectors | FIXED | All three cases recomputed independently and they match. A third case was added (`security=2`, non-zero listener, `é`, four databases incl. uppercase `A`). |
| T2, T3, T4 | FIXED | `http_login/tests/transcript_tests.rs` resets every field. The unsorted case is a real two-entry scope. The truncated-proof line is gone. |
| T5 | FIXED | Expects `51_000_000_000`. |
| B1 database names | FIXED | `scope_bytes.rs:48-60` and `transcript.rs:151-157` check ASCII `[A-Za-z0-9_-]` only. `A-b_9`/`Main` are accepted (two tests). This matches `protocol.ts` `DATABASE_SYNTAX` (the client additionally requires NFC, which ASCII implies). |
| G1 orphan `tests/mod.rs` | PARTIAL, alternative form | `tests.rs` is kept and mounted through `#[path = "tests/mod.rs"]` and `#[path = "../tests.rs"] mod boundary_tests;`. It compiles and rust-guard `modules` resolves `#[path]` (`modules_cmd/resolution.rs::resolve_attr`), but it breaks the tests-folder convention. See F7. |
| G2 rustfmt | FIXED | `rustfmt --check` is clean on all changed files. |
| P2-1 cap_tests local `use`, 6-segment path | FIXED | Imports are hoisted. The `pub(super)` widening of `record` / `new_token` / `service` is listed in DESIGN.md. |
| P2-2 scheduler stub | FIXED | Constant `PairStatus`, no `inner` store. |
| P2-3 duplicate mode/channel enums | NOT DONE (was optional) | See F10. |
| P2-4 label UTF-8 | FIXED | `transcript.rs:121`. `scope.len() < 2` is gone. |
| P2-5 proof via key schedule | DONE, but does not compile | F1 |
| P2-6 README row | DONE, with an inaccurate claim and unrelated edits | F9 |
| P2-8 FINISH padding | Recorded in DESIGN.md as a revision-2 obligation | Correct: no service exists yet. |
| WP-B config, validation, meta | DONE | `login_listener.rs`, `http_listener.rs:134-211`, `http_validation.rs:27-30`, `tokens.rs`, `config.rs` (`SecurityConfig` field and duplicate-id loop), `server_meta.rs:464-492`. Every full struct literal of `SecurityConfig` / `HttpListenerSettings` was found by brace-matching (4 + 1) and updated. All others use `..`. `deploy/server.example.ktav` documents the keys, and its new `tokens:` block uses known keys under `deny_unknown_fields`. |
| WP-C transport hook | DONE (code), PARTIAL (tests) | `routes/login/*`, `service.rs:41-53` (early branch before the gate), `accept.rs` (`serve` delegates to `serve_with_login` with `NoLogin`), `guarded_io.rs` (flag plus `Arc<AtomicU64>` override), `framing/canned_login.rs`. No field was added to `HttpServerConfig`. Bearer-only canned bytes are byte-identical (`Content-Length: 0`, no `Pragma`). |
| Accidental files named in DESIGN.md | Removed | Neither `boot/config/login_listener_tests.rs` (at top level) nor `routes/login/login_tests_mount.rs` exists any more. |

Layout (real `ls -A`): `transport-http/src/server` 7, `server/routes` 2,
`routes/login` **7 (full)**, `routes/login/tests` 2, `framing` 5,
`framing/tests` 6, `server/tests` 4; `shamir-server/src/boot/config` **7
(full)**, `config/tests` 3, `engine` 7, `tests/registry` 6,
`security/tokens/tests` 5; `shamir-connect/src/server/auth` 6,
`auth/http_login` 6, `http_login/tests` 5, `limits` 5. Largest touched files:
`server_meta.rs` 919, `service_tests.rs` 897, `config.rs` 784, `lockout.rs`
714, `guarded_io.rs` 712. All are within the limits.

## 2. Findings (severity-ordered, each with the fix)

### P0: compile errors

**F1. `shamir-connect/src/server/auth/http_login/tests/proof_tests.rs:39`: E0308.**
`server_key` is `ServerKey = Zeroizing<[u8; 32]>`. `hmac_sha256` takes `&[u8]`.
`&Zeroizing<[u8;32]>` deref-coerces to `&[u8;32]`, but rustc does not chain
that deref with the unsizing step to `&[u8]`, so the argument is
"expected `&[u8]`, found `&Zeroizing<[u8; 32]>`". That is why the rest of the
crate writes `&key[..]`, as lines 11-13 of the same file do. Line 38
(`build_server_signature(&server_key, ..)`, which expects `&[u8; 32]`) is
fine. Fix:

```rust
    let client_recomputed = hmac_sha256(&server_key[..], transcript);
```

This is the only call of a crypto helper in the diff with a `Zeroizing`
argument that is not sliced (I grepped every new `hmac_sha256(` / `sha256(` /
`constant_time_eq(` call). The fast tier stops at `shamir-connect`, so this
is what CI hits next.

### P1: test oracles that cannot fail for their stated reason

**F2. `transport-http/src/framing/tests/login_canned_tests.rs::shared_write_timeout_expires_stalled_canned_rejection`.**
Under `start_paused = true` the runtime auto-advances to the next timer
whenever it is idle. If the override were ignored, the 5 s login default
canned deadline would fire during `task.await` and `is_err()` would still
pass. The test does not prove that the override is used. Fix (replace the body
after `let mut request = [0; 64];`):

```rust
    let started = tokio::time::Instant::now();
    let task = tokio::spawn(async move { guarded.read(&mut request).await });
    client.write_all(b"GET / HTTP/1.0\r\n\r\n").await.unwrap();
    let error = task
        .await
        .unwrap()
        .expect_err("stalled canned write must hit the override deadline");
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    // Paused clock: elapsed is the virtual time, 5 ms with the override, 5 s without it.
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    let mut output = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(15),
        client.read_to_end(&mut output),
    )
    .await
    .unwrap()
    .unwrap();
```

(Drop the `tokio::time::advance` line. `override_ms` stays alive, cloned into
`guarded`.)

**F3. `shamir-server/src/boot/config/tests/login_listener_tests.rs:40-43`.**
The unknown-key case asserts only `is_err()`. Any parse failure passes,
including a ktav syntax problem or the flatten buffering failing on the nested
map, which is the class of oracle that `t3b3-revision-1.md` warns about.
There is also no positive parse of a `login:` block at all. Fix (replace
lines 40-43):

```rust
    let parsed: ListenerConfig = ktav::from_str(
        "{ kind: http, addr: 127.0.0.1:1, tls: false, external_tls: true, login: { enabled: true, authority_allowlist: [localhost:1], listener_id: 00112233445566778899aabbccddeeff } }",
    )
    .expect("nested login block parses through the flattened HTTP settings");
    assert!(parsed.http.external_tls);
    assert!(parsed.http.login.enabled);
    assert_eq!(parsed.http.login.authority_allowlist, vec!["localhost:1"]);
    assert_eq!(
        parsed.http.login.listener_id.as_deref(),
        Some("00112233445566778899aabbccddeeff")
    );
    let error = ktav::from_str::<ListenerConfig>(
        "{ kind: http, addr: 127.0.0.1:1, tls: false, login: { enabled: false, typo: true } }",
    )
    .expect_err("unknown nested login key");
    assert!(error.to_string().contains("typo"), "{error}");
```

(If ktav does not echo the field name, assert on `"unknown field"` instead.
That is the serde text it wraps.)

**F4. Required WP-C tests are missing** (REVIEW-1 WP-C list; DESIGN.md
admits the first two). Add these to `server/routes/login/tests/route_tests.rs`
(212 lines now, about 300 after):

```rust
#[tokio::test(start_paused = true)]
async fn stalled_body_times_out_with_request_timeout_after_three_seconds() {
    let started = tokio::time::Instant::now();
    let output = exchange(b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Type: application/msgpack\r\nContent-Length: 4\r\n\r\nda", Arc::new(Admit)).await;
    assert!(output.starts_with(b"HTTP/1.1 408"));
    assert!(output.ends_with(b"\x81\xa5error\xafrequest_timeout"));
    let elapsed = started.elapsed();
    assert!(
        elapsed >= Duration::from_secs(3) && elapsed < Duration::from_secs(4),
        "{elapsed:?}"
    );
}

#[tokio::test]
async fn missing_or_duplicate_content_type_is_unsupported() {
    for wire in [
        &b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\n\r\n"[..],
        &b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Type: application/msgpack\r\nContent-Type: application/msgpack\r\nContent-Length: 0\r\n\r\n"[..],
    ] {
        let output = exchange(wire, Arc::new(Admit)).await;
        assert!(output.starts_with(b"HTTP/1.1 415"));
        assert!(output.ends_with(b"\x81\xa5error\xb6unsupported_media_type"));
    }
}

#[tokio::test]
async fn admitted_request_receives_continue_then_body_is_read() {
    let (mut client, server) = duplex(8192);
    let (hold_tx, hold_rx) = tokio::sync::mpsc::channel(1);
    drop(hold_tx);
    let hook = Arc::new(Admit);
    let peer = PeerInfo {
        addr: "127.0.0.1:1".parse().expect("address"),
        tls: false,
        forwarded_for: Vec::new(),
    };
    let service = service_fn(move |request| {
        let hook = hook.clone();
        let peer = peer.clone();
        async move {
            Ok::<_, std::convert::Infallible>(
                handle_login(
                    request,
                    hook,
                    peer,
                    false,
                    tokio::time::Instant::now(),
                    Duration::from_secs(20),
                    Arc::new(std::sync::atomic::AtomicU64::new(0)),
                )
                .await,
            )
        }
    });
    let io = TokioIo::new(crate::framing::guarded_io::GuardedIo::new(
        server, 8192, 64, false, false, hold_rx,
    ));
    let task = tokio::spawn(http1::Builder::new().serve_connection(io, service));
    client
        .write_all(b"POST /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\nContent-Type: application/msgpack\r\nExpect: 100-continue\r\nContent-Length: 4\r\n\r\n")
        .await
        .unwrap();
    let mut interim = [0_u8; 25];
    tokio::time::timeout(Duration::from_secs(15), client.read_exact(&mut interim))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(&interim, b"HTTP/1.1 100 Continue\r\n\r\n");
    client.write_all(b"data").await.unwrap();
    let mut output = Vec::new();
    tokio::time::timeout(Duration::from_secs(15), client.read_to_end(&mut output))
        .await
        .unwrap()
        .unwrap();
    task.abort();
    let _ = task.await;
    assert!(output.starts_with(b"HTTP/1.1 200"));
    assert!(output.ends_with(b"data"));
}
```

The route-level write-timeout override (`finish_reply` stores 5000) has no
test either. Give `exchange` a fourth parameter
`write_timeout_override: Arc<std::sync::atomic::AtomicU64>` and pass it
through instead of the inline `Arc::new(..)`. Update the existing callers
with `Arc::new(AtomicU64::new(0))`. Then add:

```rust
#[tokio::test]
async fn every_login_reply_arms_the_five_second_write_override() {
    let write_override = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let output = exchange(
        b"GET /shamir/v1/auth/begin HTTP/1.1\r\nHost: x\r\n\r\n",
        Arc::new(Admit),
        write_override.clone(),
    )
    .await;
    assert!(output.starts_with(b"HTTP/1.1 405"));
    assert_eq!(write_override.load(std::sync::atomic::Ordering::Acquire), 5_000);
}
```

Acceptance 1 also requires "bearer routes on the same listener behave
exactly as before". Add to `server/tests/login_routing_tests.rs` a second
test that builds the same `Context` but with an enabled hook (copy `Admit`
there). It sends `GET /shamir/v1/ping HTTP/1.1` without `Authorization` and
asserts `HTTP/1.1 401`, the presence of `www-authenticate: Bearer`, and the
absence of `pragma` (the gate still runs for non-login paths).

### P1: API visibility that will block revision 2's unit tests

**F5. `LoginHead::new` and the `LoginResponse` getters are `pub(crate)`.**
The application lives in another crate (`shamir-server`,
`security/perimeter/http_login/routes.rs`). The brief's revision-2 unit
tests (the Host/Origin/XFF table driving `admit_head`, and the
issuance/compensation mapping checking `handle`'s status and exact body) need
to build a `LoginHead` and read a `LoginResponse`. Today they cannot do
either without a socket. Fix now, while the API has no users
(`routes/login/login_head.rs`, `login_response.rs`, `route_handler.rs`):

```rust
// login_head.rs
    /// Builds a head view (the transport and application unit tests use this).
    pub fn new(method: &'a [u8], path: &'a [u8], headers: &'a HeaderMap) -> Self {

// login_response.rs
    /// Response status.
    pub fn status(&self) -> StatusCode {
        self.status
    }
    /// Exact MessagePack body bytes.
    pub fn body(&self) -> &Bytes {
        &self.body
    }
    /// Retry-After seconds, when set.
    pub fn retry_after(&self) -> Option<u32> {
        self.retry_after
    }
}

impl core::fmt::Debug for LoginResponse {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LoginResponse")
            .field("status", &self.status)
            .field("body_len", &self.body.len())
            .field("retry_after", &self.retry_after)
            .finish()
    }
}

// route_handler.rs, login_response()
        Full::new(reply.body().clone())
```

The redacted `Debug` keeps DESIGN.md's "no body output" property. It also
lets `Result<Permit, LoginResponse>` use `expect`/`expect_err` in tests, and
`Permit: Debug` is not required for that.

### P2: ordering, layout and hygiene

**F6. A declared `Content-Length` over the limit is rejected only after
`admit_head`.** Today that spends ARRIVAL/CREDIT and an input slot on a
request that is refusable from its headers alone (`route_handler.rs:137-164`
run after `:119-135`). `body_limit` is synchronous, so move the size check
up. The ordering is still "header checks before ARRIVAL" (spec section 4):
cut lines 137-164 and paste them directly before `let head = LoginHead::new(`.
That moves `protocol_limit`, `limit` and the `declared` match together;
nothing else changes. `admitted` stays where it is.

**F7. G1 done through a `#[path]` chain.** `http_login/tests.rs` (a test
file outside `tests/`) plus two `#[path]` attributes. Convention fix (one
commit, because deleting `tests.rs` and dropping the attribute must land
together, or `mod tests;` becomes E0761, "file for module found at both"):
- move `http_login/tests.rs` to `http_login/tests/boundary_tests.rs`
  (content unchanged);
- `http_login/tests/mod.rs`:

  ```rust
  //! Unit tests for the protocol-local HTTP-login primitives.

  mod boundary_tests;
  mod proof_tests;
  mod scope_bytes_tests;
  mod transcript_tests;
  mod transcript_vector_tests;
  ```
- `http_login/mod.rs`: replace the three test lines with
  `#[cfg(test)]` / `mod tests;`.

Counts afterwards: `http_login/` 5, `http_login/tests/` 6.

**F8. `login.listener_id` is validated only when `login.enabled`**
(`http_listener.rs:207-210`). A malformed id on a disabled listener is
accepted silently and only fails the day login is switched on. Spec 5.1
says a supplied value MUST be 32 lowercase hex characters. Fix: move the
block out of `if self.login.enabled { .. }` to just before the
`trusted_proxies` loop:

```rust
        if let Some(id) = &self.login.listener_id {
            parse_listener_id(id)
                .map_err(|message| invalid(format!("login.listener_id {message}")))?;
        }
```

(Keep the duplicate check in `Config::validate` limited to enabled listeners.)

**F9. Unrelated churn and a false README claim.**
- `http_listener.rs:104` and `:109`: the two existing multi-line `format!`
  calls were collapsed onto single lines of more than 300 characters.
  rustfmt cannot reflow them, which is why `--check` stays clean. Restore
  master's form:

  ```rust
            return Err(invalid(format!(
                "HTTP head/body/request timeout must be greater than zero (effective head_timeout_ms={head_timeout_ms} ms from {}, body_timeout_ms={} ms, request_timeout_ms={} ms)",
                if self.head_timeout_ms.is_some() { "listener override" } else { "security.connection.auth_init_timeout_ms inheritance" },
                self.body_timeout_ms,
                self.request_timeout_ms,
            )));
  ```
  Do the same for the second call (`"HTTP timeout limits invalid: ..."`, with
  `"listeners[].head_timeout_ms"`).
- `boot/config.rs` duplicate-id branch: split the 130-character line:

  ```rust
                    if listener_ids.contains(&decoded) {
                        return Err(ConfigError::Validation(format!(
                            "listeners[{idx}].login.listener_id duplicates another enabled listener"
                        )));
                    }
  ```
- `test-vectors/README.md`: the two unrelated backtick removals (`"alice"`
  and `"hello world!1"`) are reverted. The new row says "TS client consumes
  the JSON", which is false: no TS file references it. The TS suite pins its
  own vector in `browser-login.test.ts`, "pins the independently specified
  login transcript bytes". Write instead: "Message-layout fixture,
  hand-derived; same layout as the TS `buildLoginTranscript` pin".
- `config/tests/http_listener_tests.rs:44` repeats
  `assert!(!defaults.login.enabled);`. Delete the second one.

**F10. `HttpLoginMode` / `HttpLoginChannels` duplicate
`server::token::{TokenMode, TokenChannels}`** (REVIEW-1 P2-3). This is still
recommended before revision 2 maps BEGIN scope to `NewToken`: it is a public
rename with no users yet.

**F11. Small items, no behaviour impact.**
- `route_handler.rs:289`: `0xa0 + u8::try_from(code.len()).unwrap_or(0)`
  silently emits a wrong marker for a code of 32 bytes or more. All codes are
  shorter, so use `0xa0 | u8::try_from(code.len()).expect("fixstr code < 32")`
  behind `debug_assert!(code.len() < 32)`, or a static table like
  `canned_login.rs`.
- `accept.rs`: `serve_with_login` only forwards to `serve_inner`. Either
  inline `serve_inner` or drop it. The `serve` doc lost its "gracefully drains
  tracked tasks on cancellation" sentence; restore it.
- Unreachable but harmless defences: `route_handler.rs:26` (HTTP/1.1 is
  already enforced by the `GuardedIo` scanner and `service.rs:31`);
  `http_listener.rs` "direct plaintext login requires a loopback bind"
  (pre-empted by the existing non-loopback plaintext rule); the `413` arm of
  `GuardedIo::reject` (never called with 413). Keep or delete; no lint fires.
- `deploy/server.example.ktav`: the commented `listener_id: null` assumes
  ktav accepts `null` for an `Option`; I could not confirm this. See also
  section 5.

## 3. Crypto and behaviour against spec section 2 and the TS client

- Transcript `T`: the 20-byte domain without NUL, `01 03 sec`, 16 + 32 raw,
  U16 BE user length plus user, 32 + 32 + 16 raw, three U32 BE values, U8
  `0x13`, then the scope. This is identical to `protocol.ts::buildLoginTranscript`.
  The length bounds 173..933 are enforced and tested (min/max boundary test).
  The vectors were recomputed independently.
- Scope: U16 label length plus label, mode `00/01`, channels `00/01/02`, count
  `FF` for null or `1..4` with U16 length plus name. This is identical to
  `encodeScope`. The label rule (1..128 bytes, no C0/DEL) and the database rule
  (ASCII `[A-Za-z0-9_-]`, 1..255 bytes, strictly bytewise sorted, 512-byte
  total) match `freezeScope`. The builder also re-validates externally supplied
  scope bytes, including UTF-8 of the label. One interop residual is already
  accepted (AUTH2): the server applies full PRECIS UsernameCaseMapped, while
  the TS `canonicalUser` is a partial implementation.
- Proof: `HMAC(stored_key, T) XOR proof -> SHA256`, compared in constant
  time with the recovered key zeroized; the server signature is
  `HMAC(server_key, T)`. This matches `core.ts`.
- Transport wire: the canned bodies are correct fixmaps:
  `request_timeout` (0xaf), `request_too_large` (0xb1),
  `request_headers_too_large` (0xb9), `invalid_request` (0xaf),
  `not_found` (0xa9), and `unsupported_media_type` (0xb6, built at run time).
  `core.ts` checks `Content-Type: application/msgpack` before the status, and
  every login response and every login-listener canned rejection carries it.
  `405` carries `Allow: POST` and body `{error: method_not_allowed}`, as
  `statusError` expects. The five mandatory headers and HSTS on TLS are
  present. There is no `WWW-Authenticate`, cookie or CORS header.
- Deliberate deviation, which I agree with: `NoLogin` answers
  `404 {error: not_found}` on every listener without login, including
  bearer-only ones, where today the gate answered `401`. Brief decision 2
  says "else today's behaviour", but acceptance item 1 and spec 5.1 ("Login
  disabled retains 404 behavior") require 404. The spec wins. Note this in
  DESIGN.md.
- FINISH padding (spec section 4: 50 ms plus 0..=25 ms from body receipt)
  belongs to revision 2 together with BEGIN padding. The hook supports it,
  because `handle` is entered at body receipt.

## 4. State of items 1-3

Work items 1-3 of the brief's "Order of work", which WP-A/B/C were meant
to close:
- **1 (connect primitives, vectors, `pair_status`, caps): DONE once F1 is fixed**
  (F7 is cosmetic).
- **2 (config keys, startup validation, `listener_id` in `server_meta`): DONE**
  (F3 and F8 are open).
- **3 (transport hook, routing before the gate, body reading, error-body flag,
  write override): PARTIAL** (code done; F2, F4 and F5 are open).

Brief acceptance items 1-3:
- **Acceptance 1 (routes): PARTIAL.** The transport behaviour is implemented
  and unit-tested for 404, 405, 415 (params, empty `Content-Encoding`),
  413 (declared and streamed), no `100` on rejection, headers and HSTS.
  Nothing in `shamir-server` implements `LoginRoutes` or calls
  `serve_with_login`, so a real `kind: http` listener with `login.enabled: true`
  still serves `NoLogin`'s 404.
- **Acceptance 2 (strict wire): PARTIAL (only the transcript and scope
  field bounds).** The strict MessagePack reader and the begin/finish request
  types do not exist.
- **Acceptance 3 (challenge): PARTIAL.** The persisted `listener_id` and the
  configured override parsing exist, and the stable-across-reopen test is
  present. The BEGIN service, the 12-key challenge, the class selection, the
  fake/real salt and tuple, and the 50-75 ms padding do not exist.

### Finishing list inside items 1-3

1. F1 (one line), then CI's fast tier for `shamir-connect`.
2. F2, F3, F4 and F5 (tests and visibility), then F6 and F8; F7, F9, F10 and
   F11 when convenient.
3. Acceptance 1, application side (revision 2, package 3):
   `security/perimeter/http_login/routes.rs` implements `LoginRoutes`
   (`enabled` from `login.enabled`, `body_limit` 4096/256, `admit_head` with
   the decision-6 order); the launcher (`server_launcher/http/launcher.rs`)
   uses `serve_with_login`, computes the class (TLS -> `0x01`; loopback
   plaintext -> `0x00`; `external_tls` -> `0x02`), builds the default
   authority list from the actual bound port, and resolves `listener_id` as
   the configured value, or else `ServerMetaStore::http_login_listener_id(addr)`.
   For e2e, launch (c) checks 404 on both paths for GET and POST plus bearer
   `Ping`, and launch (d) checks TLS HSTS.
4. Acceptance 2 (revision 2, package 1): the strict reader (positive fixint
   and every unsigned width including non-shortest; signed markers rejected;
   duplicate, missing, unknown and mistyped keys; trailing bytes; maps only;
   exact `bin` lengths; strict UTF-8), `BeginRequest`/`FinishRequest`
   validation calling `encode_scope`/`build_transcript`, the canonical-user
   check (`NormalizedUsername::from_raw(u)? == u` byte for byte), and the
   acceptance-2 table test with multi-line literals.
5. Acceptance 3 (revision 2, package 3): the BEGIN service producing exactly
   the 12 keys, with `security` equal to the listener class, the resolved
   `listener_id`, the real salt and tuple when eligible and the fake blob plus
   the default tuple otherwise, equivalent work across the three branches,
   and padding to 50 ms plus a CSPRNG 0..=25 ms from body receipt (and the
   same for FINISH). The e2e restart on the same data directory keeps
   `listener_id`, and a configured value is honoured.

## 5. What I could not verify

- Anything that needs rustc or clippy to actually run. In particular: the
  E0308 in F1 is reasoned from rustc's coercion rules plus the codebase's own
  `&x[..]` convention; whether clippy 1.95 adds a lint I do not know of;
  hyper's close behaviour when a `Connection: close` response leaves a
  declared body unread (the 413/403 route tests assume it closes, as other
  transport tests already do).
- ktav specifics: that a nested inline `login: { .. }` map passes through
  the `#[serde(flatten)]` buffer (F3 adds the positive control that proves
  it); whether an all-decimal `listener_id` or a bracketed IPv6 entry
  (`[::1]:7340`) inside an inline array parses as a string rather than an
  integer or a nested array (operator-facing; a parse test with
  `authority_allowlist: ["[::1]:7340"]`-style input should be added once the
  ktav quoting syntax is confirmed); `null` for `Option`.
- The runtime of the new tests beyond the hand traces (route tests, canned
  tests, the cap test with `TokensConfig::default()`, and the fjall reopen
  inside `http_login_listener_id_is_stable_across_reopen_and_address_scoped`).
- PRECIS outcomes for `"a\u{301}"` and `"é"` (expected: rejected and accepted).
  They depend on the `precis-profiles` tables.
