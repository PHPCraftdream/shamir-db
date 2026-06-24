# Recovery brief — TS builders: keyset `.after` + foreignKey `onDelete`

RECOVERY IMPLEMENTATION TASK (TDD, TypeScript). Do NOT commit, do NOT push.

⛔ ABSOLUTELY FORBIDDEN: `git reset` / `checkout` / `clean` / `stash` / `restore`
/ `rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits. (A prior agent ran `git reset --hard` and
destroyed hours of work.)

Work dir: crates/shamir-client-ts. TS tests: `npx vitest run <file>`.

The Rust side already landed + committed: `Pagination::After` (wire tag
`"After"`, key=WireValue[], optional limit) and `FkAction`/`ForeignKeyDto.on_delete`
(snake_case: no_action|restrict|cascade|set_null; builder default restrict).
Mirror these in the TS client.

### Part 1 — keyset `.after` (TS)
- crates/shamir-client-ts/src/core/types/query.ts — extend the `Pagination`
  union with `{ mode: 'After'; key: WireValue[]; limit?: number }` (PascalCase
  `'After'`, matching the existing `'LimitOffset'`/`'Page'`). Use `WireValue`
  (from types/write.ts) for key elements.
- crates/shamir-client-ts/src/core/builders/query.ts — add an `'after'`
  internal pagination mode + `after(key: WireValue[], limit?: number): this`
  that stores key+limit; `build()` emits `{ mode: 'After', key, ...(limit!=null?{limit}:{}) }`.
- Test (extend src/core/builders/__tests__/query.test.ts): `Query.from('t')
  .orderByAsc('score').after([30], 2).build()` → `pagination` deep-equals
  `{ mode: 'After', key: [30], limit: 2 }`; without limit → no `limit` key.

### Part 2 — foreignKey `onDelete` (TS)
- crates/shamir-client-ts/src/core/types/ddl.ts — add the on_delete wire field
  to the foreign-key constraint type: action strings `'no_action' | 'restrict'
  | 'cascade' | 'set_null'`.
- crates/shamir-client-ts/src/core/builders/ddl.ts — `.foreignKey(table, field,
  opts?: { onDelete?: ... })` where onDelete DEFAULTS to `'restrict'` and emits
  `on_delete: '<action>'` in the constraint.
- crates/shamir-client-ts/src/core/types/index.ts — re-export any new type if
  the existing pattern requires it.
- Test (extend src/core/builders/__tests__/ddl.test.ts): `.foreignKey('parent',
  'id')` emits `on_delete: 'restrict'`; `{ onDelete: 'cascade' }` emits cascade.

Run: `npx vitest run src/core/builders/__tests__/query.test.ts
src/core/builders/__tests__/ddl.test.ts` — all green. Also `npx tsc --noEmit`
must not introduce NEW errors in the files you touched (pre-existing errors in
e2e-schema-validators.test.ts are known and not yours).

Surgical diffs, imports at top. Final message: files touched + the TS shapes +
pass counts. NEVER touch git.
