# CI-02f — verifier for the shamir-client-ts layout sweep

Parent brief: `docs/dev-artifacts/prompts/ci/02-layout-sweep-client-ts.md`.
You verify a completed pure-structural refactor. Pre-sweep state =
commit `effaa28b` (use `git show effaa28b:<path>`; read-only git only).

## Rules (binding)
- ⛔ NEVER run any git command that mutates the working tree or index. NO
  `cargo` / `npm` / `tsc` / `vitest` / `node` or any compile/test/install
  command. Allowed: read-only git, `rg`, `wc -l`, `python` (stdlib only,
  including `python -I scripts/ci/check_layout.py`), file tools.
- Do not modify tracked files. If you need a scratch script, put it in the
  system temp directory, not the repo, and delete it after.

## Checklist (all of it, in order)
1. `python -I scripts/ci/check_layout.py` exits 0; print its output.
2. Inventory diff vs effaa28b for crates/shamir-client-ts: list every
   pre-sweep file and where its content lives now (moved / split / deleted /
   untouched). NO content may vanish: for every moved file, compare
   `git show effaa28b:<old>` to the new file — byte-identical except import
   specifier lines (and, for the 6 deliberately rewritten files — new
   client-internals.ts, ddl split parts, the two split test files — item
   sets must match per the stage briefs 02a/02d/02e).
3. Export-surface equality: build the exported-name list of
   src/index.ts, src/browser.ts, core/builders/index.ts, core/types/index.ts,
   core/builders/ddl/index.ts (and each pre-split builder/type module) from
   effaa28b and from the working tree; assert identical name sets.
4. Import resolution: for every relative `from '<spec>.js'` in every tracked
   .ts under crates/shamir-client-ts/src, resolve the specifier against the
   importing file's directory and assert the target file exists (write a
   small python checker in temp for this; stdlib only).
5. Test integrity: every describe/it title present under
   src/__tests__ in effaa28b appears exactly once in the working tree;
   `*.test.ts` count = 35 + untouched sub-suite counts
   (core/protocol 4, core/client 6, core/support 2, core/types 1,
   core/builders 15, core/receipts 1, core/visibility 2, core/platform 1).
6. Layout: no folder under crates/shamir-client-ts exceeds 7 direct entries
   except the crate root (8, intentional); no .ts file exceeds 1000 lines.
7. No `@ts-ignore` / `@ts-expect-error` / absolute machine paths added.
Report PASS/FAIL per checklist item with specifics for any FAIL.
