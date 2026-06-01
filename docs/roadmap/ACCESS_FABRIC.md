בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Shomer — the access fabric

**Shomer** (שׁוֹמֵר, "guardian") is ShamirDB's access-control subsystem.

> The behavior-preserving substrate refactor we do FIRST (Actor + the
> transparent gate) is isolated in
> [`ACCESS_REFACTOR.md`](ACCESS_REFACTOR.md). This file is the full model +
> the rights stages (P0–P6).

## What it is (the model)

A **hierarchical POSIX-style Discretionary Access Control (DAC)** model:
ownership + group + world, `rwx` mode bits, over resources in a **tree**
with path traversal + inheritance, plus `setuid`-style delegation
(definer-rights) — augmented with **capability bits** for non-resource
powers (egress/`net`), and **per-record provenance** (`created_by` /
`modified_by`) that seeds row-level ownership. Not RBAC (no role-grant
graph), not MAC (no labels). Components keep POSIX-familiar names
(`owner/group/mode/setuid/chmod/chown/chgrp`).

## The resource tree

```
/                          ← admin (root of authority)
├── db1/                   ← database   [owner+group+mode]
│   ├── store/             ← store      [own mode or inherits db]
│   │   └── tableA/        ← table      [owner+group+mode]  ← main boundary
│   │        ├── records   ← inherit the table's mode (+ per-record created_by)
│   │        └── indexes   ← derived overlay → inherit the table
│   └── functions/         ← namespace; `w` here = create a function
│        └── reembed       ← function = file; x=invoke, setuid=definer
└── users/, groups/        ← admin-owned
```

- **Mode-bearing** (carry owner/group/mode): db, store, table, function,
  function-namespace (~5 types — already in the catalogue, +3 fields).
- **Inherit** the container: records, indexes (derived).
- **Capability, not a resource**: egress (`net` bit + allowlist).
- **Provenance**: every record carries an engine-owned, unspoofable header
  `created_by / modified_by / created_at / modified_at`; the WAL entry
  carries the acting principal.

Check (POSIX semantics): pick the FIRST matching class (owner > group >
other) by `actor` vs resource owner/group, check the action's bit, require
`x` on every ancestor (traversal); admin bypasses; `setuid` runs a function
with its owner's authority (definer).

## The five seams (prepare the place before the judgment)

| # | Seam | Inert form (now) | Active form (later) |
|---|---|---|---|
| 1 | **Actor** — identity carried from the edge to every op | default `System` (bypass) | the authenticated principal |
| 2 | **One gate** `authorize(actor, path, action)` | `Ok(())` (allow-all) | the POSIX check |
| 3 | **ResourcePath** — uniform tree addressing | addressing only | the subscription namespace too |
| 4 | **Metadata envelope** owner/group/mode on resources | System-owned, open mode | real plates + chmod/chown |
| 5 | **publish event seam** at commit | null sink | the subscription herald |

Seams 1–2 are the painful-to-retrofit refactor (do them first, no-op).
Seams 3–5 are additive (ride their feature slice).

## Implementation stages

Each stage is a `/crush` slice, zero-trust verified + gated. Stages P0–P3
are **behavior-preserving substrate** (defaults open → nothing restricted);
P4 flips enforcement on; P5 is function specifics; P6 is the future chapter.

- **P0 — Actor threading** (essential refactor, no-op): introduce `Actor`/
  `Principal`, carried from the edge (session) to every operation entry
  (execute, invoke_function, table read/write/insert/delete, function
  create/invoke). Default `System` everywhere; the wire path sets the
  authenticated principal. No gate, no enforcement. Tests: actor flows
  end-to-end; default System.

- **P1 — Provenance seal** (depends on P0): engine-owned record metadata
  header `created_by/modified_by/created_at/modified_at` (principal interned
  to u64; commit clock), stamped at commit from the Actor, separate from the
  user payload (unspoofable). WAL V2 entry gains `actor` (backward-compat:
  old entries = Unknown). `created_by` denormalized forward (survives history
  GC); `modified_by` = latest. Old records read as `created_by = System`.
  This is the data-completion of P0 and seeds row-level ownership for free.

- **P2 — The single gate** (depends on P0): one transparent
  `authorize(actor, ResourcePath, action) -> Result` choke point (`Ok(())`),
  routed through by all resource ops. Formalize `ResourcePath` (the tree).
  Behavior unchanged.

- **P3 — Metadata plates** (depends on P2): owner/group/mode envelope on the
  ~5 container catalogue records (default System-owned, open mode); admin ops
  chmod/chown/chgrp + group create / add-member. Still open → unchanged.

- **P4 — Seat the doorkeeper** (depends on P0–P3): the gate implements the
  POSIX check (class pick, rwx, ancestor traversal, admin bypass, setuid).
  Records inherit table mode; indexes inherit table; `net` capability gated.
  Enforcement ON — but System-owned/open defaults keep existing flows; only
  explicitly-restricted resources gate.

- **P5 — Function specifics** (depends on P4; folds the deferred slice-10
  function-RBAC, but as POSIX-mode not RBAC): visibility public/private via
  other-`x`; definer via setuid; create-function = `w` on the
  functions-namespace; secret-grants (already enforced, slice 9) + `net`
  capability. The "re-embed" capstone — private + definer + net +
  secret-grant + allowlist — fully enforced.

- **P6 — later, by real need** (the reactive chapter): row-level ownership
  enforcement (using P1's `created_by`); subscriptions (publish seam #5 + WS
  protocol + `r`-gated subscribe + function-triggers). A big separate
  chapter — deferred until the need is real.

## Discipline

P0–P3 change nothing observable (Actor=System, gate transparent, plates
open). P4 turns the model on with open defaults so nothing breaks. Prepare
seams, then policy, then features — and don't build the role-graph: this is
DAC, not RBAC; sharing-with-some is the **group**, not a grant DSL.
