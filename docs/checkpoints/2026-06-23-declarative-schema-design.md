בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Checkpoint — 2026-06-23 (declarative schema validators design committed; awaiting push / Phase 0 impl)

## Session summary

Two completed bodies of work this session, both now committed (3 commits ahead of
origin/master, UNPUSHED). (1) The native↔WASM function-parity campaign (impl), committed
as `cc84d62 feat(parity)` + `bacab51 docs(checkpoints)`. (2) A full DESIGN for a NEW
feature — **declarative schema validators** — committed as `6dd135f docs(design)`.

The declarative-schema-validators feature: a THIRD validator kind (alongside WASM/Native)
declared as DATA (an array of field-rules `[{path, type, constraints}]`, like relational
column types), stored **strictly per-table** in the table's catalogue record, gated by
table authority, compiled to an executable validator on boot/DDL. The design lives in
`docs/design/declarative-schema-validators/` — 10 layer docs (00 overview, 01 engine, 02
DDL+introspection+update-model, 03 storage, 04 interning, 05 permissions, 06 client
rust/js/ts, 07 testing, 08 Phase-0 by-name field interface, 09 builtin-checks incl.
foreign_key/unique).

The design went through THREE oxx-agent review rounds, each grounding it harder against
the codebase and catching real divergences (per-repo not per-table interner; ValidatorBinding
+ synthetic RecordId mechanics; the by-name field-access ABI problem; create_table-has-no-authz-
gate; Dec/Big collapse to Bin on the lens path; FK db-handle/tx-visibility being aspirational
not real). All round-3 fixes applied. Key resolved design decisions: validators are a NARROW
role (`RecordValidator` + by-name `RecordFields`) NOT the general `ShamirFunction`/`Params`
contract — wasm's msgpack de-intern is localized inside its `WasmRecordValidator` adapter
(paid only by wasm), native/declarative are lazy; the by-name invariant extends to FUNCTIONS
too (their FnBatch/FnCtx/Params API is already by-name — interning never leaks to any user
code). Storage: schema + persistent `schema_validator_id` + `schema_version` in the table's
catalogue record; compile-on-boot (load_tables in init) + compile-on-DDL. Permissions: schema
gated by `Action::Write` on the table (set_table_schema; create_table currently has NO authz
gate — a pre-existing gap noted as needing hardening). Phasing: 0 (by-name field interface +
validator-plane refactor, migrates the parity native/wasm validators) → A (pure schema vertical)
→ B (scalar-bridge + format + cross-field) → C (relational foreign_key/unique — ASPIRATIONAL,
needs a NEW tx-scoped read-only validator db-handle, NOT the autocommit DbGateway, + index
integration) → D+ (referential actions).

Nothing in flight. No /loop or /babysit timers. TaskList empty. Working tree clean.

## Active goal

None (`/goal` not set). No babysit cron. TaskList empty.

## TaskList

Empty. (The parity campaign tasks #186-#190 all completed+cleared in prior turns. The
declarative-schema work was DESIGN-only this session — no impl tasks created yet.)

## Decisions

- **Declarative schema = third validator kind, strictly per-table** (chose: store in the
  table's catalogue record, table-Write authz; rejected: global validators-table + binding,
  and rejected per-table interner — interner is per-repo).
- **Validators are a narrow role (`RecordValidator` + `RecordFields`), not `ShamirFunction`** —
  the parity campaign's forcing-validators-through-the-function-contract was the interning leak;
  wasm de-intern localized in its adapter, native/declarative lazy. By-name invariant extends to
  functions (already by-name).
- **Phase C (foreign_key/unique) kept ASPIRATIONAL** — the validator db-handle + tx-scoped read
  snapshot do not exist (DbGateway is autocommit + re-entrant-deadlocks); honestly marked as
  needing new primitives, does NOT block Phase 0/A/B.
- **Committed design docs after 3 review rounds** (user said "коммит"); did NOT push (standing rule).
- **Earlier: committed parity campaign + pushed 5 perf commits** (prior turns).

## Open questions

- **Push?** 3 commits ahead of origin/master (feat(parity), docs(checkpoints), docs(design)) —
  all unpushed. Awaiting explicit "пуш" (pre-push hook re-runs the gate on the working tree).
- **Implement Phase 0?** The design is committed + grounded. Next natural step is `/babygoal`
  on Phase 0 (the `RecordValidator`/`RecordFields` validator-plane refactor + native/wasm
  migration), then Phase A. Awaiting the user's go.
- **create_table authz hardening** — the design flagged that create_table has no authz gate
  today (pre-existing). Whether to harden it (add `authorize_access(table, Create)` in
  add_table_as) is an open product/security decision for Phase A.

## Repo state

```
(working tree clean)
```

```
6dd135f docs(design): declarative schema validators — per-table schema vertical
bacab51 docs(checkpoints): perf-hunt roadmap + parity campaign session checkpoints
cc84d62 feat(parity): native↔WASM function parity — functions, validators, scalars
e145489 perf(query): apply_distinct_qv keep-mask — drop redundant index set
cfd83f7 perf(tx): elide WAL deep-clone + single-pass phantom-predicate validation
```

3 commits ahead of origin/master, nothing pushed. Working tree clean. The parity feature is
implemented + green (committed); the declarative-schema work is DESIGN-only (committed), not
yet implemented.
