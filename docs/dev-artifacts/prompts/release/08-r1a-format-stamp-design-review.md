# Brief: independent review of the format-stamp design (release-1 R1a)

## Why

`docs/dev-artifacts/design/format-stamp.md` was written by one executor from `docs/dev-artifacts/prompts/release/07-r1a-format-stamp-design.md`.
The project cycle is design, independent review, brief, implementation. You are the independent review: do not trust the
author's claims, check them against the code and against the release plan
(`docs/dev-artifacts/release-1/README.md`, `01-versioning-and-migrations.md`).

## What

1. **Verify every OBSERVED claim.** Open each cited `path:line`; mark claims that are wrong, stale or unsupported.
   Re-derive independently the place in the open path where the stamp check must run (what the process writes before it:
   the data-dir lock, WAL recovery, the system store, the audit log, compile cache) and the list of subcommands that open a
   data directory and what each of them needs (read-only inspect, stamp, refuse).
2. **Attack the stamp protocol.** Torn write, crash between each step of tmp / sync / rename / directory sync, Windows
   rename-over-existing and its failure modes, antivirus or sharing-violation retries, a stamp copied from another
   directory, two processes racing on an empty directory, restore of a backup over a stamped directory, a read-only
   volume, a stamp newer than the binary combined with an interrupted older-format write. For each: is the design's
   behaviour safe (never destroys data, never silently accepts newer data)? Write the missing case down.
3. **Check the decision table** is complete and consistent with the prose (every combination of stamp state x directory
   state x epoch relation has exactly one action, the same in section 3 and in the tests of section 6).
4. **Check the epoch policy** of section 4 against the real format surfaces (WAL sidecar `WMT2`/`WMT3`, backup manifest
   `format_version`, seed manifest, vector snapshot codec, index meta version, function row `call_signature`): is anything
   that can make the previous binary misread or lose data left without an owner?
5. **Check the implementation plan**: crate and folder choice against the layout rules (7 entries per folder, 1000 lines
   per file), dependency direction (the `shamir-engine` crate is being cut into crates right now: nothing may be added to
   its `repo`, `table`, `tx`, `query` layers), the two PR-sized steps, the named tests and which of them must run on all
   three OS.
6. **Check the open questions** and their defaults against the release plan section 6; flag any default that is unsafe.

## Output

Edit `docs/dev-artifacts/design/format-stamp.md` in place: fix every defect you can fix with certainty, and append a
final section "Review" (English, numbered findings: severity, what was wrong, evidence as `path:line`, what you changed or
what stays open for the owner). Findings you could not settle go to the open questions with a recommended default. Do not
change the structure or the decisions that are sound; do not touch any other file. If the design is sound, say so in the
Review section and list what you checked.

## Do not

Change code, tests or CI files. Rewrite the document for style. Add scope (R1b, R1c, R1d designs).

## Observable acceptance

The document's Review section exists, every finding carries evidence, and `python -I scripts/ci/check_layout.py` is OK.
The final report lists the findings by severity and what changed in the document.
