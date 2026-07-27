# Brief for F-33 Step 6 (#840, P2) — document the hybrid repo engine, amend in-memory's persistence claims

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only edit
files; the orchestrator commits.

## Context

F-33 is now fully implemented and proven end-to-end (Steps 1-5, all
landed: `606eb6f3`, `c32c4e3d`, `af4053da`, `606a8b48`, `ca95939d`).
`CREATE REPO ... ENGINE 'hybrid'` is a real, working, tested feature:
table CONFIGURATION (index/sorted-index/functional-index definitions,
schema validators, buffer config, the field-name interner) durably
mirrors to disk, while table DATA (`__history__`/`__data__`/`__tx__`/
`__changelog__`) stays fully ephemeral in-memory — wiped on every
restart. This step is DOCS-ONLY: no production code changes. Its job is
to make this a discoverable, correctly-described feature in the
TS-client-facing guide docs, and to fix the one existing doc claim that
is now incomplete because it predates `hybrid`'s existence.

**Read the file below in full first** — this is the ONLY file with a
substantive claim that needs correcting, plus where the new feature
belongs.

## What to change

### 1. `docs/guide-docs/guide/03-storage.md` — the incomplete claim

Section "2. Репозитории" (~line 53-79) currently presents exactly TWO
engine choices: the durable-by-default `fjall` (implicit, no `engine`
field) and an explicit `engine: 'in_memory'` opt-out, describing the
opt-out as: *"`in_memory`-репозиторий — эфемерный scratch-пространство:
данные не переживают рестарт."* This sentence is now incomplete — it
doesn't distinguish "table config" from "table data" at all (because
before `hybrid` existed, `in_memory` losing BOTH on restart was simply
the only ephemeral behavior available, so the distinction didn't need
naming). Now there are three choices along a durability spectrum, and the
doc should present them as such:

- **`fjall`** (default): everything durable — config AND data survive a
  restart.
- **`in_memory`** (opt-out): everything ephemeral — config AND data are
  both lost on restart. Scratch space, caches, sessions, tests.
- **`hybrid`** (new, opt-in): table CONFIGURATION (indexes, schema
  validators, buffer tuning) survives a restart; table DATA does not.
  For workloads where the shape of the data (indexes/validation rules)
  is expensive to redeclare after every restart but the rows themselves
  are genuinely disposable (e.g. a high-churn cache table whose schema
  took real DDL work to set up, or a staging table that's re-seeded from
  an external source on every boot but whose index/validator setup
  shouldn't have to be re-run by the application).

Rewrite this section (and the matching bullet in "Что важно знать уже
сейчас", ~line 267-269) to present all three, with a short DDL example for
`hybrid` in the same style as the existing `in_memory` example (wire form
comment + TS builder call):

```ts
// wire form: { "create_repo": "cache_repo", "engine": "hybrid" }
await db.run(ddl.createRepo('cache_repo', { engine: 'hybrid' }));
```

Note **`hybrid` requires the server to be running with a `data_dir`**
(same requirement as the durable `fjall` default) — unlike `fjall`/no-`engine`,
which silently falls back to `in_memory` when there's no `data_dir`,
`hybrid` errors clearly if requested without one (an explicit durability
promise for the config half must not be silently downgraded).

### 2. `docs/guide-docs/guide/README.md`'s floor-3 table-of-contents row

(~line 31) currently reads: `Репозитории (durable-by-default),
`in_memory`-scratch, бэкап, интроспекция ...`. Add `hybrid` to this list
so the table of contents reflects the third choice, e.g. `Репозитории
(durable-by-default), `in_memory`-scratch, `hybrid`-config-only, бэкап,
...` (keep the row's existing terse, comma-separated style — don't expand
it into a sentence).

### 3. Check for any OTHER public guide/architecture doc making the same
   now-incomplete claim

Grep `docs/guide-docs/` for other places asserting (in any phrasing) that
an in-memory/ephemeral repo loses "everything"/"all data" on restart
without qualifying config vs. data — `docs/guide-docs/architecture/ARCHITECTURE.md`
and `docs/guide-docs/security/data-protection.md` are worth checking
specifically (both reference `in_memory`/durability elsewhere in this
codebase's docs). If you find another such claim, amend it the same way;
if you find none beyond `03-storage.md`/`README.md`, say so explicitly in
your summary rather than silently skipping the check.

### 4. `docs/guide-docs/KNOWN_LIMITATIONS.md` — one new entry

Add a new entry (find the right existing numbered section by topic, or
add a new one if none fits — this file is organized by topic, not
chronologically) documenting `hybrid`'s current residual scope, so a
reader doesn't assume it does more than it does:

- `hybrid` is not yet a supported `dst_engine` for `MigrateRepo` — check
  `crates/shamir-db/src/shamir_db/execute/admin_migration.rs`'s
  `dst_engine` match (~line 112-120, per Step 4's own investigation notes
  in `docs/dev-artifacts/prompts/post-alpha/74-f33-step4-hybrid-ddl-surface.md`)
  to confirm the exact current supported set before writing this, don't
  guess.
- `hybrid` requires the `fjall` cargo feature (same as the durable
  default) — a build with `default-features = false` and `fjall` excluded
  cannot create a hybrid repo at all; `ENGINE 'hybrid'` falls into the
  unsupported-engine error path in that configuration, same as `fjall`
  itself would.

Do NOT invent additional residuals beyond what you can verify against the
actual landed code (Steps 1-5's commits, listed above) — if you're unsure
whether something is a real residual, check the code before writing it
down.

## Constraints

- Docs-only. Do not touch any `.rs` file.
- Keep the existing Russian conversational tone and terse style of
  `docs/guide-docs/guide/*.md` — these are the TS-client-facing "floor"
  guides, not the Rust engine internals docs. Match the voice already
  there (see how `in_memory` is currently introduced for the target
  register).
- `docs/guide-docs/KNOWN_LIMITATIONS.md` is in English, matching its
  existing entries' style and heading conventions — don't switch its
  language.
- No dev-artifacts/checkpoints should be touched, moved, or deleted (this
  repo's durable-history rule — not relevant to this task's scope, but
  stated for clarity: this step touches ONLY the 3 files named above,
  nothing under `docs/dev-artifacts/` or `docs/checkpoints/`).

## Verification the orchestrator will run

Docs-only step — no build/test gate applies. The orchestrator will read
the full diff for accuracy against the landed F-33 code and Russian/English
consistency before committing.
