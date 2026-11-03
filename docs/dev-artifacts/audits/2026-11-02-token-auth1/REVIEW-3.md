# TOKEN-AUTH1 review 3: revision 2 (`b605471f7`, master merge `63d80e2f5`)

Scope: everything revision 2 added or changed: `crates/shamir-server/src/security/perimeter/http_login/`
(`wire/`, `admission/`, `pending/`, `service/`, `routes.rs`, `metrics.rs`, `mod.rs`), the launcher
wiring (`server_launcher/http/launcher.rs`, `listeners.rs`, `launch.rs`), `tests/e2e/http_login/`
together with the `tests/e2e/http_rest` harness changes, the Part 0 transport and config edits, and
the test-folder consolidation in `shamir-connect`. I checked it against the briefs
(`auth1-login-http-server.md`, `auth1-revision-1.md`, `auth1-revision-2.md`), REVIEW-1, REVIEW-2,
the executor's DESIGN.md, the spec `TOKEN_LOGIN_HTTP.md`, and the TS client
(`origin/master:crates/shamir-client-ts/src/login/{protocol.ts,core.ts}`).

Method: nothing was compiled. I read every new or changed `.rs` file the way rustc 1.95, clippy
1.95 (workspace `absolute_paths` max 5, allowed crates std/core/alloc/tokio; `too_many_arguments`
max 7; edition 2021) and rust-guard `modules` would, and opened the target of every external API
the new code calls (`shamir-connect` crypto/lockout/fake-blob/username/audit-chain, `TokenService`,
`FjallUserDirectory`, the transport hook, the `http_rest` harness). I also ran these read-only tools:
`rustfmt +1.95.0 --check` on every changed `.rs` file (all pass, see L3);
`python -I scripts/ci/check_layout.py` (OK); and real `ls -A` entry counts. I confirmed with
`ci.yml` that the rust-guard `structure` gate only runs for active campaigns
(`.rust-guard/stages.toml` has `campaigns = []`), so CI enforces only `modules` and `orderings` from
rust-guard on this PR. Everything else in section 3 is project policy, not a CI failure.

## 0. Verdict

The revision-2 code is complete enough to compile and test, and its structure matches the brief.
Strict reader, admission tables, pending store, BEGIN/FINISH, transport hook implementation,
launcher wiring and two e2e launches are all present. I found **two compile errors** (one in a
`shamir-server` unit test, one in the e2e binary) and **two clippy `-D warnings` hits**. All four
are one- or two-line fixes (section 1). With them fixed, I expect the fast tier to compile.

The **wire format matches the TS client** byte for byte: the 12-key challenge, the 11-key success
response, `{error}`, `{error, retry_after}` with a matching `Retry-After`, the five mandatory
headers, and 404 before the body. The transcript, constant-time compares, fake-verifier parity,
admission order and pending lifecycle follow the spec.

Two findings are **P1**:

- **S1 (lifecycle).** FINISH workers live in a `JoinSet` owned by `HttpLoginRoutes`. If the server
  force-aborts the listener after its 10 s drain budget, that `JoinSet` is dropped, which aborts the
  workers between `TokenService::create` and the post-create recheck or compensation. This is the
  risk the executor flagged; it is confirmed.
- **S4 (CPU DoS).** At table capacity, the ARRIVAL/CREDIT tables scan for an eviction candidate in
  O(N²) (up to about 16.8 M slot visits per new source per table) while holding the one admission
  mutex. That is a CPU-amplification DoS on every login.

Acceptance items 4–11 are PARTIAL (section 6); item 12 is DONE. Sizing: about 2 h of editing gets
to a green fast tier, fixes S1 and S4, and closes the cheapest coverage gaps (section 7).

## 1. Compile errors (P0)

**C1. `service/tests/begin_tests.rs:38` and `:90` — E0308: expected `Subnet`, found `[{integer}; 16]`.**
`PairKey` is `(Subnet, [u8; 16])`, and `Subnet` is an enum (`V4([u8; 3])` / `V6([u8; 8])`,
`shamir-connect/src/server/limits/lockout.rs:117`). The oracle was also vacuous: a zero key never
has a backoff. Replace both calls with the real pair of the request:

```rust
    assert_eq!(
        fixture.lockout.current_backoff_ms(
            fixture.context.pair_key("alice", source().address()),
            90_000_000_000,
        ),
        0
    );
```

(`pair_key` is `pub(crate)` on `LoginServiceContext`; `LockoutStore` is already imported.)

**C2. `tests/e2e/http_login/login_flow.rs:17` — E0603: constant `ADMIN_PASSWORD` is private.**
`tests/e2e/http_rest/common.rs:17` declares it `pub(super)`, which makes it visible inside
`http_rest` only. Widen it the same way `grant_default_main_access` was widened:

```rust
pub(crate) const ADMIN_PASSWORD: &[u8] = b"http-rest-admin-password";
```

The fast tier stops at the first failing crate. These are the only compile errors I found in
`shamir-server` (lib, unit tests, e2e) and in the `shamir-transport-http` / `shamir-connect` edits.
I checked: every `use` target, re-export visibility (`wire::response` module vs function
namespaces, private `metrics` module reached from descendants), every struct literal of an
extended or new type (`ListenerEnv` has one literal; `LoginServiceOptions` 2; `PendingEntry` 3;
`TokenView`, `NewToken`, `HttpLoginTranscriptInput`, `UserRecord`, `TokenServiceOptions`, `Context`,
`HttpServerConfig`), and every caller of a changed signature (`start_http_listener` has 1 caller
with 7 args; `LoginResponse::body` now returns `&Bytes`, and its only caller clones). I also checked
moved values, including partial moves out of `FakeBlob` / `DerivedKeys`, neither of which has a
`Drop` impl. Finally I checked `Send`/`'static` of the spawned worker and of both RPITIT futures,
and `Option<&String>` to `Option<&str>` coercion through `Some(..)`.

## 2. Clippy `-D warnings` (P0)

**L1. `service/begin.rs:249-261` — `clippy::let_and_return`.** `let result = …?; result`. Write
the join directly, and annotate the closure so the error type does not depend on inference:

```rust
async fn load_snapshot(
    users: Arc<FjallUserDirectory>,
    username: NormalizedUsername,
    secrets: Arc<ServerSecrets>,
    span: tracing::Span,
) -> Result<VerifierSnapshot, ()> {
    tokio::task::spawn_blocking(move || -> Result<VerifierSnapshot, ()> {
        let _entered = span.enter();
        let real = users.lookup_by_name(username.as_str());
        let user_id = users.user_id(username.as_str());
        let floor = user_id
            .and_then(|id| users.floor_if_present(&id))
            .unwrap_or(0);
        let fake = FakeBlob::derive(&secrets.server_secret, &username).map_err(|_| ())?;
        Ok((real, user_id, floor, fake))
    })
    .await
    .unwrap_or(Err(()))
}
```

**L2. `service/audit.rs:105` — `clippy::len_zero`.** `receiver.len() > 0`, and tokio 1.51's
`mpsc::Receiver` has `is_empty()`. Fix:

```rust
            let remaining = receiver.len();
            if remaining != 0 {
                metrics::counter!("shamir_login_http_audit_dropped").increment(remaining as u64);
            }
```

**L3 (P3, not a CI failure). `service/audit.rs:50-106` is not rustfmt-formatted, yet `--check`
passes.** The body of `tokio::spawn(async move { … }.instrument(..))` contains a 123-column line
that rustfmt cannot fit, so rustfmt silently gives up on the whole expression
(`--config error_on_line_overflow=true` reports `audit.rs:51`). Move the worker into a named
`async fn run_worker(receiver, failures, writer, shutdown)` and spawn
`run_worker(..).instrument(Span::current())`. rustfmt then formats it, and L2 becomes readable.

What I checked and found clean:

- `manual_async_fn`: all trait impls use `async fn`.
- `needless_range_loop`: `table.rs:206` uses `self.slots` non-indexed inside the body; `debit`
  iterates `(0..count).rev()`, which is not a range literal.
- `collapsible_if`: edition 2021, and the inner `if let` blocks are not collapsible.
- `redundant_closure`: `filter_map(|proxy| parse_cidr(proxy))` needs a `&String`→`&str` coercion.
- `unnecessary_lazy_evaluations`: `then(|| tiers[i])` indexes, so clippy keeps it lazy.
- `type_complexity`, `too_many_arguments` (max 7 everywhere), `absolute_paths` (longest is
  `shamir_connect::common::time::UnixNanos::try_now` = 5 segments, allowed).
- `MutexGuard` across `.await` in tests: none.
- `uninlined_format_args`: `head.rs:205 format!("[{}]", ip)`. Master already has many such calls
  and clippy 1.95 is green there, so this lint is not on.
- `assertions_on_constants`: `store_tests.rs:210` follows the same pattern as the passing
  `shamir-numa` `cache_padded_tests.rs:17`.
- Dead code: every new private or `pub(super)` item, enum variant and test-struct field is used or
  read.

## 3. rust-guard and layout

- Entry counts (real `ls -A`):

  | Folder | Entries |
  |---|---|
  | `perimeter/` | 5 |
  | `http_login/` | **7 (full)** |
  | `admission/` | 6 |
  | `admission/tests/` | 4 |
  | `pending/` | 3 |
  | `pending/tests/` | 2 |
  | `service/` | **7 (full)** |
  | `service/tests/` | 5 |
  | `wire/` | 7 |
  | `wire/tests/` | 3 |
  | `tests/e2e/` | 5 |
  | `tests/e2e/http_login/` | 4 |
  | `tests/e2e/http_rest/` | 7 |
  | `boot/config/` | 7 |
  | `server_launcher/http/` | 7 (unchanged since master) |

  The `wire/tests.rs` and `connect …/http_login/tests.rs` problems of the DESIGN.md report are
  resolved in the commit: the connect file was renamed to `tests/boundary_tests.rs` with the
  `#[path]` attributes removed, and `wire/` holds no `tests.rs`. DESIGN.md's "wire has 8 entries"
  is stale.
- File lengths: largest new file is `pending/store.rs` (444 lines). Largest touched files are
  `launch.rs` (823), `config.rs` (786), `http_listener_tests.rs` (548). All are under 1000. The
  layout checker reports OK.
- Module rules hold:
  - Every `mod.rs` is re-exports only.
  - Every `tests/mod.rs` is a pure manifest (`wire/tests/mod.rs` has only a `//!` doc).
  - No inline `#[cfg(test)] mod` bodies. `#[cfg(test)]` appears only on two test-seam *functions*
    (`AdmissionState::admit_slot_for_test`, `LoginBudgetTable::bucket_size`).
  - No local `use`, no `#[allow]`.
  - All four test folders are mounted under `#[cfg(test)]`, and the e2e files are mounted via
    `#[path = "http_login/mod.rs"]`. No orphan `.rs` file, so `modules` passes.
- Policy items the inactive structure gate would report (no CI failure today):
  - Visibility widenings:
    - `LoginHead::new` and the `LoginResponse` accessors went from `pub(crate)` to `pub`.
    - `http_rest::common` went to `pub(super)`.
    - `grant_default_main_access` went to `pub(crate)`, and `ADMIN_PASSWORD` will too (C2).
    - The `http_login` tree is now public library API through `lib.rs`.
  - The "one primary export per file" rule is not met:
    - `admission/table.rs` exports 5 public types.
    - `pending/store.rs` exports 5.
    - `admission/state.rs` exports 4.
    - `wire/response.rs` exports 2 structs and 4 functions.
  - Two misplacements: `impl TokenScope` sits in `begin_request.rs`, outside the type's module
    (`OWNED_IMPL` is configured for `RepoInstance` only, so nothing reports it), and
    `LoginServiceContext` lives in `issuance.rs`, which contains no issuance logic.

## 4. Security and protocol review

### Confirmed correct

- **Wire.**
  - Strict reader: exact key set via a duplicate bitmask, the nested `token` map re-parsed
    strictly, depth ≤ 3, map count ≤ 12, array16/32 ≤ 4.
  - Integers: fixint and `cc`–`cf` accepted including non-shortest forms; `d0`–`d3` rejected.
  - Strings: str8/16/32 with UTF-8 checked.
  - Fields: `bin(N)` must be exact; trailing bytes are rejected; protocol version and binding mode
    are checked.
  - Usernames are canonical byte for byte: `NormalizedUsername` of the input must equal the input,
    and the server never normalises.
  - The all-zero client nonce is rejected, and the scope is checked via `connect::encode_scope`
    (≤ 512).
  - Encoders emit exactly the 12/11/1/2-key maps the TS `exact()` checks expect.
  - TS `statusError` expectations match: 404 is body-agnostic, 405 carries
    `Allow: POST`+`method_not_allowed`, 429/503 carry `retry_after` equal to `Retry-After`,
    `Content-Type` is always `application/msgpack`.
- **Transcript and proof.** BEGIN builds `T` with the canonical user and encoded scope and stores
  `T` itself. FINISH recovers user and scope from fixed offsets (71/73/166+len) and re-validates the
  scope canonically. `verify_http_login_proof` and the `stored_key` recheck use `constant_time_eq`.
  The signature is `HMAC(server_key, T)`.
- **Enumeration parity.**
  - BEGIN: the same work for known, unknown and ineligible users (lookup + `user_id` +
    `FakeBlob::derive` in every branch). Unknown and ineligible users get the fake
    salt/`stored_key`/`server_key` plus the current default tuple. Padding is 50 ms + 0..=25 ms
    from body receipt.
  - FINISH: a proof against a fake or ineligible entry fails exactly like a wrong password —
    `register_failure` once, then `401`.
  - Privileged accounts get a normal challenge, then `403 login_account_denied` with no pair
    failure.
  - FINISH is padded the same way.
- **Admission order and pending lifecycle.**
  - Head order: transport 404/405/415/400/413, then app Host, Origin, proxy, ARRIVAL, CREDIT,
    slot. Nothing is debited for 4xx head rejections. Debits are not refunded. `ServerBusy` is
    returned without allocating state. The input-slot permit is RAII.
  - Pending: claim once, with a wrong-/24, wrong-listener or wrong-class FINISH neither consuming
    nor refreshing the entry. Claimed entries are never purged or evicted. Prefix caps count
    claimed entries. Fair-share eviction uses rejection sampling among LIVE entries with no modulo
    bias. Pins keep pending sources' budget buckets resident.
- **BEGIN and FINISH policy order.**
  - BEGIN: parse, policy (no lookup, CSPRNG or state), pair gate (`Retry-After` = ceil, clamped
    1..=3600), snapshot, pending.
  - FINISH: claim, live, pair recheck (lock, backoff or `last_fail > begin_time` gives 401 without
    accounting), proof, policy, privileged, snapshot recheck, create, then the post-create recheck
    (`created_at > floor`, key/floor/roles/privilege/eligibility unchanged, claim still live),
    otherwise revoke. `reset_on_success` happens only after durable issuance.
  - Create errors map to `token_label_exists`, `token_limit_reached`, `login_read_only_replica`,
    and everything else to 503.
- **Secrets are never logged.** The only log sites on these paths are two audit-failure warnings
  and the compensation error, which logs the token id only. There are no spans carrying the peer
  address. Audit events carry the token id or fixed counts only.
- **Wiring.**
  - Pending and admission state is process-global via `LoginResources`.
  - Class: TLS→1, `external_tls`→2, otherwise 0. The default authority aliases use the actual bound
    port.
  - Trusted proxies are exempt from the per-IP accept cap only on class 0x02.
  - The listener id is the configured value or the persisted `server_meta` value; HSTS is on for
    TLS.

### Findings

**S1 (P1) — FINISH workers can be aborted between create and compensation.**

`routes.rs:20,145-147` spawns each worker into `tokio::sync::Mutex<JoinSet<()>>`. `shutdown()`
(`:37-41`) joins without a bound. It runs inside the listener task (`launcher.rs:377`), which
`server_handle.rs:192-212` aborts after `CONN_DRAIN_TIMEOUT` (10 s). Aborting drops the last
`Arc<HttpLoginRoutes>`, which drops the `JoinSet` and aborts every worker at its next `.await`.
`TokenService::create`'s `spawn_blocking` keeps running, so a token can be created but neither
returned, rechecked nor compensated.

A second path: `handle()` waits on the `tasks` lock while `shutdown()` holds it. It then spawns
after the join finished, and that task is aborted on drop.

Fix (≈40 min, `routes.rs` + `launcher.rs`; keep the helper private in `routes.rs`, since
`http_login/` is full):

```rust
struct InFlight {
    count: AtomicUsize,
    idle: tokio::sync::Notify,
}

struct InFlightGuard(Arc<InFlight>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        if self.0.count.fetch_sub(1, Ordering::AcqRel) == 1 {
            self.0.idle.notify_waiters();
        }
    }
}

// HttpLoginRoutes: replace `tasks` with `in_flight: Arc<InFlight>`.
// handle():
        self.in_flight.count.fetch_add(1, Ordering::AcqRel);
        let guard = InFlightGuard(Arc::clone(&self.in_flight));
        let future = async move {
            let _guard = guard;
            // … unchanged body …
        };
        // Detached: dropping the routes (listener abort) must not cancel issuance.
        tokio::spawn(future.instrument(tracing::Span::current()));

/// Waits at most `budget` for in-flight login work; `false` means work continues detached.
    pub async fn shutdown(&self, budget: Duration) -> bool {
        self.stopping.store(true, Ordering::Release);
        let idle = async {
            loop {
                let notified = self.in_flight.idle.notified();
                if self.in_flight.count.load(Ordering::Acquire) == 0 {
                    return;
                }
                notified.await;
            }
        };
        tokio::time::timeout(budget, idle).await.is_ok()
    }
```

In the launcher, call `routes.shutdown(Duration::from_secs(5))`, which is under the 10 s listener
budget even with the 2 s audit drain, and log a WARN when it returns `false`. The remaining risk
should be recorded in DESIGN.md: runtime teardown at process exit still cancels a worker at an
`.await`, so a token whose response was lost stays active for at most 24 h. The spec accepts this
("response-write uncertainty leaves token active"), and it is now bounded and logged rather than
silent. Making compensation fully crash-durable needs a journal and is out of scope.

**S4 (P1) — O(N²) eviction scan under the admission mutex (CPU-amplification DoS).**

`admission/table.rs::obtain` (`:206-260`) walks all 4096 slots. For every unpinned *root* that has
children, it calls `has_children` (O(N)) and then scans every slot for pinned descendants (O(N)).
With about 2048 root+child pairs resident, one `obtain` costs about 16.8 M slot visits. `debit` can
call `obtain` once per tier, and `admit()` debits both tables while holding the single
`tokio::sync::Mutex<State>`. So every BEGIN or FINISH from a *new* /16 or /48 at table capacity
burns tens to hundreds of milliseconds of serialized CPU (more in debug builds), and a handful of
fresh prefixes per second stalls all logins. Cheap /48s make this practical. It violates the
CLAUDE.md "O(x→0)" pillar and the brief's "bounded, allocation-free critical section".

Fix (≈1 h, `table.rs` only):

1. Store `parent_slot: u16` in `Bucket`. A root stores its own index. A child takes
   `indices[count - 1]`, which is resolved first because the loop runs broad to narrow.
2. In `obtain`, after the free-slot fast path, make **one** pass that fills two stack arrays
   `has_child: [bool; BUCKETS]` and `held_child: [bool; BUCKETS]` (a child is "held" if protected
   or `pins != 0`).
3. Select candidates in a second pass, using `held_child[i]` instead of the nested scans.
4. Remove a root's subtree with one pass over `parent_slot == idx`.

That is O(N) per `obtain`. Add a churn test that fills the table with 2048 root+child pairs and
asserts a bounded number of slot visits through a `#[cfg(test)]` counter, or simply keep the
existing churn test, which then runs fast in debug.

**S2 (P2) — the audit consumer stops before the workers finish draining.**
`launcher.rs:253` creates `audit_shutdown = shutdown.child_token()`, so the *root* shutdown cancels
it at the very start of `ServerHandle::shutdown`, before `routes.shutdown()` drains the workers.
Late `issued` / `compensation_failed` events go to a closed queue and are only counted as dropped.
Fix: `let audit_shutdown = CancellationToken::new();`. Inside the listener task, hold
`let _audit_stop = audit_shutdown.clone().drop_guard();` so an aborted listener still stops the
consumer, and keep the explicit `cancel()` after `routes.shutdown`.

**S3 (P2) — one audit queue per listener.** `LoginAudit::start` is called per enabled listener
(`launcher.rs:254`), so N login listeners allow N×1024 events and N×1 MiB, against the spec's
single 1,024/1 MiB budget. Fix: create the `LoginAudit` once in `LoginResources` (it is `Clone`),
stop it from `spawn_listeners`' shutdown path, and pass clones to each listener. Otherwise, record
it as a deviation in DESIGN.md.

**S5 (P2) — metrics gaps.**

- `shamir_login_http_accounting_dropped{pair_quota|pair_table_full}` is never emitted:
  `InMemoryLockoutStore::register_failure` returns no "dropped" outcome, so this needs a
  `shamir-connect` counter or outcome variant.
- `shamir_login_http_pair_slots` is refreshed only on HTTP FINISH paths (native failures and
  `admin_unlock_user` leave it stale), and each refresh is an O(8192) scan under the lockout lock.
- Head-level rejections (404/405/415/429/503 from `admit_head`) never reach
  `shamir_login_http_total`.
- The two `*_duration_seconds` "histograms" are exported as summaries, because `PrometheusBuilder`
  (`security/observability/mod.rs:317`) has no buckets configured. The fix is
  `.set_buckets_for_metric(Matcher::Prefix("shamir_login_http_".into()), &[0.05, 0.075, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0])?`;
  verify the 0.18 signature when wiring it.

**S6 (P2) — the redaction oracle is partly blind.** `finish_tracing_redacts_credentials_and_secrets`
scopes the subscriber with `WithSubscriber`, which only covers polls on the test thread. Events
emitted inside `spawn_blocking` (directory, `TokenService` store and audit closures) go to the
thread's default dispatcher and are not captured. BEGIN is not covered at all. Either install the
capture as the global default in a dedicated nextest test, or wrap the dispatcher through
`tracing::dispatcher::set_default` inside each blocking closure under `cfg(test)`. The simpler
choice is the global default plus `#[serial]`-free isolation, since nextest runs one test per
process.

**S7 (P3) — compensation timestamps and outcome.** `compensate` revokes with the `now` captured
before `create` (`finish.rs:82,177-192`), so `revoked_at_ns < created_at_ns`. Re-read
`(context.unix_clock)()` and fall back to `created.record_view.created_at_ns`. Also, `Ok(false)`
(already revoked, or not in the mirror) is treated as successful compensation. That is acceptable,
but add a comment.

**S8 (P3) — handshake-id allocation TOCTOU.** `unique_id` checks `contains_id` (`begin.rs:264`),
then `insert_pinned` re-checks under the lock and returns `Invalid`, which becomes 503 without
retrying. A collision is 2⁻²⁵⁶, so this is cosmetic. A proper fix adds
`PendingError::Duplicate` and loops 3× over id + nonce + transcript + insert.

**S9 (P3) — smaller items.**

- `PendingId` derives `Debug`, which prints the handshake id if anything logs it; give it a
  redacted `Debug`.
- The `claim` id lookup is not constant-time. It is low risk, because a match also requires the
  same /24.
- BEGIN policy denials pad a flat 50 ms with no jitter. This is defensible under "no CSPRNG
  before policy", but record it in DESIGN.md.
- Expiry is a full scan, not the insertion-ordered queue of decision 12.
- In-flight input requests do not pin budget buckets.
- The input permit is released before the response is written, so the output budget is
  unaccounted.
- `audit.failure` counts `RateLimited`/`ServerBusy` as "authentication".
- `head.rs:150 let _ = self.bound;` only silences dead code; remove the field.
- `LoginHeadPolicy::new` rejects a non-canonical but valid `browser_origin_allowlist` entry with a
  startup error mislabelled `origin_forbidden`.
- `deploy/server.example.ktav` says the audit queue drains "with the listener task". That is true
  only on a graceful stop (S1/S2).

## 5. e2e oracles

- **Launches.** There are **2** (≤ 4): (a) plaintext loopback login plus a native seed listener,
  and (c) login disabled.
- **Grants.** Correct for an ordinary non-superuser token user: `db://default` 0o755 and
  `default/main` 0o711 via `grant_default_main_access`, plus an HMAC-signed table chmod to 0o744
  (other r--) before the bearer read. The token mode is `read_only`.
- **Request and credit budget of launch (a).** In order:

  | Requests | BEGIN/FINISH credits |
  |---|---|
  | Successful login | 2 |
  | Duplicate label (valid proof, then `403 token_label_exists`) | 2 |
  | Wrong password (`401`; registers the pair failure for `USERNAME`, which is never used again) | 2 |
  | Unknown user (`200` then `401`) | 2 |
  | Flood | 1 per request |

  The /24 holds 10 credits, so the flood's 3rd BEGIN is the first possible 429. Twelve attempts
  guarantee a 429 even with refill, because at most 10 credits plus about 0.15 refilled in roughly
  1 s of padded requests are available. ARRIVAL (10 per second) cannot trigger first, because the
  requests are padded to ≥ 50 ms. The order is correct (the 429 comes last) and the oracle checks
  `Retry-After` equals the body value. Weakness: it does not prove the 429 came from CREDIT rather
  than ARRIVAL, which is acceptable.
- **Hang safety.** Every request is wrapped in `tokio::time::timeout(15 s)`, `GET` reads are capped
  with `take(12_289)`, the server sends `Connection: close`, and the shutdown oracle is
  `report.errors().is_empty()`.
- **Oracle gaps (P3).**
  - `login()` does not assert `challenge.security == 0`. Add it; acceptance 3 requires it.
  - The disabled test does not assert `content-type: application/msgpack`.
  - Launches (b) (`login_allow_read_write=false`, superuser and replicator gives
    `login_account_denied`) and (d) (TLS class 0x01, HSTS, restart keeps `listener_id`) are
    missing. Both fit within the 4-launch budget.

## 6. Acceptance status (items 4–12)

| # | Status | Evidence / gap |
|---|---|---|
| 4 Successful login | PARTIAL (complete once C2 is fixed) | `login_flow.rs` covers proof, `server_signature` check, `id` = parsed token id, owner, TTL ≤ 24 h, Ping, and the granted table read. Unit test: `finish_issues_token_and_returns_server_signature`. Blocked only by C2. |
| 5 Failures | PARTIAL | Unit tests: `incorrect_proof_records_one_pair_failure_and_consumes_claim` (also covers replay), `wrong_source_does_not_consume_correct_claim`, `pair_failure_after_begin_skips_proof_without_accounting_again`. Store tests cover expiry at 120 s and claim-once. e2e covers wrong password and unknown user. **Missing:** an ineligible-user FINISH; the three-way parity of the *next* BEGIN's 429/`Retry-After`; a 61 s expiry driven through `finish()`; the strict `>` case `last_fail == begin_time`. |
| 6 Policy | PARTIAL | `begin_policy_denials_are_padded_and_have_no_side_effects`, `disabled_self_service_denies_valid_proof_without_pair_failure`, `valid_proof_for_privileged_user_is_denied_without_pair_failure`, `user_floor_change_before_finish_rejects_valid_proof`; the e2e duplicate label. **Missing:** replicator, `token_limit_reached`, role change, compensation injection (change the floor between create and recheck, assert the token is revoked and 401), and FINISH-time `read_write`/replica rechecks. |
| 7 Admission | PARTIAL | `table_tests` (5 tests covering tiers, refill, child quota, churn, pins), `slot_tests` (5), `store_tests` (7: caps, fair-share, all-claimed, entropy), e2e 429. **Missing:** an explicit atomic all-or-nothing/no-refund test, a test of the `Dropped` fallback, a 503 response-mapping test (body plus `Retry-After: 1`) showing existing pending state is untouched, and the S4 fix. |
| 8 Classes/proxies | PARTIAL | Revision-1 config tests; `head_tests` (Host/Origin table, class-2 trust walk, mapped IPv4, default loopback aliases). **Missing:** a class-0 test where a non-loopback peer gets `proxy_forbidden`, and a TLS e2e (launch d). |
| 9 Lockout | PARTIAL | HTTP uses the shared `InMemoryLockoutStore` and `pair_key` = (`subnet_of`, `username_hash`); the curve, lock and idle reset are AUTH-PRE code. **Missing:** a native↔HTTP shared-pair test, `admin_unlock_user` through HTTP, and pair-drop accounting (S5). |
| 10 Metrics | PARTIAL | All 8 names exist. **Missing:** the `pair_quota`/`pair_table_full` reasons, real histogram buckets, head-level totals, and live `pair_slots` (S5). |
| 11 Audit/redaction | PARTIAL | `captures_issued_and_aggregate_events_without_identity`, `captures_compensation_event_with_token_id_only`, `failures_count_only_fixed_classes`, and FINISH-only redaction. **Gaps:** S2 (shutdown order), S3 (per-listener queue), S6 (blind oracle, no BEGIN coverage). |
| 12 Transcript vectors | DONE | Green on `4cd13b977`. Revision 2 only renamed `tests.rs` to `tests/boundary_tests.rs` and dropped the `#[path]` attributes. |

## 7. Finishing list (about 2 h of editing)

1. **5 min.** Fix C1, C2, L1 and L2 exactly as written in sections 1 and 2. The fast tier then
   compiles.
2. **15 min.** Fix L3: extract `audit.rs`'s worker into `async fn run_worker(..)`, then run
   `cargo fmt -p shamir-server`.
3. **40 min.** Fix S1 (detached workers, in-flight counter, bounded `shutdown(budget)`, WARN on
   timeout) and S2 (independent audit token plus `drop_guard`). Record the remaining risk in
   DESIGN.md.
4. **60 min.** Fix S4 (`parent_slot` and the one-pass marking in `obtain`), keeping the existing
   `table_tests` green.
5. **Remaining time, cheapest first.** Each is a unit test using the existing fixture:
   - Add `assert_eq!(challenge.security, 0)` in e2e `login()`.
   - Ineligible FINISH registers exactly one failure: hardening on with a mismatched default, which
     reuses `hardened_mismatched_tuple_uses_fake_salt_and_current_default`.
   - Compensation injection: wrap `unix_clock` or bump the floor from a `snapshot` seam, then
     assert the mirror count and the 401.
   - S7: refresh the clock in `compensate`.
6. **Before pushing.** CLAUDE.md allows locally `cargo check -p shamir-connect -p
   shamir-transport-http -p shamir-server --all-targets`, `cargo clippy` with the same `-p` set
   plus `-- -D warnings`, `cargo fmt -p … -- --check`, and `python -I scripts/ci/check_layout.py`.
   The revision briefs forbade the *executor* from compiling, not the orchestrator. Running clippy
   locally settles every "expected" claim in sections 1 and 2 in one pass instead of one fast-tier
   round trip per crate.

Deferred, to record in DESIGN.md as known deviations:

- S3 per-listener audit queue.
- S5 pair-drop metrics, which need a `shamir-connect` change.
- Histogram buckets (observability file).
- e2e launches (b) and (d).
- The remaining S9 items.

## 8. Severity counts and limits

**Counts.**

| Severity | Count | Items |
|---|---|---|
| P0 | 4 | C1 (two sites), C2, L1, L2 |
| P1 | 2 | S1, S4 |
| P2 | 4 | S2, S3, S5, S6 |
| P3 | about 15 | L3, S7, S8, the S9 list, the section 3 policy items, the section 5 oracle gaps |

**What I could not verify.**

- Nothing was compiled or run. Clippy verdicts come from reading against 1.95 lint defaults, and
  the master-precedent check settled `uninlined_format_args`.
- `len_zero` (L2) and `let_and_return` (L1) are expected but not observed.
- I did not open the full `TokenService::open` path to see whether it spawns timers that interact
  with `start_paused` in the service tests.
- Runtime numbers for S4 are estimates from slot-visit counts.
- The `metrics-exporter-prometheus 0.18` bucket API signature: only 0.16.2 is in the local cargo
  registry.
- Whether hyper closes the connection immediately after an early 408 on a partially read body
  (`stalled_body_times_out_after_three_seconds`). I relied on `Connection: close` semantics.
- The TS client's `node-http.ts` request headers. I read only `core.ts` and `protocol.ts`, as
  instructed.
