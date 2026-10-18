# CI-02c — regroup `crates/shamir-client-ts/src/core/` into topical folders

Parent brief: `docs/dev-artifacts/prompts/ci/02-layout-sweep-client-ts.md`.
core/ has 19 direct entries; target 7. Imports use explicit `.js` ESM
specifiers (NodeNext). Pure moves + relative-import fixes only; byte-content
of moved files changes ONLY in import specifier lines.

## Rules (binding)
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `git rm`, or any git command that mutates the working tree or index. Only
  edit files; the orchestrator commits. Moving = create new + `rm` old.
- NO `cargo` / `npm` / `tsc` / `vitest` / `node` or any compile/test/install
  command. Allowed: read-only git, `rg`, `wc -l`,
  `python -I scripts/ci/check_layout.py`, file tools, `mkdir`, `rm`.
- Imports at the top; preserve comments; no reformatting; LF endings.

## Move map (create at new path, fix that file's relative imports, rm old)
- `framing.ts`, `protocol.ts`, `hmac.ts`, `scram.ts` → `core/protocol/`
- `errors.ts`, `field-map.ts`, `interner-ops.ts`, `platform.ts`, `exec-ctx.ts` → `core/support/`
- `client.ts`, `client-internals.ts`, `cursor-iterator.ts`, `db.ts`,
  `subscription-handle.ts`, `subscription-router.ts` → `core/client/`
- `core/__tests__/framing.test.ts`, `hmac.test.ts`, `protocol.test.ts`,
  `scram.test.ts` → `core/protocol/__tests__/`
- `core/__tests__/field-map.test.ts`, `interner-ops.test.ts` → `core/support/__tests__/`
- `core/__tests__/client.test.ts`, `connect-local.test.ts`,
  `cursor-iterator.test.ts`, `db.test.ts`, `subscription-handle.test.ts`,
  `subscription-router.test.ts` → `core/client/__tests__/`
- `core/__tests__/batch-types.test.ts`: read its imports; move it next to the
  module it tests under `core/types/__tests__/` (create the folder; types files
  themselves do NOT move in this stage). If ambiguous, park it at
  `core/types/__tests__/` and say so in the report.
`receipts/`, `visibility/`, `builders/`, `types/*.ts` do NOT move in this stage.

## Import rewrites (exhaustive — grep every old specifier across src/)
- Inside moved files: e.g. client/ client.ts's `'./framing.js'` →
  `'../protocol/framing.js'`, `'./platform.js'` → `'../support/platform.js'`,
  `'./types/batch.js'` → `'../types/batch.js'` (depth +1 for anything still in
  core/), `'./receipts/index.js'` → `'../receipts/index.js'`;
  `'./client-internals.js'` stays (same folder).
- `src/index.ts` + `src/browser.ts`: `'./core/client.js'` →
  `'./core/client/client.js'`; `'./core/errors.js'` →
  `'./core/support/errors.js'`; `'./core/subscription-router.js'` /
  `'./core/subscription-handle.js'` / `'./core/cursor-iterator.js'` /
  `'./core/db.js'` → same under `'./core/client/…'`. Paths into
  core/types, core/builders, core/receipts, core/visibility are UNCHANGED.
- `src/platform/node.ts` + `src/platform/browser.ts`:
  `'../core/platform.js'` → `'../core/support/platform.js'`.
- `src/__tests__/*.ts`: `'../core/framing.js'` → `'../core/protocol/framing.js'`;
  `'../core/platform.js'` → `'../core/support/platform.js'`;
  `'../core/client.js'` → `'../core/client/client.js'`
  (`'../core/types/index.js'` unchanged).
- Any remaining importer found by `rg "'\./<name>\.js'|'\.\./<name>\.js'|'\.\./\.\./<name>\.js'"` per moved basename
  (framing|protocol|hmac|scram|errors|field-map|interner-ops|platform|exec-ctx|client|client-internals|cursor-iterator|db|subscription-handle|subscription-router)
  must be updated — including inside receipts/, visibility/, builders/, types/
  if they reference moved modules.

Final core/ root (7 entries): builders/, client/, protocol/, support/,
receipts/, types/, visibility/. Subfolder counts: client/ 7 (6 files +
__tests__ with 6 test files), protocol/ 5, support/ 6.

## Verify
- `python -I scripts/ci/check_layout.py` exits 0.
- `rg` proves no stale specifiers: none of the old direct forms
  (`'./framing.js'` outside protocol/, `'../core/framing.js'`,
  `'./core/client.js'`, `'../core/platform.js'`, `'./errors.js'` outside its
  folder, etc.) remain anywhere under crates/shamir-client-ts/src.
- Every moved file still exists exactly once (old path gone, new present).
Report: final tree of core/ with entry counts, list of import-rewritten files.
