בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Shomer — the object & operation hierarchy

The complete tree of securable **objects** (resources) and the **operations**
over them. This is the foundation the enforcement (P4), the metadata
(owner/group/mode, P3) and the DDL build on. It refines `ResourcePath` /
`Action` in `shamir-types::access` (the transparent gate from
`ACCESS_REFACTOR.md` is already in place).

## Objects — the resource tree

```
Root (/)                                  — the system; the admin domain
├── databases/
│   └── <db>                              Database
│       └── <store>                       Store   (a repo / storage backend)
│           └── <table>                   Table
│               ├── <record key>          Record  (a row; leaf)
│               └── indexes/<index>       Index   (derived)
├── functions/                            FunctionNamespace (global container)
│   └── <function>                        Function
├── users/<user>                          User      (a principal)
└── groups/<group>                        Group     (a set of principals)
```

`ResourcePath` is an ordered, traversable path (so the gate can walk
ancestors). It exposes `parent()` and typed constructors
(`ResourcePath::table(db, store, table)`, `::function(name)`, `::record(db,
store, table, key)`, …). Functions are **global** (registered on the
`ShamirDb` instance), so `functions/` and `users/`/`groups/` sit at the root,
beside `databases/`.

Mode-bearing objects (carry owner/group/mode): Root, Database, Store, Table,
Function, FunctionNamespace, User, Group. **Records and Indexes inherit their
Table** (no own metadata until row-level security is enabled).

## Operations — the `Action` set

| Action | POSIX-ish | Meaning |
|---|---|---|
| `Read` | r | read a value / get a record / describe an object |
| `Write` | w | modify a value / update a record / alter an object |
| `Create` | w (on a container) | create a child in a container |
| `Delete` | w (on a container) | remove an object |
| `Execute` | x | invoke a function; **traverse** a container (reach children) |
| `List` | r (on a container) | enumerate a container's children |
| `Manage` | owner/admin only | change owner/group/mode/grants (chmod/chown/chgrp/grant) |

## Object × operation matrix

| Object | Read | Write | Create | Delete | Execute | List | Manage |
|---|---|---|---|---|---|---|---|
| **Root** | — | — | create db | — | — | list dbs | server admin |
| **Database** | describe | — | create store/table | drop db | traverse | list stores/tables | chmod/chown/chgrp |
| **Store** | describe | — | create table | drop store | traverse | list tables | chmod/chown/chgrp |
| **Table** | query rows | insert/update/delete rows | (insert row) | drop table | traverse | — | chmod/chown/chgrp |
| **Record** | get row | update row | — | delete row | — | — | *(inherits Table)* |
| **Index** | *(via Table)* | rebuild | — | drop index | — | — | *(inherits Table)* |
| **FunctionNamespace** | — | — | **create function** | — | — | list functions | — |
| **Function** | describe / source | alter | — | drop | **invoke** | — | chmod/chown/setuid |
| **User** | describe | change own password | — | drop user | — | — | admin |
| **Group** | describe | — | — | drop group | — | list members | add/remove members |

## Rules

- **Traversal**: to act on a deep object the actor needs `Execute` on every
  ancestor container (POSIX `x` on dirs).
- **Inheritance**: Record/Index resolve against their Table's owner/mode.
- **Admin bypass**: the Root owner / admin bypasses all checks.
- **Definer (`setuid`)**: a function may run with its owner's authority
  instead of the caller's (controlled escalation).
- **Capabilities, not tree objects**: egress (`net`) is a capability bit on a
  Function (gated by the function's grant + the host allowlist), NOT a
  `ResourcePath`. Secret-grants (which `env.*` a function may read) are
  likewise function-attached capabilities (already enforced, slice 9).

## Mapping to DDL (later)

- `chmod` / `chown` / `chgrp` on any mode-bearing object → admin ops
  (`Manage`).
- `create_group` / `drop_group` / `add_member` / `remove_member` → Group ops.
- Function `visibility` (public/private) ≈ the `other`-`x` bit; `security`
  (invoker/definer) ≈ `setuid`; create-function = `Create` on
  FunctionNamespace.
- These become `BatchOp` admin variants alongside the existing DDL.

## Implementation order (this goal)

1. **Hierarchy** (this doc) → expand `ResourcePath` (traversable, all object
   types) + `Action` (full set) in `shamir-types::access`; update the R1/R2
   call sites; tests. Gate stays transparent → behavior unchanged.
2. **Metadata** (P3): owner/group/mode envelope on the mode-bearing objects
   (catalogue) + provenance (`created_by`/`modified_by`, P1).
3. **Enforcement** (P4): the gate implements the matrix above (class pick,
   rwx, traversal, admin bypass, setuid, inheritance, capabilities).
4. **DDL**: chmod/chown/chgrp + groups + function visibility/security as
   admin ops.
5. **Tests + benchmarks** throughout (the gate is on the hot path → benchmark
   the check).
