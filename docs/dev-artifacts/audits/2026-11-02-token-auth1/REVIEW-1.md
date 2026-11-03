# TOKEN-AUTH1 review 1: the executor's partial, uncommitted work

Scope: the uncommitted tree of `fix/p34-auth1-login-server` on top of master
`f11889057` (brief commit `f24a7999a`), reviewed against
`docs/dev-artifacts/prompts/remediation/p34/auth1-login-http-server.md`,
`docs/guide-docs/client-server-protocol-spec/TOKEN_LOGIN_HTTP.md` and the TS
client on `fix/p34-auth2-login-ts` (`src/login/protocol.ts`, `core.ts`).
Nothing was compiled. Every finding below comes from reading the code. The
transcript vectors were also recomputed independently with a Python one-liner
from the spec section 2 layout.

## 0. What was actually done

The executor got through part of work item 1 ("shamir-connect: scope bytes,
transcript, proof, vectors, `pair_status`, record caps") and started nothing
else. Its own note (`DESIGN.md`) says so: "Work is limited to numbered item 1".
No file under `crates/shamir-transport-http/`, `crates/shamir-server/src/boot/`,
`crates/shamir-server/src/security/perimeter/`, `server_launcher/`,
`server_meta.rs`, `deploy/` or `tests/e2e/` was touched.

Changed (tracked): `shamir-connect/src/server/{auth/mod.rs, mod.rs, limits/mod.rs,
limits/lockout.rs, auth/token/record.rs, tests/security/lockout_tests.rs,
tests/token_tests/record_tests.rs}`,
`shamir-server/src/security/tokens/{service/token_service.rs, tests/mod.rs,
tests/service_tests.rs}`, `shamir-server/tests/misc/runtime/scheduler.rs`.
New (untracked): `shamir-connect/src/server/auth/http_login/{mod.rs, proof.rs,
scope_bytes.rs, transcript.rs, tests.rs, tests/mod.rs}`,
`shamir-connect/test-vectors/http_login_transcript_v1.json`,
`shamir-server/src/security/tokens/tests/cap_tests.rs`, `DESIGN.md`.

## 1. Brief components vs. state (dependency order)

| # | Component (brief decision) | State | Evidence |
|---|---|---|---|
| 1a | Scope encoder (D1, D16) | PARTIAL | `http_login/scope_bytes.rs`. Layout matches spec section 2 and `protocol.ts::encodeScope`. Database names wrongly run through `UsernameCaseMapped` (defect B1). Not rustfmt-clean. |
| 1b | Transcript builder `T` (D1, D16) | PARTIAL | `http_login/transcript.rs:37-76`. Field order and widths match spec section 2 and `protocol.ts::buildLoginTranscript` (166 fixed bytes, domain 20 bytes, U16 user length, U32 KDF, U8 version). Same database-name defect (B1). |
| 1c | Proof verify over `&[u8]` + server signature (D1) | DONE (code) | `http_login/proof.rs`. Constant-time `constant_time_eq`, recovered client key in `Zeroizing`, does not reuse `AuthMessage`. |
| 1d | Transcript vectors (D16) | PARTIAL, WRONG | `test-vectors/http_login_transcript_v1.json`: both `transcript_hex` values are 1 byte short (T1). There is no TOML mirror, the README is not updated, and the test needs `serde_json`, which is not a dependency (C2). |
| 1e | `LockoutStore::pair_status` (D15) | PARTIAL | Trait method plus impl at `limits/lockout.rs:206,610-625`. Does not compile (`ns::MILLISECOND`, C1). Both implementors updated (`InMemoryLockoutStore`, scheduler test stub). The test has wrong expected values and a missing import (C4, T5). |
| 1f | Label/database caps 128/255 (D14) | DONE (code) | `auth/token/record.rs:101-106` and `token_service.rs:163,172`. `record_tests.rs` boundaries updated correctly (`"é".repeat(127)+"x"` = 255 bytes). `cap_tests.rs` has issues (P2-1). |
| 2 | Config keys + startup validation (D4, D9) | MISSING | No `boot/config/login_listener.rs`. `TokensConfig`, `SecurityConfig` and `HttpListenerSettings` are unchanged. |
| 2b | `listener_id` get-or-create in `server_meta` (D10) | MISSING | `engine/server_meta.rs` untouched. |
| 3 | Transport route hook `LoginRoutes`, `serve_with_login`, `NoLogin`, body reader, GuardedIo error bodies + write-timeout override (D2-D5) | MISSING | `crates/shamir-transport-http/` untouched. `src/server/` still has 6 entries and no `routes/`. |
| 4 | Strict msgpack reader, begin/finish request + response types | MISSING | No `security/perimeter/http_login/`. |
| 5 | ARRIVAL/CREDIT tables, input slots (D12) | MISSING | none |
| 6 | Pending store (D12) | MISSING | none |
| 7 | BEGIN/FINISH services, eligibility + fake verifier (D11), issuance + compensation (D13), padding, audit queue (D17), metrics | MISSING | none |
| 8 | Launcher wiring, startup ineligible count, `deploy/server.example.ktav` | MISSING | `server_launcher/http/launcher.rs`, `deploy/` untouched |
| 9 | e2e `tests/e2e/http_login/` + `Harness` hook constructor | MISSING | `tests/e2e/` has its 4 master entries |
| 10 | DESIGN.md design section + final report | MISSING | `DESIGN.md` has a 1-paragraph summary. It lacks the required hook trait, module map, memory charges, concurrency argument, widenings list and final report. |

Brief acceptance items 1-12:

| Item | State |
|---|---|
| 1 Routes / 404 / 405 / 415 / 413 / 408 / headers | MISSING |
| 2 Strict wire | MISSING (only the scope/transcript field bounds exist, in 1a/1b) |
| 3 Challenge | MISSING |
| 4 Successful login | MISSING |
| 5 Failures | MISSING (only the `pair_status` primitive exists, and it does not compile) |
| 6 Policy | MISSING (only the cap change exists) |
| 7 Admission | MISSING |
| 8 Security classes / proxies / Host / Origin | MISSING |
| 9 Lockout | PARTIAL (read probe only; the HTTP pair gate is not written) |
| 10 Metrics | MISSING |
| 11 Audit + redaction | MISSING |
| 12 Transcript vectors, T bounds, builder rejects out-of-range fields | PARTIAL (builder logic correct except B1; vectors wrong; tests broken) |

## 2. Defect list (severity-ordered)

### P0: compile errors (the whole `shamir-connect` lib, and so every dependent crate, does not build)

**C1. `ns::MILLISECOND` does not exist.** `crates/shamir-connect/src/server/limits/lockout.rs:621`.
`common/runtime/time.rs::ns` defines only `SECOND`, `MINUTE`, `HOUR` and `DAY`
(E0425). This breaks the non-test lib build of `shamir-connect`, so
`shamir-server` and everything downstream fails too. Fix, rustfmt-shaped (it
also removes the double blank line at `lockout.rs:192-193`):

```rust
    fn pair_status(&self, key: PairKey, now_ns: u64) -> PairStatus {
        let slots = self.slots.lock();
        let Some(index) = lookup_pair(&slots, key) else {
            return PairStatus {
                locked_until_ns: None,
                backoff_until_ns: None,
                last_fail_at_ns: None,
            };
        };
        let slot = &slots[index];
        let locked_until_ns = slot
            .lockout
            .and_then(|lock| lock.triggered_at_ns.checked_add(lock.duration_ns))
            .filter(|until| now_ns < *until);
        let failure = slot.failure;
        let last_fail_at_ns = (failure.last_fail_at_ns != 0).then_some(failure.last_fail_at_ns);
        let backoff_until_ns = if failure.count != 0 && !failure.is_stale(now_ns) {
            failure
                .backoff_ms()
                .checked_mul(ns::SECOND / 1_000)
                .and_then(|duration| failure.last_fail_at_ns.checked_add(duration))
                .filter(|until| now_ns < *until)
        } else {
            None
        };
        PairStatus {
            locked_until_ns,
            backoff_until_ns,
            last_fail_at_ns,
        }
    }
```

Also wrap the overlong trait doc at `lockout.rs:205` onto two `///` lines.
rustfmt does not wrap comments, but the line is 150+ characters.

**C2. `serde_json` is not a dependency of `shamir-connect`.** `http_login/tests.rs:42`.
The `[dev-dependencies]` are `hex`, `toml`, `bench-scale-tool`, `tempfile` and
`tokio`, and `Cargo.toml` must not change. The repo convention
(`test-vectors/README.md`) is a JSON+TOML pair, with Rust reading the TOML
through `include_str!` + `toml::from_str`. Fix: add
`test-vectors/http_login_transcript_v1.toml` with `[[cases]]` tables and put
`databases` only in the second case (a missing key decodes to `None` with
`#[serde(default)]`). Load it with
`include_str!("../../../../../test-vectors/http_login_transcript_v1.toml")`,
counting the path from the new test file in `http_login/tests/`. Drop
`std::fs`, `PathBuf` and `env!("CARGO_MANIFEST_DIR")`. `hex::decode` (a
dev-dependency) can replace the hand-written `decode_hex`.

**C3. `LockoutStore` trait not in scope in `http_login/tests.rs`.** Lines 7 and
233-251 call `store.pair_status(..)` and `store.register_failure(..)`, both
trait methods, but line 7 imports only `InMemoryLockoutStore, PairKey,
PairStatus, Subnet` (E0599). Best fix: delete this test. It duplicates the
lockout test, and the brief puts `pair_status` tests next to the lockout
tests. Otherwise add `LockoutStore` to the import.

**C4. `PairStatus` not imported in `server/tests/security/lockout_tests.rs`.**
The import block at lines 2-6 is unchanged, but the new test (line 209ff) names
`PairStatus` (E0422). Fix: add `PairStatus` to the `use crate::server::lockout::{..}` list.

### P0 (likely): warnings under `-D warnings`

**W1. `crates/shamir-connect/src/server/limits/mod.rs:4` `pub use lockout::PairStatus;`.**
`limits` is a private module (`server/mod.rs:11`) and the server re-exports the
module `limits::lockout`, not this item. The re-export is therefore never
exported and never used, and current rustc reports such non-exported `pub use`
as `unused_imports`. I cannot run rustc to confirm. Delete the line either way:
`shamir_connect::server::lockout::PairStatus` is already the public path and
`scheduler.rs` uses it.

### P1: tests that compile (after C1-C4) but fail

**T1. Both transcript vectors are 1 byte short (172 and 178 bytes, while the
formula and the builder give 173 and 179).** In the KDF block the executor
typed `00004c00 000002 00000001` instead of `00004c00 00000002 00000001`, so
`assert_eq!(transcript, decode_hex(..))` fails for both cases. The independently
recomputed correct values (scope hex fields are already correct) are:

```
null_databases_read_only_rest (173 bytes):
5348414d49522d485454502d4c4f47494e2d7631010300000000000000000000000000000000000303030303030303030303030303030303030303030303030303030303030303000161010101010101010101010101010101010101010101010101010101010101010102020202020202020202020202020202020202020202020202020202020202020404040404040404040404040404040400004c000000000200000001130001780000ff
two_sorted_databases_read_write_both (179 bytes):
5348414d49522d485454502d4c4f47494e2d7631010300000000000000000000000000000000000303030303030303030303030303030303030303030303030303030303030303000161010101010101010101010101010101010101010101010101010101010101010102020202020202020202020202020202020202020202020202020202020202020404040404040404040404040404040400004c00000000020000000113000178010202000161000162
```

Both cases also use class `0x00` and an all-zero listener id. Add a third case
with `security = 0x02`, a non-zero `listener_id`, a multi-byte UTF-8 user and
four databases. That case guards the security byte and the listener field
order, and it is cheap. All vectors stay hand-derivable.

**T2. `transcript_rejects_invalid_metadata_kdf_username_nonces_and_scope_shapes`
never resets `memory_kb` / `time`.** `http_login/tests.rs:152` leaves
`memory_kb = 262_145`, so `input.time = 2; assert!(..is_ok())` at line 154
fails. Line 156 leaves `time = 9`, so line 158 would fail the same way. Fix:
add `input.memory_kb = 19_456;` after line 152 and `input.time = 2;` after
line 156. Also split the one-line multi-statement lines into one statement
per line (fmt).

**T3. Same test, line 180: `[0,1,b'x',0,0, 1, 0,2,b'b',b'a']` is a VALID
scope.** It is count 1 with the single database `"ba"`, but the test asserts
`is_err()`. The intended case "unsorted" needs two entries:
`&[0, 1, b'x', 0, 0, 2, 0, 1, b'b', 0, 1, b'a'][..]`.

**T4. `proof_round_trip_..`, line 202:
`&proof[..31].try_into().expect("fixed proof")` panics.** It compiles (the
target is `[u8; 32]: TryFrom<&[u8]>`), but the slice has 31 bytes, so the
`expect` fires and the test fails. `ClientProof` is a fixed `[u8; 32]`;
truncation is a wire-parser concern (work item 4). Delete the line.

**T5. `lockout_tests.rs::pair_status_is_read_only_..` has wrong expected
values.** The 49 failures are at `2e9 + n*1e9`, `n = 1..=49`. Together with the
failure at `1e9`, the 50th failure lands at `51_000_000_000`. `register_failure`
(`lockout.rs:563-597`) does not reset on a 2 s gap and locks at
`count >= 50` with `triggered_at_ns = now`. The test expects `2_049_000_000`
in three places (lines 220, 222, 225). With those values, `stale` at
`2_049_000_000 + 1h + 1` is still inside the lock, so line 223 fails too. Fix:
replace each `2_049_000_000` with `51_000_000_000`. The duplicate in
`http_login/tests.rs:246-251` already uses the right numbers.

### P1: behaviour contradicting the spec and the client

**B1. Database names are case-mapped through `NormalizedUsername::from_raw`
(PRECIS UsernameCaseMapped).** This happens in `scope_bytes.rs:51-52` and
`transcript.rs:109-111`. The check maps `"Main"` to `"main"`, compares that
with the input, and rejects the name. The client accepts `^[A-Za-z0-9_-]+$`
(`protocol.ts:9,154`), so a client scope with an uppercase database gets
`400 invalid_request` from this server. The spec asks only for NFC, and an
ASCII-only name is trivially NFC. Fix: delete both normalisation blocks (and
the then-unused import in `scope_bytes.rs:1`). The ASCII syntax check already
implies NFC. Add a test that `"Main"` and `"A-b_9"` are accepted.

### P1: CI gates

**G1. rust-guard `modules`: orphan `http_login/tests/mod.rs`.** `http_login/mod.rs:7-9`
mounts `#[path = "tests.rs"] mod tests;`, so `tests/mod.rs` (one comment line)
is unreachable. Once committed, `rust-guard modules --repo .` (runs on every PR
and master push) fails. The layout also breaks the CLAUDE.md test convention
(`tests/` folder with a pure manifest). Fix: move the test bodies into
`http_login/tests/{transcript_vector_tests.rs, scope_bytes_tests.rs, transcript_tests.rs, proof_tests.rs}`,
make `tests/mod.rs` a pure manifest (`mod scope_bytes_tests;` and so on),
change `mod.rs` to plain `#[cfg(test)] mod tests;`, and have the orchestrator
delete `http_login/tests.rs`. The brief forbids executors from deleting files.
Entry count afterwards: `http_login/` = 5, `http_login/tests/` = 5.

**G2. `cargo fmt -- --check` fails in at least these files:**
`http_login/{proof.rs, scope_bytes.rs, transcript.rs, tests.rs}` (one-line
`if {..}` bodies, lines > 100 chars, packed struct literals);
`limits/lockout.rs:192-193,610-625`; `tests/security/lockout_tests.rs:209-226`;
`shamir-server/src/security/tokens/tests/cap_tests.rs:10,14`. Run
`cargo fmt -p shamir-connect` and `cargo fmt -p shamir-server` after the fixes.

**Layout (real `ls` counts) is within limits:** `server/auth/` 6,
`auth/http_login/` 6 (5 after G1), `http_login/tests/` 1, `server/limits/` 5,
`server/tests/security/` 6, `shamir-server/src/security/tokens/tests/` 5.
`lockout.rs` is 700 lines, `http_login/tests.rs` 252. `check_layout.py` passes,
but it ignores untracked files.

### P2: quality, rules and brief conformance

1. **`cap_tests.rs`** has three function-local `use` statements at lines 5-7
   (CLAUDE.md "Imports at the top", rust-guard `LOCAL_USE`) and the 6-segment
   path `crate::security::tokens::tests::service_tests::service(..)` at line 10
   (the `absolute_paths` rule, max 5). Hoist to the top:
   `use super::service_tests::{new_token, record, service}; use std::sync::Arc; use tempfile::TempDir;`.
   Related: three helpers in `service_tests.rs` (`record`, `new_token`, `service`)
   were widened from private to `pub(super)`. The rust-guard `structure` gate
   is a no-op today (`campaigns = []`), but the brief requires every widening
   to be listed in DESIGN.md. Either list them or move this one test into
   `service_tests.rs` (897 + ~25 lines, under 1000) and revert the widening.
2. **`scheduler.rs:62,70-72`**: the gc-counting stub now owns a real
   `InMemoryLockoutStore` (an 8,192-slot, 1 MiB table) only to answer
   `pair_status`. The underscore-named `_key` is also used. Replace with a
   constant and drop the `inner` field:
   `fn pair_status(&self, _key: PairKey, _now_ns: u64) -> PairStatus { PairStatus { locked_until_ns: None, backoff_until_ns: None, last_fail_at_ns: None } }`.
3. **`HttpLoginMode` / `HttpLoginChannels` (`scope_bytes.rs:3-21`)** duplicate
   `server::token::{TokenMode, TokenChannels}` (`auth/token/record.rs:4-24`):
   same variants, and the latter carry serde names. Reusing them removes a
   mapping in issuance (D13) and two public types. Recommended, not mandatory.
4. **`validate_scope_bytes` (`transcript.rs:78-117`)** does not check that the
   label bytes are UTF-8. Today it only sees bytes produced by `encode_scope`
   from a `&str`, so this is harmless, but "the builder rejects every
   out-of-range field" (acceptance 12) is literal. Add
   `std::str::from_utf8(&scope[2..label_end]).map_err(|_| "invalid scope label")?;`.
   The `scope.len() < 2` check at line 79 is redundant with line 80.
5. **The proof round-trip test** uses an arbitrary `client_key` instead of the
   key schedule the brief names ("client key schedule from `scram.rs`"). Use
   `hmac_sha256(&salted, domain_tags::CLIENT_KEY)` /
   `hmac_sha256(&salted, domain_tags::SERVER_KEY)` with a fixed 32-byte
   `salted`, which needs no Argon2. Then check
   `build_server_signature(server_key, T)` against a client-side recomputation.
6. **`test-vectors/README.md`** does not list the new file. Add one line:
   "message-layout fixture, hand-derived; TS client consumes the JSON".
7. **DESIGN.md** is missing every required section (brief "Rules", last
   bullet). It also says "the nested tests/mod.rs artifact remains nonempty and
   unused", which is the G1 defect described as accepted.
8. **Spec point for later packages, not a defect in the existing code:** spec
   section 4 also requires FINISH padding ("Pad finish from body receipt to
   50 ms plus uniform integer jitter 0..25 ms"). Brief decision 11 only
   mentions BEGIN padding. The FINISH service must pad too.

### Crypto check summary (existing code)

- The transcript byte order and widths match spec section 2 and
  `protocol.ts::buildLoginTranscript` exactly: 20-byte domain without NUL,
  `01 03 sec`, 16 + 32 raw, U16 BE user length, user, 32 + 32 + 16 raw, three
  U32 BE values, U8 `0x13`, then the scope. Scope bytes match
  `protocol.ts::encodeScope`, including `0xFF` for null databases. The only
  wire mismatch is B1.
- The builder rejects version != 1, binding != 3, security > 2, a non-canonical
  user, a KDF tuple outside 19,456..262,144 / 2..8 / 1..8 / `0x13` (via
  `validate_client_limits`, whose bounds match `types.rs:123-148`), an all-zero
  client nonce, and a server nonce that is all zero or equal to the client
  nonce. This mirrors the client's checks.
- Proof verification: `HMAC(stored_key, T)` XOR proof, then `SHA256`, compared
  with `subtle` constant time. The recovered key is zeroized. The server
  signature is `HMAC(server_key, T)`. This matches the client
  (`core.ts:394-400`) and the Rust `domain_tags` key schedule.
- Enumeration uniformity and padding cannot be judged yet: no service code
  exists.

## 3. Finishing plan

Honest sizing: the whole brief (items 2-10) is far more than 2 hours of
editing. Items 4-7 alone (strict reader, two 4,096-entry hierarchical credit
tables with quotas/eviction/pinning, input slots, pending store with fair-share
eviction, BEGIN/FINISH with eligibility, issuance compensation, padding, audit
queue, 8 metrics) are several thousand lines with tests. The three packages
below take about 2 h in total and close items 1-3 to a consistent,
CI-checkable state. Items 4-10 need a follow-up brief, sketched at the end.
The file sets are disjoint. DESIGN.md is owned by the orchestrator: each child
reports its design and progress lines in its final message and does not edit
the note.

Order: WP-A first (it unblocks compilation of every crate). WP-B and WP-C can
run in parallel after WP-A and do not depend on each other. Each package
leaves the tree compiling and clippy-clean by construction: new items are
`pub` in already-exported modules (`shamir_server::config`,
`shamir_server::server_meta`, the `shamir_transport_http` crate root), so
nothing is dead code before the launcher uses it.

### WP-A: close item 1 (about 30 min) - `shamir-connect` + two test stubs

Files: `crates/shamir-connect/src/server/limits/{lockout.rs, mod.rs}`;
`crates/shamir-connect/src/server/auth/http_login/{mod.rs, scope_bytes.rs, transcript.rs, proof.rs, tests/**}`
(new topic files); `crates/shamir-connect/src/server/tests/security/lockout_tests.rs`;
`crates/shamir-connect/test-vectors/{http_login_transcript_v1.json, http_login_transcript_v1.toml (new), README.md}`;
`crates/shamir-server/tests/misc/runtime/scheduler.rs`;
`crates/shamir-server/src/security/tokens/tests/{cap_tests.rs, service_tests.rs}`.

Steps:
1. C1 (code above), W1, C4, T5.
2. B1 in both files, plus the acceptance test for uppercase names; P2-4.
3. G1: write the four topic test files from `tests.rs`, applying C2 (TOML),
   C3 (drop the duplicate pair_status test), T1 (corrected hex plus a third
   case), T2, T3, T4 and P2-5. Make `tests/mod.rs` a manifest and change
   `mod.rs` to `#[cfg(test)] mod tests;`. Leave `tests.rs` for the
   orchestrator to delete. (Until it is deleted it is an unreferenced file
   that rustc ignores; rust-guard sees it only if it is committed.)
4. P2-1 and P2-2. Optional: P2-3, which renames the public API before anyone
   depends on it.
5. `cargo fmt -p shamir-connect -p shamir-server`, then re-read every touched
   file.

The orchestrator then runs `cargo check -p shamir-connect --all-targets` and
`cargo clippy -p shamir-connect --all-targets -- -D warnings` (allowed
locally) before WP-B/WP-C start, plus the same for `-p shamir-server` (the
`LockoutStore` trait gained a method).

### WP-B: item 2 (about 45 min) - configuration and listener id, `shamir-server` boot + meta only

Files: `crates/shamir-server/src/boot/config/login_listener.rs` (new, folder
goes to 7), `boot/config/{http_listener.rs, http_validation.rs, tokens.rs}`,
`boot/config.rs` (`SecurityConfig.login_enumeration_hardening` with
`#[serde(default = ..)]` true, plus the module declaration),
`boot/config/tests/{mod.rs, login_listener_tests.rs}` (new, folder goes to 3),
`crates/shamir-server/src/engine/server_meta.rs`,
`crates/shamir-server/src/tests/registry/server_meta_tests.rs`, and every
full struct literal of `HttpListenerSettings` / `TokensConfig` /
`SecurityConfig` that does not already end in `..Default::default()`. The
initial `rg` list is: `src/tests/boot/{server_tests.rs, tokens_config_tests.rs}`,
`src/security/tokens/tests/service_tests.rs` (coordinate with WP-A: only
`TokensConfig::default()` is used there today, so probably no edit),
`tests/e2e/http_rest/{common.rs, lifecycle.rs, rails.rs}`,
`tests/connection/{max_connections.rs, slow_loris.rs, slow_loris_pre_tls.rs}`,
`tests/e2e/core/shutdown_release_e2e.rs`, `tests/e2e/transport/*`,
`tests/admin/identity/permission_e2e/common.rs`,
`../shamir-client/tests/cursor/lifecycle.rs`. Inspect each hit; most use
`..Default::default()` and need nothing. Also `deploy/server.example.ktav`.

Content:
- `LoginListenerSettings { enabled: bool, authority_allowlist: Vec<String>, listener_id: Option<String> }`
  with `#[serde(deny_unknown_fields)]` and `Default`. Put it in
  `HttpListenerSettings` as a non-flattened `login` field with
  `#[serde(default)]`, plus `external_tls: bool`. `HttpListenerSettings` itself
  is flattened into `ListenerConfig`, and `deny_unknown_fields` does not
  combine with `flatten`, which is why `login` stays a nested struct.
- Add both new fields to `has_http_only_settings` in
  `http_validation.rs:23-32`, so that `login.enabled` on a non-HTTP kind is
  rejected.
- In `TokensConfig`: `login_ttl: u32 = 86_400` (validate `1..=86_400`) and
  `login_allow_read_write: bool = false`.
- In `HttpListenerSettings::validate`: every startup error of brief decision 9
  plus the 5 s head-timeout cap. `browser_origin_allowlist` and `kdf_override`
  live on `ListenerConfig`, which `validate` already receives. Authority
  canonicaliser: lowercase host, bracketed IPv6, default port normalised; keep
  it as a private fn in `login_listener.rs`. Duplicate `listener_id` across
  listeners must be checked in `Config::validate`, next to the per-listener
  loop at `boot/config.rs:628`.
- `ServerMetaStore::http_login_listener_id(addr: &str) -> Result<[u8; 16], MetaError>`,
  get-or-create under `write_lock`, modelled on `ensure_token_key`
  (`server_meta.rs:366-379`). Key: `format!("http_login_listener_id:{addr}")`,
  CSPRNG through `try_random_bytes`, length-checked on read. The file grows to
  about 925 lines (limit 1000).
- Tests: one per startup error, the defaults, `login_ttl` bounds, and the
  stable id across `ServerMetaStore` reopen.

### WP-C: item 3 (about 50 min) - transport hook, `shamir-transport-http` only

Files: `crates/shamir-transport-http/src/server/routes/{mod.rs, login/{mod.rs, login_routes.rs, login_response.rs, login_head.rs, no_login.rs, route_handler.rs}, login/tests/**}`
(new; `server/` goes to 7), `server/mod.rs`, `server/service.rs` (an early
branch only), `server/accept.rs` (`serve_with_login` only),
`framing/guarded_io.rs` (error-body flag and `Arc<AtomicU64>` write-timeout
override only), `framing/tests/{mod.rs, login_canned_tests.rs}` (folder goes
to 6), `src/lib.rs` (re-exports only).

Do NOT add a field to `HttpServerConfig`. That would force a literal sweep
into `shamir-server`'s launcher and collide with later packages. Carry the
login switch in the hook instead (`LoginRoutes::enabled()`; `NoLogin` returns
false) and pass it from `serve_with_login` into `GuardedIo::new`.

- Canned bodies are fixed msgpack byte strings: fixmap(1), fixstr `error`,
  fixstr code (for example
  `b"\x81\xa5error\xa9not_found"`; the crate has no msgpack dependency). On
  login listeners they also need `Pragma: no-cache`.
- `response()` in `server/response.rs` does not emit `Pragma` today. Build
  login responses in `routes/login/` with all five headers plus HSTS on TLS,
  instead of editing the shared helper. This keeps bearer responses
  byte-identical.
- Body reading: check the declared `Content-Length` against the limit before
  polling; streamed bytes are capped too. `413` closes the connection. A 3 s
  `timeout_at` from the end of admission gives `408`. Poll the body only after
  `admit_head` returns `Ok(permit)`, which defers `100 Continue`.
- `LoginRoutes`: implementors use `async fn` (clippy `manual_async_fn`). The
  trait needs `Send` futures (`-> impl Future<Output = LoginResponse> + Send`
  in the trait declaration, `async fn` in impls).
- Tests: the list in the brief's "Test plan, Unit, shamir-transport-http"
  (routing before the gate, NoLogin 404, 405 `Allow: POST`, 415 cases, 413
  declared and streamed, 408 with `tokio::time::pause`, no `100 Continue` on
  rejection, mandatory headers, canned error bodies, write-timeout override).
  `tokio` `test-util` is already a dev-dependency.

### Follow-up brief (not in the 2 h): items 4-10

Suggested disjoint packages under
`crates/shamir-server/src/security/perimeter/http_login/` (`perimeter/` goes
4 -> 5):
1. `wire/`: strict reader, request/response types, acceptance-2 table.
2. `admission/` + `pending/`: ARRIVAL/CREDIT, input slots, pending store with
   injected clock and seeded selection, memory-budget test.
3. `service/` + `routes.rs` + `metrics.rs`, then launcher wiring + e2e (four
   launches): BEGIN/FINISH, eligibility/fake path, issuance compensation,
   BEGIN and FINISH padding, audit queue.

Packages 1 and 2 are independent. Package 3 depends on 1, 2, WP-B and WP-C.

## 4. What I could not verify

- Anything that needs rustc or clippy. In particular: whether W1 fires as
  `unused_imports` on this toolchain (1.95); whether clippy's `absolute_paths`
  counts the 6-segment `crate::` path in `cap_tests.rs:10` (configuration
  `absolute-paths-max-segments = 5`); and the type inference in the test array
  literals, which I read as compiling.
- Runtime results of the existing tests beyond the failures traced by hand
  (T1-T5). `TokenService::create` in `cap_tests.rs` should succeed with the
  128-byte label and 255-byte database under `TokensConfig::default()` and the
  fixed clock. I traced this but did not run it.
- PRECIS behaviour of `precis-profiles` for the test users (`"A"`,
  `"a\u{301}"`). Both are expected to differ from their normalised form and so
  be rejected, but this depends on the crate's tables.
- Whether the TS client's partial PRECIS (`canonicalUser`) and the server's
  full `UsernameCaseMapped` agree for non-ASCII names. This interop risk is
  accepted by the AUTH2 notes; it is outside AUTH1's code.
- The size estimates in section 3. They are editing-time guesses, not measured.
