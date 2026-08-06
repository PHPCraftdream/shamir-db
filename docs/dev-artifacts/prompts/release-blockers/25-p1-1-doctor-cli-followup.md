# Follow-up brief — P1-1 (#1014): close two gaps in the `doctor` CLI pass

## Context

The first `/crush` pass for #1014 (session `t1014-doctor-cli`, brief
`docs/dev-artifacts/prompts/release-blockers/24-p1-1-doctor-cli.md`) landed
`crates/shamir-server/src/doctor.rs` + `tests/doctor_e2e.rs` +
`main.rs`/`lib.rs` wiring. Zero-trust review (mandatory before any commit,
per this repo's session discipline) found two real gaps against the
original brief's definition-of-done. Both must be closed in THIS pass —
do not touch anything else.

## Gap 1 — missing test scenarios (b) and (c)

The original brief's Definition of Done required at least 3 integration
tests covering:
- (a) a healthy data dir → exit 0, clean report — **DONE**
  (`doctor_healthy_data_dir_exit_zero`).
- (b) a data dir with a deliberately Building/stuck index → non-zero exit,
  report correctly flags it — **MISSING**.
- (c) `--apply` actually heals it and a SECOND `doctor` run reports
  healthy — **MISSING**.

`doctor_e2e.rs` currently has 4 tests but none of them construct an
unhealthy table: `doctor_healthy_data_dir_exit_zero`,
`doctor_nonexistent_data_dir_error` (just a bad path, not an unhealthy
table), `doctor_filter_options_work`, `doctor_json_output_works` (all
healthy-path). Add tests for (b) and (c).

**How to construct a durably-stuck-Building index for an offline CLI
test** — this is the crux of the gap, investigate before writing tests:

- `crates/shamir-index/src/base_index/backfill_pause_hook.rs` —
  `BackfillPauseHook` is a production (non-`#[cfg(test)]`-gated) `pub
  struct`, already used by `crates/shamir-engine/src/table/tests/doctor_tests.rs`
  (see `verify_detects_building_regular_index` /
  `verify_detects_building_sorted_index`, ~line 545/693) to park a
  `create_index`/`create_sorted_index` call mid-backfill and observe the
  `Building` state via `verify()` while parked — but those tests observe
  it IN-PROCESS, in the same test, without ever going through a restart.
  Read that file in full first.
- `crates/shamir-engine/src/table/tests/p1011_reader_drain_tests.rs`
  (this session's own #1011 work) also uses a pause-hook pattern
  (`set_lookup_pause_hook`) — read it for the general "register a hook,
  spawn the operation, wait for it to park, assert, release" shape.
- `crates/shamir-engine/src/repo/tests/hybrid_table_open_tests.rs` shows
  this codebase's standard way to simulate "restart" for a durable-state
  test: **do not spawn a second OS process** — reopen the table/repo
  handle from the SAME underlying storage directory within the same test
  process (drop the in-memory `TableManager`, re-fetch via
  `RepoInstance::get_table`), which forces a re-read from durable
  storage exactly like a cold restart would. Read a couple of its test
  functions (e.g. `create_does_not_error_or_panic_across_restart`) for
  the pattern.
- `crates/shamir-server/tests/doctor_e2e.rs`'s existing tests instead
  shell out to a real `cargo run -p shamir-server -- doctor` **subprocess**
  against a data_dir a prior in-process server wrote and then
  `handle.shutdown()`-closed (so the file lock is released for the
  subprocess to reopen). For scenario (b)/(c) you need BOTH: an
  in-process phase that leaves a table durably stuck at `Building`
  (via the pause hook, parked, then `handle.shutdown()` WITHOUT letting
  the backfill complete — confirm `handle.shutdown()` doesn't itself
  wait for or cancel in-flight backfills in a way that finishes them;
  if it does, you may need to abort/drop the parked task rather than
  await it, or find the exact point where the `Building` registration is
  durably persisted independent of backfill completion and stop there),
  followed by the SAME subprocess-based `doctor` invocation the existing
  tests already use.
- If, after investigating, a clean durably-stuck-Building state genuinely
  cannot be constructed from `shamir-server`'s black-box test binary
  without disproportionate new engine-side test scaffolding, STOP and
  report exactly why in your final report (which specific step failed
  and what would be needed) rather than shipping a fake/vacuous test —
  a test that doesn't actually leave the index in `Building` before
  invoking `doctor` proves nothing. This is a legitimate escalation, not
  a failure — the original brief anticipated this might need engine-side
  help.

## Gap 2 — resolved index names are computed but never shown

`doctor.rs::resolve_index_names()` builds a `Vec<String>` of
human-readable index names via the table's interner and stores it on
`TableReport::index_names` — but nothing ever prints or otherwise
surfaces this field:
- `print_human_report`/`print_index_health` still print raw
  `name_interned={}` integers, never a resolved name.
- The JSON path serializes `index_names` as a flat, deduped,
  alphabetically-sorted list on the `TableReport` — but it's
  disconnected from which specific `IndexHealth`/`Index2Health` entry it
  belongs to, so a JSON consumer cannot tell which name maps to which
  index either.

The original brief was explicit: "`IndexHealth.name_interned` is a raw
`u64` needing interner resolution for human-readable display (mirrors
`access_tree.rs`'s existing name-resolution pattern)" — the intent was
per-index resolved names in the actual report output, not a disconnected
side list.

**Fix**: resolve each index's name at the point where it's printed /
serialized, not into a separate flat list.
- `print_index_health(idx: &IndexHealth)` and
  `print_index2_health(idx: &Index2Health)` need the resolved name
  passed in (or resolved inline) so the human-readable text shows the
  actual name instead of `name_interned=<u64>`. Note
  `Index2Health` already carries a resolved `.name: String` field
  directly (see its printer — it already does `idx.name`, no interner
  needed there); the gap is specific to `IndexHealth` (regular/unique/
  sorted), which only carries `name_interned: u64`.
  the interner. Simplest correct fix: build a `name_interned -> String`
  map (reuse the existing `new_fx_set` + interner-resolution logic you
  already wrote, just don't dedup/collapse it into a `Vec<String>` —
  keep it as a lookup map) and thread that resolved name through to
  `print_index_health` (and the JSON path — replace or augment each
  `IndexHealth` entry's representation with the resolved name, don't
  just leave the flat `index_names` list; either add a
  `resolved_name: Option<String>` alongside each printed/serialized
  index entry, or produce a name-keyed structure — your call on the
  exact shape, but the resolved name MUST be visibly attached to its
  specific index in both text and JSON output).
- Remove the now-redundant flat `index_names: Vec<String>` field on
  `TableReport` once the per-index resolution replaces it (don't leave
  dead output alongside the fix).

## Constraints (same as the original brief)

- Follow `CLAUDE.md`: no inline `#[cfg(test)] mod tests {}`, tests in
  `tests/` directories (already true for `doctor_e2e.rs`, keep it that
  way).
- Gate: `cargo fmt -p shamir-server -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `./scripts/test.sh -p shamir-server --full` (run the REAL wrapper —
  `cargo test` is blocked outright in this repo; your prior pass's
  report only showed fmt+clippy results, not a test run — make sure
  this pass's report shows the actual `./scripts/test.sh` output/summary,
  not just fmt/clippy).

⛔ NEVER run `git reset` / `checkout` / `clean` / `stash` / `restore` /
`rm`, or any git command that mutates the working tree or index. Only
edit files; the orchestrator commits.

## Definition of done

- [ ] `doctor_e2e.rs` has a test proving a durably-stuck-Building index
      is detected by `doctor` (non-zero exit, report flags it) — OR a
      clear written escalation explaining why this isn't constructible
      from `shamir-server`'s test binary without further engine-side
      scaffolding.
- [ ] `doctor_e2e.rs` has a test proving `--apply` heals that same
      unhealthy table and a second `doctor` run reports it healthy (only
      if the above scenario was constructible).
- [ ] Resolved index names are visibly attached to their specific index
      in both human-readable text output and JSON output — not a
      disconnected flat list.
- [ ] fmt/clippy/`./scripts/test.sh -p shamir-server --full` all green,
      exact commands and their real output/summary reported (not just
      "gates green").
