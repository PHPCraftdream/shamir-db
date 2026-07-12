בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Brief: Root/User/Group get real, kind-specific permission models (task #552)

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Context

`docs/design/root-user-group-dac-posture-550-decision.md` §1 (already
signed off by the project owner) documents this exact design in full —
read it first, it is the source of truth for every claim below. This
brief is the actionable slice of that decision.

Today, `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs:132-134`
hard-codes:
```rust
ResourcePath::Root | ResourcePath::User { .. } | ResourcePath::Group { .. } => {
    Ok(ResourceMeta::open())
}
```
— every principal has full rwx on Root, on every User path, and on
every Group path, unconditionally. `set_resource_meta` (same file,
lines 225-231) explicitly rejects writes to all three
(`"resource path '{}' does not support set_resource_meta in this slice"`).
This task replaces the blanket `open()` with three DIFFERENT models —
one per resource kind — reasoned from what each kind actually needs.

## Scope

All changes are in `crates/shamir-db/src/shamir_db/shamir_db/access_control.rs`
and `crates/shamir-db/src/shamir_db/system_store.rs` unless noted.

### 1. Root — full persisted meta, mirroring the existing `FunctionNamespace` pattern

`FunctionNamespace`'s resource_meta arm (lines 122-131) already does
exactly this shape for a different singleton — copy the pattern, not
invent a new one:

```rust
ResourcePath::Root => match self.system_store.load_setting("root_meta").await {
    Ok(Some(v)) => Ok(ResourceMeta::from_record(&v)),
    Ok(None) => Ok(ResourceMeta { owner: Actor::System, group: None, mode: 0o755 }),
    Err(e) => {
        log::warn!("resource_meta: failed to load root meta: {e}");
        Err(e)
    }
},
```

Add the matching `set_resource_meta` write arm (mirror the
`FunctionNamespace` arm at lines 213-224, using the settings key
`"root_meta"` instead of `"fn_namespace_meta"`).

`0o755` (not the universal-777 default) is deliberate: owner (System)
keeps rwx; group/other keep r-x (traverse+list stay open exactly as
today) but lose w (creating a top-level database narrows from
everyone-writable to owner-only) — this is the only observable
behavior change for Root itself, and it matches the coarse wire-admin
gate's existing intent that database creation is a privileged act.

**Guardrail (required, not optional):** in the `set_resource_meta`
write path for Root, reject (return a validation error, do not let it
persist) a `chmod` that would clear owner-Execute on Root when the
CURRENT owner (before the write) is not `Actor::System`. Rationale:
`Actor::System` bypasses `permits()` unconditionally and can always
recover; a non-System owner who accidentally clears their own traverse
bit on Root would have no way back in. If the owner is `Actor::System`,
no guard is needed (System always bypasses).

Add a test: `chown /` to a non-System owner via `set_resource_meta`
succeeds (this is new behavior — Root previously rejected all writes);
after that, a `chmod` clearing owner-Execute for that same non-System
owner is rejected; the equivalent `chmod` when owner is still `System`
is NOT rejected (System doesn't need the guard).

### 2. User — a FIXED, computed 3-tier rule, never persisted

```rust
ResourcePath::User { name } => Ok(ResourceMeta {
    owner: Actor::User(principal_id(name)), // principal_id, NOT principal64 — task #555 (Actor::Admin/principal64) hasn't landed yet; this call site gets a one-line swap when it does
    group: None,
    mode: 0o750, // owner (self): rwx; group: n/a (group is None); other: nothing
}),
```

Never stored — no `set_resource_meta` arm is added for `User` (leave
it falling through to the existing catch-all `_ => Err(NotFound(...))`
at line 228). This is a deliberate non-goal: persisting per-user meta
here would create a second identity-adjacent store, which would
collide with the (separately approved, not-yet-implemented) directory-
canonical-store decision in `docs/design/identity-privilege-unification-548-549-decision.md`
§3 — do not add persistence for User meta in this task.

Effective behavior via the existing `permits()`/`class_of()` machinery
(no changes needed there — this is pure data, not new logic): System —
full (bypass); the user themselves (`Actor::User(principal_id(name))`
matches `class_of`'s owner check) — Read + Manage on their own `User`
path; everyone else — `class_of` resolves to `Other`, and `Other` has
no bits in `0o750` → denied Read/List/anything. This is a REAL
narrowing the moment any caller enforces on a `User` path — today
`open()` passes everyone; `0o750` passes only self and System.

Add a test: `authorize_access(Actor::User(principal_id("alice")), ResourcePath::user("alice"), Action::Read)` succeeds;
the same call with `Action::Manage` succeeds (self-service umbrella);
`authorize_access(Actor::User(principal_id("bob")), ResourcePath::user("alice"), Action::Read)` is DENIED.

### 3. Group — persisted `owner` field on the existing group record

Groups have a natural owner (their creator) and a natural "group"
class (their own members) — unlike Root/User, this warrants real
storage, not a computed rule.

**`system_store.rs` changes:**
- `save_group` (line 630) gains an `owner: u64` parameter (persist-
  friendly `Actor::to_owner_id()` encoding, matching every other
  `ResourceMeta::inject_into` convention in this codebase) and writes
  it into the record (`m.insert("owner", QueryValue::Int(owner as i64))`).
  Update its THREE existing call sites in this same file
  (`add_group_member`, `remove_group_member` — both do read-modify-
  write via `save_group`, so thread the existing record's owner field
  through unchanged) plus `access_control.rs`'s `rename_group_as`
  (thread the existing owner through unchanged there too — renaming
  must not touch ownership).
- Legacy records lacking the field (created before this task) decode
  via `ResourceMeta::from_record`'s existing fallback → `Actor::System`
  — fail-safe: only superuser manages pre-existing groups until an
  operator explicitly `chown`s them (once #3's write arm below exists).

**`access_control.rs` changes:**
- Add a `resource_meta` read arm for `ResourcePath::Group { name }`:
  resolve `name` to a `group_id` (reuse `resolve_group_id` with
  `GroupRef::Name`), `load_group(group_id)`, and build
  `ResourceMeta { owner: ResourceMeta::owner_field(&rec).unwrap_or(Actor::System), group: Some(group_id), mode: 0o750 }`.
  `group: Some(group_id)` makes a group's own members a real
  permission class (roster-read for members), matching the design
  doc's "members read (roster)" intent. Not-found → fall back to
  `ResourceMeta::open()` (mirrors the `FunctionFolder` "never created"
  convention at lines 107-112 — a nonexistent group is not an error
  case for meta resolution, just uses the default).
- Add a `set_resource_meta` write arm for `Group` (mirror the
  `FunctionNamespace` pattern): load the group record by resolving
  `name`, inject the new owner/mode via a NEW `system_store` method
  `set_group_owner(group_id, owner: u64)` (simplest: reload existing
  `name`+`members`, call the updated `save_group` with the new owner).
  Only `owner` is settable this way (per the design doc, group `mode`
  stays fixed/computed at `0o750` — no demonstrated need for per-group
  chmod yet; do not add a mode-write path).
- `create_group_as` (line 269): after resolving the group id, persist
  the ACTING actor as owner — `save_group(group_id, name, &[], actor.to_owner_id())`
  instead of the current `&[]` 3-arg call. Creation itself stays gated
  on `Manage(Root)` only (the design doc is explicit: creation writes
  into the Root container, so Root's gate is the right one — do not
  add a second check here).
- `drop_group_as`, `rename_group_as`, `add_group_member_as`,
  `remove_group_member_as` (lines 321-430): change the gate from
  the current unconditional
  `self.authorize_access(actor, &ResourcePath::Root, Action::Manage).await`
  to: succeed if EITHER that Root-Manage check succeeds OR
  `self.authorize_access(actor, &ResourcePath::group(name), Action::Manage).await`
  succeeds (resolve `name` from the existing `group_id`/`GroupRef`
  argument first — `system_store.load_group(group_id)` to get the
  `name` field, or reuse `resolve_group_id`'s reverse lookup pattern).
  Do not change the underlying `Manage`-semantics — `permits()` already
  resolves `Manage` as owner-only (line 656-658 in `shamir-types/src/access.rs`),
  so once the Group `resource_meta` arm above reports the real owner,
  `authorize_access(actor, Group{name}, Manage)` naturally succeeds
  for that group's own creator with zero changes to `permits`/`class_of`.

Add tests: a group's creator can `rename_group_as`/`add_group_member_as`/
`remove_group_member_as`/`drop_group_as` their OWN group WITHOUT
`Manage(Root)`; a different non-superuser (not the group's owner, no
Root-Manage) is DENIED the same four ops on someone else's group;
`create_group_as` still requires `Manage(Root)` regardless of who's
asking (unchanged).

## Out of scope (do not touch)

- `principal_id` → `principal64` migration (task #555, separate,
  unblocked, independent — do NOT wait for it or reference it beyond
  the one-line-swap comment above).
- Any change to `permits()`/`class_of()`/`Action`/`Mode` in
  `crates/shamir-types/src/access.rs` — this task is pure call-site/
  storage wiring; the POSIX evaluation logic already does the right
  thing once real meta is supplied.
- `PrincipalResolver`/`UserAdminPort` (task #559) — unrelated, separate
  decision track.

## Test scope

```
./scripts/test.sh -p shamir-db --full
./scripts/test.sh -p shamir-types
```
Add new tests under `crates/shamir-db/src/shamir_db/tests/` following
this codebase's existing layout convention (one file per topic,
`tests/mod.rs` re-exports only — see `group_tests.rs` and
`admin_access_validation_tests.rs` for the existing pattern to extend,
or add a new `root_user_group_meta_tests.rs` if the existing files
don't fit topically).

## Definition of done

- `cargo fmt -p shamir-db -p shamir-types -- --check` clean.
- `cargo clippy -p shamir-db -p shamir-types --all-targets -- -D warnings` clean.
- `./scripts/test.sh -p shamir-db -p shamir-types --full` green, including
  the new tests above.
- No change to any OTHER resource kind's `resource_meta`/`set_resource_meta`
  arm.
