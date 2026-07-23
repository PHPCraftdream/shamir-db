# Brief: CR-A3 — server-side page_size validation (#762)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

CRITICAL PROCESS RULE: run ALL test/build commands in the FOREGROUND. Do
not background `cargo`/`./scripts/test.sh` invocations.

## Problem — INFINITE LOOP, verified against the current tree 2026-07-23

`crates/shamir-server/src/db_handler/cursor_handlers.rs` never validates
`page_size` (a plain `u32` parameter on both `create_cursor` ~line 230 and
`fetch_next` ~line 380). The `has_more` computation at both ~line 303 and
~line 479 is `page.records.len() as u64 >= page_size as u64`. With
`page_size == 0`: `0 >= 0 → true`, forever — `has_more` never becomes
`false`, `offset`/the keyset bookmark never advances (the page is always
empty), and the client loops forever: the Rust SDK's `CursorStream` keeps
issuing `FetchNext` (nothing ever signals end-of-stream), the TS SDK's
`CursorIterator.next()` recurses indefinitely on an always-empty buffer.

There is also no UPPER bound — a client can request an arbitrarily large
`page_size`, forcing the server to materialize/serialize a huge page. (A
byte-size clamp is CR-A5's job, already queued as #763 right after this
one; this task adds the independent, cheaper row-COUNT hygiene bound —
land both, they're complementary, not redundant.)

## Fix

### 1. New config field: `max_cursor_page_size`

Mirror the existing two-field pattern exactly:

- `crates/shamir-server/src/config.rs`'s `CursorLimitsConfig` (~lines
  292-302): add `pub max_cursor_page_size: u32` with a
  `#[serde(default = "default_max_cursor_page_size")]` and a
  `default_max_cursor_page_size() -> u32` function returning `10_000`.
  Update `impl Default for CursorLimitsConfig` (~line 304+) accordingly.
- `crates/shamir-server/src/db_handler/config.rs`'s `CursorLimitsCap`
  (~lines 77-80): add `pub max_cursor_page_size: u32`. Update `UNLIMITED`
  (`u32::MAX`) and the operator-facing `DEFAULT` const (`10_000`, matching
  the config default above).
- Thread it through `server_launcher.rs`'s existing
  `CursorLimitsCap { max_cursors_per_session, idle_timeout_secs }`
  construction (search for that literal) — add the third field.
- Add a `Config::validate()` check (mirror the existing
  `max_inflight_response_bytes >= max_result_size_bytes` cross-check
  pattern, `config.rs` ~line 622-632): reject `max_cursor_page_size == 0`
  at config-load time (a zero cap would make every cursor unusable).

### 2. Reject `page_size == 0` in both `create_cursor` and `fetch_next`

Add a validation check at the top of each function (right after the
existing `query_version`/temporal checks in `create_cursor`; right after
`cursor_registry.get_owned` in `fetch_next` — decide whether to validate
BEFORE or after the registry lookup; validating page_size doesn't need the
cursor, so either order works, but validating first avoids a wasted
registry hit for a malformed request — prefer that). Use a distinct,
specific wire error code (this codebase's cursor errors are all specific —
`cursor_not_found`/`cursor_expired`/`cursor_limit_exceeded`/
`cursor_temporal_not_supported` — not a generic `"validation"` string) —
e.g. `BatchError::query_coded("", "invalid_page_size", "page_size must be between 1 and <max>")`
via the existing `error_response()` helper already in this file. Check
whether adding a real `BatchError` variant (rather than the ad hoc
`query_coded` string-code path CR-A1 used for `access_denied`) fits this
codebase's convention better for a NEW, cursor-specific validation
failure — look at how `CursorLimitExceeded`/`CursorTemporalNotSupported`
were added in FG-5a/b for the precedent to follow.

### 3. Clamp (or reject) `page_size` above `max_cursor_page_size`

Pick ONE semantic and document it clearly in the wire error/`CURSORS.md`:
- **Reject** (simpler, more honest, recommended): `page_size >
  self.cursor_limits.max_cursor_page_size` → same/similar error as the
  zero case, distinguishable if useful (e.g. `invalid_page_size` covers
  both "zero" and "too large" with a message stating the valid range, or
  split into two codes if that's cleaner — your call, document it).
- Do NOT silently clamp without telling the client — a client that thinks
  it got `page_size=100000` rows per page but silently got 10,000 would
  misinterpret `has_more` semantics.

## Tests (TDD — write failing tests first)

In `crates/shamir-server/src/db_handler/tests/cursor_handler_tests.rs`:

- `create_cursor` with `page_size = 0` → a clean error response (not a
  `CursorPage` that could loop forever), and no cursor registered.
- `fetch_next` with `page_size = 0` against an already-open, still-open
  (`has_more == true`) cursor → a clean error; the cursor itself must
  remain usable afterward (a bad `page_size` on one `FetchNext` call
  should not corrupt or close the cursor — verify a SUBSEQUENT `fetch_next`
  with a valid `page_size` still works normally).
- `create_cursor`/`fetch_next` with `page_size` above the configured
  `max_cursor_page_size` → the chosen semantic (reject, per the
  recommendation above), asserted explicitly.
- Config test: `CursorLimitsConfig::default().max_cursor_page_size == 10_000`;
  `Config::validate()` rejects `max_cursor_page_size == 0`.

## Gate

```
cargo fmt -p shamir-server -- --check
cargo clippy -p shamir-server --all-targets -- -D warnings
./scripts/test.sh -p shamir-server --full
```

All must pass before returning. Stay inside `shamir-server`
(`cursor_handlers.rs`, `db_handler/config.rs`, `config.rs`,
`server_launcher.rs`, tests). This touches the SAME `create_cursor`/
`fetch_next` functions CR-A1 and CR-A2 already modified (committed at
`b860765c` and `9f7b9d55`) — re-read the current file state before
editing rather than assuming stale line numbers.
