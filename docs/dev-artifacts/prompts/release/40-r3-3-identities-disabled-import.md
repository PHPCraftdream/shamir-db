# R3-3 — imported identities are disabled and credential-free (orchestrator decision)

Release-1 offline logical import: users imported from a logical dump are
created DISABLED and credential-free-by-construction in the server's user
directory (`FjallUserDirectory`, crate `shamir-server-security`).

## Orchestrator decision (binding)

The decision, as issued for this task:

1. **Disabled marker, serde-default-omitted.** `PersistedUser` gains an
   explicit `disabled` marker with `#[serde(default)]` plus
   `skip_serializing_if` on `false`: the marker decodes as `false`
   (enabled) for every old record that lacks it, and is omitted from the
   encoded bytes when `false`, so every existing record and every
   reference-catalogue byte stays byte-identical.
2. **CSPRNG credential fill.** The import creation path
   (`FjallUserDirectory::insert_disabled_import`) fills
   `salt`/`stored_key`/`server_key` from the OS CSPRNG (16/32/32 random
   bytes; the crate's existing randomness dependency, no new dependency)
   and takes the KDF parameters from the directory's default KDF params.
   Nothing constant, zeroed, or guessable is stored — no password can ever
   match.
3. **Uniform auth refusal.** Every authentication path refuses a disabled
   user with the SAME generic failure as an unknown user or wrong
   password — no distinguishable error message, no timing shortcut beyond
   what the existing unknown-user path already performs.
4. **Ownership visible; no enable op.** Ownership/ACL/principal-resolution
   lookups must NOT refuse disabled users (the import needs them for
   ownership mapping), and there is no production "enable" operation: an
   imported user stays disabled until an operator provisions credentials
   in a later shell command. Ownership can be reassigned with the existing
   `chown`/`chgrp` operations.
5. **Tests.** CI-only tests in `shamir-server-security` cover: a disabled
   imported user fails authentication exactly like an unknown user (equal
   error, not just failure); a record without the marker decodes as
   enabled and round-trips to byte-identical bytes; credential material
   differs between two imports of the same name; ACL/owner-style lookup
   still resolves the disabled user.

## Implementation notes

- Marker: `crates/server/shamir-server-security/src/security/user_directory/record.rs`
  (`PersistedUser::disabled`).
- Creation path: `insert_disabled_import` next to `insert_with_scope` in
  `crates/server/shamir-server-security/src/security/user_directory/directory.rs`.
- Auth refusal lives at the single SCRAM-snapshot surface
  (`UserDirectory::lookup_by_name` returning `None`), which login BEGIN
  (fake-verifier challenge), login FINISH proof verification, and the
  replication handshake all consume — so the failure is byte-for-byte the
  unknown-user failure on every path.
- Open lookup surfaces (unchanged): `user_id()`, `state_by_user_id`,
  `resolve_by_principal64`, `list_all`, `lookup_roles`.
