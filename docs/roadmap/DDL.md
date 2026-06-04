# DDL — completion, execution, rights, folders

**Status:** design / proposed.

How to finish ShamirDB's DDL (the schema/management surface) and its
execution: how rights are enforced, how function folders work, and the other
concerns ("что ещё"). Grounded in the access model in
`crates/shamir-types/src/access.rs` and the existing admin `BatchOp` surface.

---

## 0. Where we already are (do NOT rebuild)

- **DDL surface:** 30+ admin `BatchOp` variants already exist and are
  wire-reachable via `DbRequest::Execute`:
  `create_db/drop_db`, `create_repo/drop_repo`, `create_table/drop_table`,
  `create_index/drop_index`, `set/get/alter_buffer_config`, `list`,
  `access_tree`, `start/commit/rollback/migration_status`,
  `create_user/drop_user`, `create_role/drop_role`, `grant/revoke_role`,
  `chmod/chown/chgrp`, `create_group/drop_group`, `add/remove_group_member`.
- **Rights model + enforcement are mature and wired:**
  - `ResourcePath` (Root → Database → Store → Table → {Record, Index};
    FunctionNamespace → Function; User; Group) with `parent()`/`ancestors()`.
  - `ResourceMeta { owner: Actor, group: Option<u64>, mode: u16 }` — POSIX
    12-bit mode + setuid; persisted per catalogue record (`inject_into` /
    `from_record`).
  - `Action { Read, Write, Create, Delete, Execute, List, Manage }`;
    `permits(actor, meta, action, in_group)` — POSIX first-match
    (System bypass; Manage = owner-only; owner→group→other).
  - **The facade gate** (`shamir-db` `shamir_db.rs`): for a `(ResourcePath,
    Action)` it walks `path.ancestors()` requiring `Execute` (traverse) on
    each container, resolves group membership (`user_in_group`), then checks
    `permits` on the target. This is full POSIX path-traversal authz.
  - **Actor plumbing is live:** `db_handler::session_actor` maps the
    authenticated session → `Actor::User(principal_id)`; `execute_as(actor)`
    threads it into the executor.
- **Gaps:**
  - **Functions** (create/drop/rename) and **validators** (create/drop/rename/
    bind/unbind/list) are **facade-only** — not yet `BatchOp`s on the wire.
  - **No function folders** — `ResourcePath` has `Function`/`FunctionNamespace`
    but no `FunctionFolder`, though the funclib already uses slash-paths
    (`math/abs`). (This is task #118.)
  - The gate is not yet **uniformly** invoked on every admin path; create
    defaults are still **open** (`owner = System`, `mode = 0o777`).

So "finish DDL + execution" is **close gaps + flip defaults + add folders**,
not build authz or the op set from scratch.

---

## 1. Completing the DDL surface (ops)

- **Function DDL → `BatchOp`:** `create_function` (source→WASM) / `drop_function`
  / `rename_function`, wire-reachable (today facade-only).
- **Validator DDL → `BatchOp`:** `create/drop/rename/bind/unbind/list_validator`
  (this is the validator follow-up #167 / S6).
- **Idempotency modifiers:** `if_not_exists` / `if_exists` / `or_replace` on
  create/drop; `cascade` where referential drops are allowed.
- **Identifier validation:** valid names, reserved names, folder path segment
  rules, name-uniqueness on create/rename.
- **Introspection:** extend `list` / `access_tree` to functions, validators,
  folders; add `describe_table` / `describe_index` (fields seen, indexes,
  bound validators).
- ShamirDB is schemaless (MessagePack + interned fields) ⇒ **no
  `alter_table`-schema**; "alter" means indexes / buffer config / validators /
  access.

---

## 2. Execution — how a DDL op runs end to end

One uniform pipeline per admin op:

```
wire DbRequest::Execute → batch executor → is_admin() → AdminExecutor
  → [GATE]  gate(actor, ResourcePath, Action)              ← §3 rights
  → validate (name / uniqueness / references / idempotency)
  → mutate catalogue:
        SystemStore   (db / repo / table / function / validator / user / group / access)
        info-twin     (per-table: indexes, validator bindings — MetaKey::*)
  → set ResourceMeta on the new resource (owner = actor, mode = default)
  → durable flush (as save_function does: data_store().flush())
  → result / structured error
```

- **Transactionality:** a DDL op inside a `transactional` batch must roll back
  with it; a standalone DDL op is an atomic catalogue write + flush. Decide
  whether DDL and DML may mix in one tx (admin ops are currently dispatched
  before the DML branch).
- **Location:** admin dispatch lives in `query/batch/executor.rs` + the
  `shamir-db` facade (which owns `SystemStore` and the compile pipeline).

---

## 3. Rights for DDL (the core of "execution of rights")

Every DDL op is a `(Action, ResourcePath)` run through the existing gate
(ancestor `Execute`-traversal + target `permits` + group):

| Operation | ResourcePath | Action |
|---|---|---|
| create_db | `Root` | Create |
| create_repo | `Database` | Create |
| create_table | `Store` | Create |
| drop_table | `Table` (or Write on `Store`) | Delete |
| create_index / bind_validator | `Table` | Write |
| create_function | `FunctionNamespace` / `FunctionFolder` | Create |
| create_validator | `FunctionNamespace` | Create |
| chmod / chown / chgrp | target | **Manage** (owner-or-admin only) |
| grant / revoke, create_user / create_group | `Root` / `User` / `Group` | Manage / Create |
| list / access_tree | container | List / Manage-on-Root |

Three things to finish in rights-execution:

1. **owner-on-create:** a newly created resource gets `ResourceMeta.owner =
   actor` (not `System`). The creator owns what they make. This enables
   **delegation**: the owner of a `Database` passes `Create` on it and owns the
   child repos/tables → they can manage access to their subtree (the AX thread:
   "a DB owner creates users scoped to their DB").
2. **default mode:** move new-resource default from `0o777` (open) to a sane
   value (e.g. `0o750`), behind an **enforcement flag** so the open→enforced
   switch is gradual and non-breaking.
3. **uniform gate:** audit that EVERY admin op calls the gate with the correct
   `(path, action)`.

**setuid = SECURITY DEFINER.** The mode setuid bit already exists; a function
with setuid runs with its **owner's** privileges. That is exactly the
getter-only pattern: a user holds `Execute` on the function but **no** `Read`
on the tables; the function reads them as its owner and returns only the
filtered result. The building block is in place.

---

## 4. Function folders (#118)

- Add `ResourcePath::FunctionFolder { path: Vec<String> }`; a function becomes
  folder-qualified (`fn://reports/daily/orders`). `parent()`:
  `Function(["reports","daily","orders"])` → `FunctionFolder(["reports","daily"])`
  → `FunctionFolder(["reports"])` → `FunctionNamespace` → `Root`.
- **Folder permissions come for free** through the gate's existing ancestor
  traversal: invoking `reports/daily/orders` requires `Execute` on every folder
  ancestor *and* on the function. A folder is a securable container with its
  own owner/group/mode (like a store).
- **DDL:** `create_function_folder` + auto-`mkdir -p` on function create (or
  explicit create); `chmod/chown/chgrp` on a folder; `list` a folder.
- **Registry:** the funclib is already folder-namespaced (`math/abs`); #118
  wires that to the access tree + DDL + traversal.
- **Getter-only scenario:** grant a user `Execute` on `/reports/` + the setuid
  functions inside, and **no** read on any table → they "see only filtered
  output from procedures, at DB speed".

---

## 5. What else (beyond rights & folders)

- **Defaults & mode tightening** — open→enforced transition without breakage
  (flag / migration); who is the bootstrap superuser.
- **Uniqueness & validation** — name collisions → error; valid identifiers;
  reserved names; folder-path bounds.
- **Referential integrity** — refuse (or `cascade`) dropping: a DB with repos,
  a table with indexes / bound validators, a function used as a validator, a
  group with owned resources. The validator `bound_in` guard is the pattern to
  generalise.
- **Idempotency** — `if_[not_]exists` / `or_replace` / `drop … cascade`.
- **Introspection** — `list` / `describe` / `access_tree` across every type
  (functions / validators / folders / bindings).
- **Atomicity + durability** — catalogue write + flush; DDL-in-tx + rollback;
  no partial states.
- **Audit** — every DDL op to the audit log (the server already has the
  appender).
- **Error model** — structured codes (`exists` / `not_found` / `access_denied`
  / `still_referenced`).
- **Migration interplay** — DDL vs the online-migration ops
  (start/commit/rollback already exist).

---

## 6. Decomposition

- **DDL-A — engine / wire.** owner-on-create + uniform-gate audit;
  `FunctionFolder` (#118) + folder DDL + traversal; function-DDL and
  validator-DDL over the wire (#167); idempotency + referential integrity +
  introspection; default-mode / enforcement flag.
- **DDL-B — builder.** A typed `ddl` module in `shamir-query-builder` for all
  admin ops (`Ddl::create_table(...)`, `create_index`, `create_user`, `grant`,
  `chmod`, `create_validator`, `bind_validator`, …) → `BatchOp`, composable
  into `Batch`. Starts with the existing ops; extends as DDL-A lands new ones.
  (This is the deferred "P7 typed admin ops" from `QUERY_BUILDER.md`.)
- **AX — access model.** Owner-delegated user admin (a DB owner creates users
  scoped to their DB(s)) + getter-only users (Execute on setuid/Definer
  functions, no table read) — built on the existing gate. See also
  `ACCESS_FABRIC.md`.

### Dependency note
DDL-B can typed-wrap the 30+ existing admin ops **now** (no wait); function /
validator DDL need their wire op (DDL-A) before DDL-B can wrap them.
