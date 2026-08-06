# Follow-up brief #2 — P1-1 (#1014): fix the actual test failures + finish JSON name resolution

## Context

Your previous pass on session `t1014-doctor-cli` hit its own `--timeout 60m`
mid-work ("Context deadline exceeded"). It left real progress (Gap 1
escalation write-up accepted as-is — good call, do not revisit it; Gap 2's
human-text output fix landed correctly) but two things are now broken/
incomplete and MUST be fixed in this pass:

## Finding A (CRITICAL) — the doctor_e2e tests were never actually run, and 3 of 4 FAIL

Your prior report claimed fmt/clippy green but never mentioned running the
actual test suite. Investigation shows why: at some point `cargo nextest
run --lib --no-fail-fast -p shamir-server doctor_e2e` was run (see the now
-deleted stray `test_output.txt` you left in the repo root — cleaned up by
the orchestrator, don't recreate stray files at the repo root). `--lib`
only runs library unit tests — `doctor_e2e.rs` lives in `tests/` (an
integration test binary), so this filter silently matched **zero** tests
("error: no tests to run") and was never actually exercised.

Running it correctly through the MANDATORY wrapper —
`./scripts/test.sh -p shamir-server --full -- doctor` — shows the truth:

```
FAIL doctor_healthy_data_dir_exit_zero
FAIL doctor_json_output_works
FAIL doctor_filter_options_work
```

All three fail identically:
```
assertion `left == right` failed: doctor should exit 0 on healthy data,
stderr: Error: config validation: listeners[0].addr `"127.0.0.1:0"` is not
a valid socket address: invalid socket address syntax
  left: Some(1)
 right: Some(0)
```

**Root cause, already diagnosed — do not re-investigate, just fix:**
`write_ktav_config()` / the inline `ktav_content` strings in
`doctor_e2e.rs` only set `data_dir` / `logging.level` / `logging.file`.
But `crates/shamir-server/src/config.rs`'s `Config` struct (see its
schema doc comment at the top of the file, lines ~10-47) has THREE fields
with no `#[serde(default)]`: `kdf_defaults: KdfConfig`, `listeners:
Vec<ListenerConfig>`, `tls: TlsConfig` — all three are **required** by
`Config::from_file`/`ktav`. `main.rs:218-219` calls
`Config::from_file(&cli.config)?; config.validate()?;` **unconditionally
for every subcommand**, including offline ones like `doctor` — there is
no bypass. Every OTHER offline-command test in this crate
(`access_tree_e2e.rs`, `backup_restore_e2e.rs`) sidesteps this entirely by
calling the command's `run()` function **directly in-process** with a
Rust-constructed `Config` struct — `doctor_e2e.rs` is the ONLY test file
in `crates/shamir-server/tests/` that shells out to a real `cargo run`
subprocess (confirmed: `grep -rn 'Command::new("cargo")'` in that
directory matches only `doctor_e2e.rs`).

**Fix — add the missing required sections to every ktav config string this
test file writes**, matching the schema in `config.rs`'s doc comment
(copy its example, minimal values are fine — `doctor::run()` only ever
reads `config.data_dir`, so `listeners`/`tls`/`kdf_defaults` just need to
satisfy `Config::validate()`'s STRUCTURAL checks; they don't need to
actually bind a socket or point at real cert files):

```
data_dir = "<path>"
logging.level = "warn"
logging.file = null
kdf_defaults = { memory_kb = 19456, time = 2, parallelism = 1, argon2_version = 19 }
listeners = [ { kind = "tcp", addr = "127.0.0.1:0", profile = "tls_exporter" } ]
tls = { cert_path = "<some path>/cert.pem", key_path = "<some path>/key.pem" }
```

(Adjust exact ktav syntax to match what `ktav::from_file` actually
accepts — check `config.rs`'s doc comment block precisely, and/or look at
how `ktav`-format files are written elsewhere in the repo, e.g. any
`.ktav` fixture under `crates/shamir-server` or `docs/`, for the correct
list/nested-object syntax; the snippet above is illustrative of WHICH
fields are needed, not a guaranteed-correct literal.)

After fixing, re-run through the wrapper and confirm all 4
`doctor_e2e` tests pass:
```
./scripts/test.sh -p shamir-server --full -- doctor
```
Report the REAL output (pass/fail counts), not a paraphrase.

**Also: do not use raw `cargo nextest run` again.** This repo's
`CLAUDE.md` mandates `./scripts/test.sh` (or `cargo t`/`cargo tl`) as the
sole test entry point — it wraps nextest with per-test timeouts and the
correct scope flags (`--lib` vs `--full`/integration). Use the wrapper
exactly as shown above for every verification in this pass.

## Finding B — Gap 2 is only half-fixed: JSON output still has no resolved names

The human-text path (`print_index_health`) now correctly shows resolved
names — good, verified by reading the diff, keep that as-is. But the
`--json`/`--pretty` JSON output path was never touched: `TableReport` (and
by extension `DoctorReport`) still serializes each `IndexHealth` /
`Index2Health` with only their existing fields (`name_interned: u64` for
regular/unique/sorted; `Index2Health` already has `.name: String` for
free, that one's fine). A JSON consumer has no way to resolve
`name_interned` to a human name — the `index_name_map` computed in `run()`
is a local variable, never attached to the serialized report.

**Fix**: attach the resolved name to each serialized `IndexHealth` entry
in `TableReport`'s output. Simplest approach that doesn't touch
`shamir_db::engine::table::doctor::IndexHealth` itself (it's an
engine-owned type, out of scope to modify): wrap each `IndexHealth` in a
small server-local struct that adds the resolved name alongside it for
serialization purposes, e.g.:

```rust
#[derive(Debug, Clone, Serialize)]
struct NamedIndexHealth {
    #[serde(flatten)]
    health: IndexHealth,
    resolved_name: String,
}
```

and change `TableReport`'s `verify: VerifyReport` field's regular/unique/
sorted index vectors to use this wrapper type when building the
serializable report (you'll need a small local mirror of the relevant
`VerifyReport` fields, or restructure `TableReport` to carry
`Vec<NamedIndexHealth>` per family directly instead of embedding the raw
`VerifyReport`). Use your judgment on the exact shape — the requirement
is just: **JSON output must show, per index, its resolved human-readable
name**, not a bare `name_interned` integer with no way to resolve it.

Verify by actually running `doctor --json` against a data dir with at
least one named index (extend/reuse the existing
`doctor_filter_options_work` or `doctor_json_output_works` test, or add
a new one) and asserting the JSON contains the index's real name string,
not just `name_interned`.

## Constraints (same as before)

- `CLAUDE.md`: no inline `#[cfg(test)] mod tests {}` — already respected.
- No stray files in the repo root (`test_output.txt` etc.) — this
  orchestrator will keep cleaning them, but don't create them in the
  first place; redirect any scratch output under `.crush/stdin/` or a
  proper `/tmp`-style path instead if you need to capture command output
  for your own reference.
- Gate, run for real and report the actual output: `cargo fmt -p
  shamir-server -- --check`, `cargo clippy --workspace --all-targets --
  -D warnings`, `./scripts/test.sh -p shamir-server --full -- doctor`
  (scope the test run to the doctor tests as shown; a full
  `--full` run with no filter is also fine if you have time budget, but
  at minimum the doctor-scoped run must be shown passing).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Definition of done

- [ ] All 4 `doctor_e2e.rs` tests pass when run via
      `./scripts/test.sh -p shamir-server --full -- doctor` — real output
      shown in your report, not paraphrased.
- [ ] JSON (`--json`/`--pretty`) output shows resolved index names per
      index, not bare `name_interned` integers — proven by a test
      assertion, not just described.
- [ ] fmt/clippy clean, real command output reported.
- [ ] No stray files left at the repo root.
