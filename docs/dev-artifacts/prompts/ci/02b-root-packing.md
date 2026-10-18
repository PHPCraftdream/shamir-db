# CI-02b — pack `crates/shamir-client-ts/` root configs and docs

Parent brief: `docs/dev-artifacts/prompts/ci/02-layout-sweep-client-ts.md`. The
crate root has 10 tracked entries; pack moveable ones into two folders.

## Rules (binding)
- ⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
  `git rm`, or any git command that mutates the working tree or index. Only
  edit files; the orchestrator commits. Moving = create new file with file
  tools + `rm` the old one (never `git mv`).
- NO `cargo` / `npm` / `tsc` / `vitest` / `node` or any compile, test or
  install command. Allowed: read-only git, `rg`, `wc -l`,
  `python -I scripts/ci/check_layout.py`, file tools, `mkdir`, `rm`.

## Task (repo-relative to crates/shamir-client-ts/)
1. `mkdir configs`; move `tsconfig.build.json` → `configs/tsconfig.build.json`
   with exactly three path fixes (everything else byte-identical):
   `"extends": "../tsconfig.json"`, `"outDir": "../dist"`,
   `"exclude": ["../node_modules", "../dist", "../src/**/__tests__/**"]`
   (exclude/include paths resolve relative to the file's own directory; the
   inherited `rootDir: "src"` and `include` are declared in the base
   tsconfig.json and stay correct).
2. Move `vitest.setup.ts` → `configs/vitest.setup.ts` byte-identical. Update
   `vitest.config.ts`: `setupFiles: ['./configs/vitest.setup.ts']` (nothing
   else). vitest.config.ts itself STAYS at the crate root.
3. Update `package.json` scripts: `"build": "tsc -p configs/tsconfig.build.json"`
   (nothing else changes; `test`, `typecheck` untouched).
4. `mkdir docs`; move `README.md` and `TRANSPORT_SPEC.md` byte-identical into
   `docs/`. Their relative link `[./TRANSPORT_SPEC.md](./TRANSPORT_SPEC.md)`
   still resolves. Do NOT touch package-lock.json, .gitignore, tsconfig.json.
5. Outside the crate (minimal reference update): in `tests/e2e/README.md`
   update the command string that names this package's build
   (`tsc -p tsconfig.build.json` → `tsc -p configs/tsconfig.build.json`),
   prose otherwise unchanged.

Result: root = 8 entries (.gitignore, package.json, package-lock.json, src,
tsconfig.json, vitest.config.ts, configs/, docs/). 8 is the intended final
state (npm/tsc/vitest/git anchor the other entries); do NOT squeeze below 8.

## Verify
- `python -I scripts/ci/check_layout.py` exits 0; `git status --porcelain` shows
  only the expected adds/deletes/modifications.
Report: entry count before/after, files touched.
