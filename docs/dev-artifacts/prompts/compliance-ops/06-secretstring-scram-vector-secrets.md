# Compliance & Ops 5f — SecretString for CreateScramUser.password + VectorBackendRef.api_key_secret

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## CRITICAL PROCESS RULE

Run ALL your test/build commands in the FOREGROUND — plain blocking Bash
calls, no backgrounding your own commands. Do not background a long-running
command and then end your turn while it is still running server-side; wait
for it to finish before reporting or continuing.

## Why this task gets extra care

The user flagged this specific item as wanting careful treatment — it is a
real, security-sensitive change (secret-bearing fields), not a docs fix or a
small function addition like the rest of Этап 4/5. The investigation below
was done thoroughly (including a full crate-dependency-graph check) before
writing this brief; **follow it precisely rather than re-deriving the
architecture from scratch** — the design decision (where `SecretString` ends
up living) is deliberate and explained.

## Context

Sixth/final item of "Этап 5 — Compliance & Ops"
(`docs/dev-artifacts/research/2026-07-17-release-audit/00-WORK-PLAN.md`),
sourced from report 03. Two currently-plaintext secret fields need the same
treatment `CreateUserOp.password` already got (see precedent below):

1. `DbRequest::CreateScramUser.password: String`
   (`crates/shamir-query-types/src/wire/db_message.rs:62`) — the wire op
   that creates a SCRAM-authenticatable login user. `DbRequest` derives
   `Debug` (`db_message.rs:27`), so `{:?}`-formatting any `DbRequest` value
   that happens to be a `CreateScramUser` currently prints the raw cleartext
   password. The doc comment on the field even says this is intentional-ish
   ("the server wraps the received `String` in `Zeroizing<Vec<u8>>`") but
   that wrapping happens **after** dispatch — the bare `String` sits
   Debug-printable and non-zeroizing in the deserialized request, the
   `handler.rs` match arm, and every intermediate hop.

2. `VectorBackendRef::External.api_key_secret: String`
   (`crates/shamir-index/src/kind.rs:197`) — same class of issue: the enum
   derives `Debug` (`kind.rs:188`), so an API key for an external vector
   backend is Debug-printable. **Confirmed via grep: this field has exactly
   ONE reference in the entire `crates/` tree today — its own declaration.**
   `VectorBackendRef::External` is not yet constructed or read anywhere
   (the external-vector-backend adapter isn't implemented yet). This means
   the fix here has **zero production call sites to migrate** — it's a
   pure type change plus a Debug-redaction regression test.

### The existing precedent — read this first

`crates/shamir-query-types/src/auth/secret.rs` already defines exactly the
right tool: `SecretString` — wraps a `String`, manual `Debug` prints
`"SecretString(***)"`, `Serialize`/`Deserialize` pass through transparently
(wire shape unchanged — still a plain JSON/msgpack string), `Drop` zeroizes
the heap buffer when compiled with the `crypto` feature (`zeroize`,
optional). It is already used for `CreateUserOp.password` and
`User.password_hash` (`crates/shamir-query-types/src/auth/types.rs`).

**The problem: `SecretString` cannot be reused as-is for
`VectorBackendRef.api_key_secret`.** `shamir-index` (where `VectorBackendRef`
lives) does **not** depend on `shamir-query-types` (confirmed via
`grep -n "shamir-query-types" crates/shamir-index/Cargo.toml` — no match).
Adding that dependency would be a real layering violation: `shamir-index` is
a low-level storage/index engine crate; `shamir-query-types` is the
wire-protocol DTO crate consumed by client/server/engine. Pulling wire types
into the index crate just to borrow one struct is backwards.

**What both crates already share:** `shamir-types` (confirmed —
`shamir-index/Cargo.toml:9` and `shamir-query-types/Cargo.toml:10` both
depend on it: `shamir-types = { path = "../shamir-types" }`). This is the
correct shared home.

**A second confirming signal:** `crates/shamir-query-builder/Cargo.toml:19`
already has `shamir-types = { path = "../shamir-types", default-features =
false }` — but `shamir-types/Cargo.toml` currently has **no `[features]`
section at all**, so that `default-features = false` is a no-op today. This
looks like the workspace already anticipated `shamir-types` growing an
optional-features split (the same way `shamir-query-types` gates `crypto`
for its own WASM-guest-friendly consumers) — this task is the natural
occasion to add it.

## The task

### 1. Move `SecretString` to `shamir-types`

- Create `crates/shamir-types/src/secret.rs` with the **exact same
  contents** as the current `crates/shamir-query-types/src/auth/secret.rs`
  (struct, `Debug`, `Serialize`/`Deserialize`, `#[cfg(feature = "crypto")]
  Drop`, `From<String>`/`From<&str>`). Wire it into
  `crates/shamir-types/src/lib.rs` (`pub mod secret;`).
- Add to `crates/shamir-types/Cargo.toml`:
  ```toml
  zeroize = { version = "1", features = ["derive"], optional = true }

  [features]
  default = ["crypto"]
  crypto  = ["dep:zeroize"]
  ```
  (Mirror `shamir-query-types/Cargo.toml`'s existing comment style
  explaining why `crypto` is optional — WASM-guest builds via
  `shamir-query-builder`'s `default-features = false` skip it.)
- Move the existing test file
  `crates/shamir-query-types/src/auth/tests/secret_tests.rs` to
  `crates/shamir-types/src/tests/secret_tests.rs` (check
  `shamir-types/src/tests/mod.rs` — if it doesn't exist yet, check how
  `shamir-types` currently organizes its `#[cfg(test)] mod tests;` wiring
  and follow the SAME pattern used elsewhere in that crate; this project's
  test-org convention is one `tests/` dir per module, `tests/mod.rs` as a
  manifest-only re-export file — see `CLAUDE.md` §"Test organisation").
  Update the `use` path inside the moved test file
  (`crate::secret::SecretString` instead of `crate::auth::secret::SecretString`).

### 2. Re-export from `shamir-query-types` for zero-blast-radius compatibility

- Delete `crates/shamir-query-types/src/auth/secret.rs`.
- In `crates/shamir-query-types/src/auth/mod.rs`, replace
  `pub mod secret;` + `pub use secret::SecretString;` with
  `pub use shamir_types::secret::SecretString;` — every existing
  `use shamir_query_types::auth::SecretString` (or `crate::auth::SecretString`
  inside this crate) keeps compiling unchanged. Confirmed call sites that
  must NOT need touching: `crates/shamir-query-types/src/auth/types.rs`
  (`User.password_hash`, `CreateUserOp.password`),
  `crates/shamir-query-builder/src/ddl/auth.rs:1,52`.
- Update `crates/shamir-query-types/Cargo.toml`: remove the now-redundant
  direct `zeroize` optional dependency and its `dep:zeroize` from the
  `crypto` feature; add `shamir-types/crypto` instead:
  ```toml
  crypto = ["dep:hmac", "dep:sha2", "shamir-types/crypto"]
  ```
  Keep `hmac`/`sha2` exactly as they are (unrelated to this task).

### 3. `DbRequest::CreateScramUser.password: String` → `SecretString`

- `crates/shamir-query-types/src/wire/db_message.rs:62` — change the field
  type. Update its doc comment: it currently explains the server-side
  `Zeroizing<Vec<u8>>` wrap; add a line noting the wire-level `String` is
  now itself wrapped in `SecretString` so it can no longer leak through
  `Debug`/logging before that server-side wrap happens. Serde stays
  transparent — the wire shape (a plain JSON/msgpack string) is UNCHANGED,
  confirm this explicitly in your summary (existing TS/other clients need
  no changes).
- `crates/shamir-server/src/db_handler/handler.rs:299-306` — the
  `DbRequest::CreateScramUser { name, password, roles, hmac } =>` arm
  destructures `password` and passes it straight to `create_scram_user`.
  Decide: either change `create_scram_user`'s signature to accept
  `SecretString` and reveal it internally, or reveal at the call site
  (`password.reveal().to_owned()`) — prefer threading `SecretString`
  through as far as is natural and only converting to a plain `String`/
  `Zeroizing<Vec<u8>>` right at `derive_scram_record`'s existing
  `Zeroizing::new(password.into_bytes())` call
  (`crates/shamir-server/src/db_handler/admin.rs:54`), which already does
  the right thing once it receives the cleartext.
- `crates/shamir-server/src/db_handler/admin.rs`:
  - `create_scram_user` (line ~98-105): `password: String` param → decide
    whether to accept `SecretString` here directly (recommended — keeps the
    "opaque until the last possible moment" property) or keep `String` and
    convert at the `handler.rs` call site. Your call; document which in
    your summary.
  - `derive_scram_record` (line ~47-50) is ALSO called from
    `crates/shamir-server/src/ports.rs:98` (`UserAdminPort::create_user`
    impl) with a plain `&str`/`String` today — that call site is fine as-is
    and does NOT need to change (it already receives `SecretString` further
    up its own call chain per `CreateUserOp.password`, and reveals it
    before calling `derive_scram_record` — check `ports.rs` around line
    90-98 to confirm, don't break it).
- `crates/shamir-client/src/client.rs:817-847`
  (`ShamirClient::create_scram_user`) — currently takes
  `password: Zeroizing<String>`, builds
  `DbRequest::CreateScramUser { password: password.as_str().to_owned(), ...
  }`, then after the roundtrip does a manual
  `if let DbRequest::CreateScramUser { password, .. } = &mut req { password.zeroize(); }`
  to wipe the transient copy immediately (not waiting for `req`'s natural
  end-of-function drop). With the field now `SecretString` (which already
  zeroizes on `Drop` under the `crypto` feature — check `shamir-client`'s
  own Cargo.toml pulls in `shamir-query-types` with `crypto` enabled,
  likely via default features, confirm this), the cleanest equivalent is to
  replace that manual `if let ... zeroize()` block with `drop(req);` placed
  at the same point (right after the roundtrip result is captured, before
  matching it) — same "wipe immediately, don't wait for function end"
  intent, less error-prone than hand-rolled field-reaching. Confirm this
  compiles and keep the surrounding comment's *intent* (wipe ASAP) even if
  you reword it.
- Test call sites (mechanical — all just need `.into()` instead of a bare
  `String`, `SecretString: From<String> + From<&str>` already exists):
  `crates/shamir-server/tests/db_handler.rs`,
  `crates/shamir-server/tests/hmac_gate.rs`,
  `crates/shamir-server/tests/permission_e2e.rs`,
  `crates/shamir-server/tests/repl_pull_e2e.rs`. Grep for
  `DbRequest::CreateScramUser` in each — most already write
  `password: "...".into()` (already correct against the new type, since
  `"...".into()` resolves to whatever the target field type is and
  `SecretString: From<&str>` exists — verify these compile unchanged). A
  few use `password: String::from_utf8(...).expect("utf8")` — these need
  `.into()` appended: `String::from_utf8(...).expect("utf8").into()`.

### 4. `VectorBackendRef::External.api_key_secret: String` → `SecretString`

- `crates/shamir-index/src/kind.rs:194-199` — change the field type to
  `shamir_types::secret::SecretString` (add the `use` at the top of the
  file). No other call sites exist today (confirmed empty grep) — this is
  a pure type change. `shamir-index/Cargo.toml` already depends on
  `shamir-types` with default features (i.e. `crypto` included), so no
  Cargo.toml change is needed here.

## Tests

1. **`shamir-types::secret` unit tests** — the 3 moved tests
   (`debug_redacts_value`, `serde_roundtrip_preserves_value`,
   `from_str_and_string`) pass unchanged in their new location.
2. **`DbRequest::CreateScramUser` Debug redaction (NEW regression test)** —
   construct a `DbRequest::CreateScramUser { password: "hunter2".into(), ...
   }`, `format!("{:?}", req)`, assert the output does NOT contain
   `"hunter2"` and DOES contain `"SecretString(***)"` or equivalent. Add
   near the existing `shamir-query-types` wire/db_message tests (check
   `crates/shamir-query-types/src/wire/tests/` or wherever
   `db_message.rs`'s existing tests live, mirror that location).
3. **`DbRequest::CreateScramUser` wire round-trip (NEW regression test)** —
   msgpack-serialize a `CreateScramUser` request, deserialize it back,
   confirm the password value survives intact (`reveal() == "hunter2"`) —
   proves the wire SHAPE is unchanged (still a plain string on the wire,
   not a wrapped object), only the Rust-side type is upgraded.
4. **`VectorBackendRef::External` Debug redaction (NEW regression test)** —
   same pattern as #2 for `api_key_secret`. Add near
   `crates/shamir-index/src/tests/kind_tests.rs` (confirmed this file
   exists from the earlier grep).
5. **Server-side end-to-end regression** — run the existing
   `crates/shamir-server/tests/{db_handler,hmac_gate,permission_e2e,
   repl_pull_e2e,change_password_e2e}.rs` suites unmodified in BEHAVIOR
   (only the `.into()` mechanical touch-ups from step 3 above) and confirm
   they still pass — this proves `create_scram_user`'s actual runtime
   behavior (Argon2id derivation, user creation, HMAC gate) is unaffected
   by the type change.

## Out of scope

- Do NOT implement the `VectorBackendRef::External` adapter itself (the
  backend that would actually USE `api_key_secret`) — it doesn't exist yet;
  this task only fixes the type of the field that will eventually hold the
  secret.
- Do NOT change the wire/JSON shape of `CreateScramUser` — `SecretString`'s
  `Serialize`/`Deserialize` are transparent pass-throughs by design; the
  byte-level wire protocol must be identical before and after this change.
- Do NOT touch `derive_scram_record`'s internal Argon2id/`Zeroizing`
  handling beyond adapting its call sites' input type — that logic is
  already correct.
- Do NOT add a `crypto`-style feature split to `shamir-index` — it already
  depends on `shamir-types` with default features, so `SecretString`'s
  zeroize-on-drop is simply available there, no extra feature wiring
  needed on the `shamir-index` side.
- Do NOT touch anything from the already-completed correctness-bug wave,
  concurrency-deadlock sweep, DDL-time-rejection/warn-log fixes, cleanup
  tails A/B/C, Этап 4's funclib top-up, or the already-completed Этап 5
  tasks 5a-5e — this brief is scoped to these two secret fields only.

## Verification (MANDATORY before you report done)

- `./scripts/test.sh @types --full` green (covers `shamir-types` +
  `shamir-collections`, includes the moved `secret_tests.rs`).
- `./scripts/test.sh -p shamir-query-types --full` green (covers the
  re-export + the new `CreateScramUser` Debug/wire-roundtrip tests).
- `./scripts/test.sh -p shamir-index --full` green (covers the new
  `VectorBackendRef` Debug test).
- `./scripts/test.sh -p shamir-server --full` green (covers all the
  mechanical test-call-site touch-ups + the real `create_scram_user`
  end-to-end paths).
- `./scripts/test.sh -p shamir-client --full` green (covers the
  `client.rs::create_scram_user` change).
- `./scripts/test.sh -p shamir-query-builder --full` green (confirms the
  `shamir_query_types::auth::SecretString` re-export + the
  `default-features = false` → `shamir-types` without `crypto` path still
  compiles for the WASM-guest-friendly builder).
- `cargo fmt --all -- --check` clean (or scoped to the touched crates,
  report which).
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- Report literal command output for all of the above.
- Explicitly confirm: (a) the wire byte shape of `CreateScramUser` is
  unchanged (plain string, not a wrapped object) — cite the round-trip
  test; (b) `VectorBackendRef::External` has zero other call sites, so no
  runtime behavior changed, only the type; (c) `shamir-query-builder`
  still compiles with `shamir-types`' `default-features = false` (i.e. the
  `crypto` feature split doesn't break the WASM-guest-friendly path it was
  designed to preserve).
