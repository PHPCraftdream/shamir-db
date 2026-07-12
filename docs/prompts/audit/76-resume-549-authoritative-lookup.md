בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: Ticket v2 — resume re-verifies against the directory (task #558)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context

`docs/design/identity-privilege-unification-548-549-decision.md` §5
(already signed off) is the source of truth for this task's design —
read it first. This brief is step #73 of that design's phased plan
(§7), renumbered to 76 since briefs 70-75 were claimed by #552-557.

**The bug this closes** (confirmed live and pre-existing during #557's
adversarial review — NOT introduced by #557, but #557 made it
concretely provable): `crates/shamir-connect/src/server/resume.rs`'s
`process_resume` currently rebuilds a resumed session's
`SessionPermissions` via `SessionPermissions::from_roles(plain.roles)`
— `plain.roles` is a snapshot taken at the ORIGINAL full-SCRAM
handshake, baked into the encrypted ticket, and never re-checked
against the directory on resume. Two ways this goes wrong today:
1. An admin's superuser status is revoked (via #557's new
   `SetSuperuser`) — the LIVE in-memory session dies on its next
   request via the existing `tickets_invalid_before_ns` epoch bump, but
   if that admin is NOT currently connected and instead resumes later
   with an old ticket minted BEFORE the revoke, the ticket's stale
   `roles` snapshot still says `"superuser"` (from before #556/#557
   even existed, or from before the revoke) — resume grants a fresh
   session with admin powers the account no longer has.
2. Reversed: since task #557 reserves the literal `"superuser"` string
   at the directory write boundary, a ticket ever legitimately
   containing that string can now ONLY have been minted before the
   #556/#557 upgrade — meaning EVERY resumed session for a genuine
   current superuser silently resolves to `is_superuser == false`,
   because the directory no longer round-trips that string through
   `lookup_roles` at the handshake path the ticket's original snapshot
   came from queried differently, and there is no re-verification step
   at all today.

Both failure modes are closed by the same fix: stop trusting the
ticket's `roles` snapshot for authorization. Fetch the CURRENT state
from the directory by `user_id` on every resume.

## 1. `TicketPlain` v2 — drop `roles`, bump the version

`crates/shamir-connect/src/server/ticket.rs`:
- Remove the `pub roles: Vec<String>` field from `TicketPlain` (and its
  `Debug` impl's redacted line, which currently masks it — just delete
  that line too).
- Bump the version literal(s): `decrypt_ticket_with_ciphers`'s hardcoded
  `if wire.version != 1 { ... }` becomes `if wire.version != 2`. This is
  the ENTIRE mechanism that rejects old tickets post-cutover — no
  separate migration path, no dual-version acceptance window (per the
  design doc's §6 "clean cutover" posture already established for this
  whole campaign). A v1 ticket presented after this change fails this
  check and resume returns `Error::AuthFailed`, forcing full SCRAM
  re-auth — this is the correct, intended behavior, not a bug to work
  around.
- `crates/shamir-connect/src/server/resume.rs`'s `process_resume` step 2
  has its OWN separate hardcoded check: `if wire.version != 1 { return
  Err(Error::AuthFailed); }` (around line 236) — bump this to `!= 2` as
  well. Two independent checks exist today (defense in depth); keep
  both in sync.
- `issue_initial_ticket` (`resume.rs`) and the refresh-ticket path inside
  `process_resume` (constructs a `TicketPlain` literal with
  `version: 1`) both need `version: 2` and must NOT set a `roles` field
  (it no longer exists on the struct — the compiler will catch every
  construction site).
- `issue_initial_ticket`'s signature currently takes a `roles:
  Vec<String>` parameter purely to populate the now-removed field —
  remove that parameter entirely. Update its one call site in
  `crates/shamir-server/src/connection/handshake.rs` (which currently
  passes `roles_for_ticket`, sourced from #557's `state_by_user_id`
  call) — that variable becomes unused for ticket issuance; check
  whether it's ALSO still needed for anything else in that function
  before deleting it (it may not be, if the session's `SessionPermissions`
  is the only other consumer and that's already wired from #557).

## 2. `UserStateLookup` — richer return type

`crates/shamir-connect/src/server/resume.rs`'s trait today:
```rust
pub trait UserStateLookup: Send + Sync {
    fn lookup(&self, user_id: &[u8; 16]) -> Option<u64>;
}
```
Replace with a richer snapshot type (defined in THIS crate —
`shamir-connect` cannot depend on `shamir-server`'s
`FjallUserDirectory`/`UserDirectoryState`, so this is a parallel,
independent struct that the `shamir-server` adapter maps INTO):

```rust
/// Authoritative directory snapshot returned by a successful resume
/// lookup — the CURRENT state, re-fetched on every resume, not trusted
/// from the ticket's stale snapshot (task #558 — see this trait's own
/// doc comment history / the design doc §5 for why).
#[derive(Debug, Clone)]
pub struct ResumeUserState {
    /// Current username (guards against stale-name resurrection via an
    /// old ticket if the account is later renamed — resume must use
    /// THIS, not the ticket's `username_nfc`).
    pub username: String,
    /// Current role set.
    pub roles: Vec<String>,
    /// Current superuser flag.
    pub superuser: bool,
    /// `tickets_invalid_before_ns` — unchanged mechanism, still consulted
    /// for the existing spec §5.4 step 9 STRICT `>` epoch check.
    pub tickets_invalid_before_ns: u64,
}

pub trait UserStateLookup: Send + Sync {
    /// Returns the account's current authoritative state, or `None` if
    /// the `user_id` is unknown/removed (resume must reject — spec §5.4
    /// step 8).
    fn lookup(&self, user_id: &[u8; 16]) -> Option<ResumeUserState>;
}
```

Update the two existing implementors in `resume.rs`:
- The blanket closure impl (`impl<F> UserStateLookup for F where F:
  Fn(&[u8;16]) -> Option<u64>`) — change the bound to
  `Fn(&[u8;16]) -> Option<ResumeUserState>`.
- `InMemoryUserStateMap` (`type ... = Arc<DashMap<[u8;16], u64, FxBuild>>`)
  — change the map's value type from `u64` to `ResumeUserState`. This is
  a dev/test-only convenience type (confirmed by grep: no production
  call site, only `crates/shamir-connect/tests/integration_resume.rs`).

## 3. `crates/shamir-server/src/connection/user_state_lookup.rs` — adapter

`RedbUserStateLookup::lookup` currently does:
```rust
self.0.state_by_user_id(user_id).map(|s| s.tickets_invalid_before_ns)
```
Change to map the FULL state across the crate boundary:
```rust
self.0.state_by_user_id(user_id).map(|s| shamir_connect::server::resume::ResumeUserState {
    username: s.username,
    roles: s.roles,
    superuser: s.superuser,
    tickets_invalid_before_ns: s.tickets_invalid_before_ns,
})
```
(`FjallUserDirectory::state_by_user_id` already returns exactly these
four fields via its own `UserDirectoryState` — task #556 built this
specifically for this task to consume; read `user_directory.rs` to
confirm the exact field names before writing the mapping.)

## 4. `process_resume` — the core fix

`crates/shamir-connect/src/server/resume.rs`, steps 8 + 12/13:

- **Step 8** (currently `let invalid_before = user_lookup.lookup(&user_id).ok_or(Error::AuthFailed)?;`) —
  becomes `let state = user_lookup.lookup(&user_id).ok_or(Error::AuthFailed)?;`.
- **Step 9** (the epoch check) — reads `state.tickets_invalid_before_ns`
  instead of the old `invalid_before` local. **This mechanism is
  UNCHANGED and RETAINED** — it still kills live in-memory sessions on
  their next request; this task is complementary (it additionally
  re-verifies at the MOMENT of resume/reconnection), not a replacement.
- **Steps 12/13** (session + optional refresh-ticket construction) —
  currently build `Session::new(user_id, plain.username_nfc, ...,
  SessionPermissions::from_roles(plain.roles), ...)`. Change to use
  `state.username.clone()` (or moved, if the refresh-ticket branch no
  longer needs `plain.username_nfc` for anything since it no longer
  populates a ticket `username_nfc`... **check**: does `TicketPlain`
  still carry `username_nfc`? Yes — that field is UNCHANGED, only
  `roles` is removed. The refreshed ticket still carries
  `plain.username_nfc` forward AS THE TICKET'S OWN FIELD (that's fine,
  it's just an opaque snapshot inside the ticket for the CLIENT's
  convenience/display, not used for authorization) — but the SESSION
  object built for the CURRENT resume must use `state.username`, not
  `plain.username_nfc`, per the design's "guards against stale-name
  resurrection via ticket on a future rename" requirement. Do not
  conflate these two: the ticket's own `username_nfc` field can keep
  round-tripping unchanged (it's not a security-relevant read path), but
  the live `Session` this resume creates must be built from `state`,
  not `plain`, for `username` AND `permissions`.
  `SessionPermissions::new(state.superuser, state.roles.clone())`
  (the #557 constructor) replaces `SessionPermissions::from_roles(plain.roles)`
  in BOTH the refresh-ticket branch and the no-refresh branch.

Re-read both branches in full before editing — the existing code has a
carefully-commented zero-clone optimization strategy ("Optim #6") that
moves `plain.username_nfc`/`plain.roles` between locals to avoid double
clones; since `roles` is gone from `plain` entirely and the session's
username source is changing to `state.username`, that optimization's
shape changes — don't just patch around it blindly, re ryan re-derive
what values are actually still moved vs cloned once `roles` and the
username source both change, and keep whatever of the original
clone-minimization intent still applies (e.g. `state.roles` is
consumed once for the session; there's no ticket copy of roles to
avoid a SECOND clone for anymore, so part of that optimization's
rationale is now moot — simplify rather than force the old shape to fit).

## 5. Sweep: `crates/shamir-connect/tests/integration_resume.rs`

Every `let users = new_user_state_map(); users.insert(user_id, 0);` (or
`users.insert(user_id, now);`) call site (~10+, found via grep) needs
its second argument changed from a bare `u64` to a `ResumeUserState`.
Add a small local test helper near the top of this file if one doesn't
already fit the existing style, e.g.:
```rust
fn state(tib: u64) -> ResumeUserState {
    ResumeUserState { username: "u".into(), roles: vec![], superuser: false, tickets_invalid_before_ns: tib }
}
```
and change `users.insert(user_id, 0)` → `users.insert(user_id, state(0))`
etc. across the mechanical sites.

**Two tests need a REAL rewrite, not a mechanical substitution** —
`resumed_admin_session_retains_roles_per_diagram_02` and
`refresh_ticket_carries_roles_forward_per_diagram_02` (both currently
assert that a session resumed via a ticket containing
`roles: vec!["superuser", ...]` ends up with `is_superuser == true`,
i.e. they currently encode the EXACT bug this task closes — "resume
trusts the ticket's roles snapshot"). Rewrite both to prove the NEW
behavior instead:
- Seed the `users` lookup map with `ResumeUserState { superuser: true,
  roles: vec!["read_write".into()], .. }` for the target `user_id`.
- Issue the initial ticket via `issue_initial_ticket` WITHOUT a `roles`
  argument (the parameter is gone).
- Resume, then assert the resulting session's
  `permissions.is_superuser == true` and `permissions.roles` match the
  LOOKUP map's state — proving the session was built from the
  directory, not from anything the ticket carried (the ticket, by
  construction, has no way to carry superuser/roles data anymore, so
  this ALSO organically proves it — but make the test's own comment
  say so explicitly, replacing the stale "diagram 02 step 12: sessions
  MUST be constructed with permissions = ticket_plain.roles" framing,
  which is now wrong).
- Rename both tests to reflect the new behavior being proven (e.g.
  `resumed_session_permissions_come_from_directory_lookup_not_ticket`)
  — keep them, don't delete them; the property they guard (admin status
  survives/reflects correctly across resume) is still real and
  important, just proven the opposite way now.

## 6. Red tests required first (TDD)

1. **v1 ticket rejected** — construct (or reuse an existing pre-#558
   fixture if one exists in the test file for an older wire shape) a
   `TicketWire` with `version: 1` (either by hand-crafting the envelope
   bytes, or by keeping a frozen pre-migration `encrypt_ticket`-equivalent
   helper for the OLD `TicketPlain` shape purely for this test — whichever
   is less invasive given the actual code after your changes), pass it to
   `process_resume`, confirm `Err(Error::AuthFailed)`.
2. **Role-revoked-then-resume gets a non-admin session even when no
   epoch bump ran** — this is the DIRECT regression test for the bug:
   seed `users` with `ResumeUserState { superuser: false, .. }` (i.e.
   simulate an account that WAS a superuser when the ticket was minted
   but has since been revoked — deliberately do NOT bump
   `tickets_invalid_before_ns` past `plain.original_auth_at_ns`, so the
   existing epoch check alone would NOT catch this), issue a ticket for
   that user_id, resume, and assert the resulting session has
   `is_superuser == false`. Under the OLD code (trusting
   `plain.roles`/a ticket that could carry the string) this would have
   incorrectly stayed `true` if the ticket had been minted while still a
   superuser — this test proves the NEW lookup-based path closes that
   gap independent of the epoch mechanism.
3. The 2 rewritten tests from §5 above (they double as red tests for the
   "resume must read the directory, not the ticket" property in the
   grant direction).

## Out of scope — do not touch

- `PrincipalResolver`/`UserAdminPort`, shamir-db Store B retirement —
  task #559.
- TS/Rust client builder changes — task #560 (no wire-visible field
  changes for CLIENTS here beyond the ticket's opaque internal shape,
  which clients never parse — tickets are opaque blobs to callers).
- `crates/shamir-server/src/bootstrap.rs`, `db_handler/admin.rs`'s
  `SetSuperuser` handler — untouched, already correct from #557.
- Any change to `check_anti_downgrade`, the channel-binding posture, or
  the counter/replay mechanism (`ConsumedCounterStore`) — all unrelated
  to this task, confirmed by reading `process_resume`'s full body; do
  not touch steps 10/11's logic, only step 8/9's data source and
  step 12/13's session-construction source.

## Definition of done

- `cargo check --workspace --all-targets` clean (this touches a
  cross-crate trait signature — confirm every implementor and call site
  in the workspace compiles, not just the two crates in the test command
  below).
- `cargo fmt -p shamir-connect -p shamir-server -- --check` clean on
  touched files.
- `cargo clippy -p shamir-connect -p shamir-server --all-targets -- -D warnings` clean, modulo the already-tracked pre-existing `read_planner.rs` issue (task #562) if it surfaces via a shared dependency — confirm via `git diff --stat` you haven't touched that file.
- `./scripts/test.sh -p shamir-connect -p shamir-server --full` green, including the new/rewritten tests.
- Repo-wide grep confirms no remaining reference to `TicketPlain.roles` or a `UserStateLookup::lookup` returning bare `u64`.

When done, produce a final summary (not a bare tool call) listing: every
file changed with a one-line description, the full text of every
new/rewritten test, the gate command outputs, and any place this
brief's assumptions didn't match the actual code (with how you resolved
it) — in particular confirm explicitly whether `plain.username_nfc` is
still round-tripped inside the ticket's own opaque payload (fine) versus
whether the live `Session`'s username now correctly comes from
`state.username` (required), since these are easy to conflate.
