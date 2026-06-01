בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Shomer — pure-refactoring track ✅ DONE

> **Status: complete.** R1 `0be711f`, R2 `1b49194`. The behavior-preserving
> substrate is in place: `Actor` + `ResourcePath` + a single transparent
> `authorize` door, with the actor flowing from the facade entries down
> through `FilterContext`/`FnCtx`/`TxContext` and every resource touch
> routed through the door. The door still returns `Ok` only; the actor is
> still `System` everywhere → behavior byte-for-byte unchanged. Enforcement
> (P4) is now one change inside `authorize`; it is NOT refactoring.

This is the **behavior-preserving** substrate refactor that lands BEFORE
any access policy. The full model + the rights stages (P0–P6) live in
[`ACCESS_FABRIC.md`](ACCESS_FABRIC.md); this file isolates the pure
refactoring that changes **no behavior**, so the rights system later just
"sits down" without touching call sites.

## Invariant for the whole track

**Behavior is byte-for-byte unchanged.** The only visible effect is a new
`log::trace!` access line. The gate always returns `Ok(())`; the actor is
always `System` at every current entry point. Nothing is restricted,
nothing's format changes, nothing is persisted differently. Each stage is a
separate `/crush` slice, zero-trust verified, full gate green
(`fmt --all --check` · `clippy --workspace --all-targets -D warnings` ·
`test --workspace --lib` · the function/lifecycle suites).

## Stages (all done)

R1 + the transparent door + the facade entries landed in `0be711f`; the
deeper threading + the door at the resource touches landed in `1b49194`.

### R1 — Access types ✅
Introduce `pub` types at the engine level:
- `Actor` — the acting identity; **default `System`** (the all-bypassing
  owner-of-owners).
- `ResourcePath` — uniform address of a resource in the tree
  (db / store / table / function / namespace).
- `Action` — `Read | Write | Execute | Create | …`.

Purely additive: `pub` types, not yet wired → no dead-code, no behavior.

### R2 — Thread `Actor` (the painful-to-retrofit seam) ✅
Carry `Actor` from the operation entry points (facade `execute` /
`invoke_function`, the server/wire session, tests) **down through the
EXISTING context objects** (`FilterContext` / `TxContext` / `FnCtx` /
the batch-exec context) to every resource touch and to the commit pipeline.
- Default `System` at every current entry; the wire path sets the
  authenticated principal (the server already has it post-auth).
- Rides existing contexts — NOT a new parameter on every function (minimise
  churn).
- No one READS the actor for decisions yet → behavior identical.

Start with **reconnaissance**: map the real access entry points and which
context object flows down through reads / writes / commits / function
invokes. That scoping IS the first move of R2.

### R3 — One door (transparent) ✅
Introduce `authorize(actor: &Actor, path: &ResourcePath, action: Action)
-> Result<(), AccessError>` that returns `Ok(())` and `log::trace!`s the
access (consumes the actor → no dead-code; bonus: an access trace for future
audit). Route EVERY resource-access entry point (read / write / insert /
delete / invoke / create) through it.
- The door allows everything → behavior identical.
- Now there is a SINGLE place where the POSIX check (P4) will later sit, and
  the actor reaches it.

**Order:** R1 → R2 → R3. Each slice green; behavior unchanged.

## Refactor tests (structure, NOT policy)
- The actor reaches a deep operation from its entry point (a wire request's
  principal arrives at the commit pipeline).
- `authorize` is invoked with the correct `(actor, path, action)` for each
  op.
- Everything is still allowed (the door returns `Ok`).

## Explicitly OUT of this refactor (later, own slices — see ACCESS_FABRIC)

| Not refactoring | Why |
|---|---|
| Provenance seal (`created_by/modified_by`, actor in WAL) | changes the record/WAL **format** + adds persisted data (P1) |
| `owner/group/mode` fields on resources | changes the catalogue **format** (P3) |
| Enforcement (real POSIX check instead of `Ok`) | changes **behavior** (P4) |
| Function specifics / row-level / subscriptions | features (P5 / P6) |

After this refactor: provenance + metadata are purely additive, and
enforcement is a single change inside the door.
