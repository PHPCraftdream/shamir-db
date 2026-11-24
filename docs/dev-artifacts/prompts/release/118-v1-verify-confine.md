# Brief: V1-VERIFY-CONFINE (task #2011, from #2010 item 1), done by hand

## Defect (pre-existing, landed with R5-5)

The v1 branch of `verify_manifest_raw` (`shamir-sync` `backup/offline.rs`) checked only the LEXICAL manifest path (`reject_unsafe_manifest_path`: no absolute path, no `..`) and then opened `snapshot_dir.join(path)` and hashed it. A symlink inside the snapshot (the entry itself, or an intermediate directory component) was followed, so a manifest entry could make `backup-verify` read a file OUTSIDE the snapshot and report `verified` when that file matched the listed size and digest. The on-disk walk (`collect_relative_file_paths`) uses `DirEntry::file_type`, which does not follow links, so the link was not reported as an unmanifested file either. The v2 verifier already walks every component with `symlink_metadata` (`confined_snapshot_path`).

## Change

1. v1 takes the entry path through the same `v2_verify::confined_snapshot_path` before hashing: a symlink component or a non-regular final component refuses (`UnconfinedManifestPath` / `ManifestPathNotRegularFile`) BEFORE any open. Doc lines that called the v1 path "byte-identical" now say it adds per-entry confinement.
2. Regression test `verify_manifest_refuses_symlinks_out_of_the_snapshot` (unix): a link to an existing outside file carrying the matching size and digest, a dangling link (an open would fail as plain `Io`, so the typed refusal proves nothing was opened), and a link in an intermediate directory.

## Non-goals

No new error class in the `backup-verify` report (a variant the CLI does not name reports `corrupt`), no change to v2, restore, or the retention path (they verify through the same function). The remaining #2010 items stay in #2010.

## Acceptance

`fmt`, clippy `-D warnings` for `shamir-sync` and `shamir-server`, the layout gate and the visibility check exit 0; the full CI tier is green on the head, with the new unix test passing on Ubuntu.

## Review results (two independent reviews, both verdict FAIL; fixed or re-scoped)

Round 1:

* FIXED (P1, introduced by the first version): on Windows an interior piece such as `C:` after a normal directory name resets the `PathBuf::push` walk of `confined_snapshot_path`, and a backslash bundles several hops into one metadata check; the v1 loop only had the whole-path `reject_unsafe_manifest_path`. A lexical shape check now runs before the walk (see round 2 for its final form).
* FIXED (P2): the remaining wording ("byte-identical", "(v2)" labels, "before any open") now matches the code: a path is refused by a lexical or metadata check before the entry is opened for content reads; a dangling v1 entry link returns `UnconfinedManifestPath`, not `Io(NotFound)`; v1 `..` stays `UnsafeManifestPath`.
* RE-SCOPED to #2010 (pre-existing, outside the manifest-entry fix): the fixed-name control files `manifest.json` and `format.stamp` are read through static links (attacker condition: write access to the snapshot directory, so the same class, not an escalation); the check-then-open window between the path check and the hash or copy (restore enumeration, `staged_listing` checks presence and type only) also exists for v2.
* Confirmed: Windows classifies junctions and mount points as symlinks in the toolchain used; the unix test is registered, non-vacuous and has no Windows unused import; retention is v2-online only.

Round 2 (narrow review of the round-1 fix):

* FIXED (P1, a regression of the round-1 fix): applying the strict v2 naming rules to v1 entries on Unix refused snapshots a real v1 backup produces (a TLS file named `tls:cert.pem`, `key.pem ` with a trailing space or `cert.pem.`: the server writes to the configured path and the v1 writer records the name without the v2 rules). The shape rule is now host-OS: on Unix only the component structure is checked (no empty, `.` or `..` component; `:`, `\`, trailing dot or space are literal), on Windows the strict v2 rule applies (`confined_v1_snapshot_path`; the v2 callers keep `confined_snapshot_path`). Unix writer-to-verifier round-trip test `backup_with_nonportable_unix_names_verifies`; the Windows-only shapes in `verify_manifest_refuses_noncanonical_entry_shapes` run under `cfg!(windows)`.
* FIXED (P2): wording no longer equates lexical refusal with resolving outside the snapshot, qualifies containment to an unchanged snapshot, and does not claim that the v1 writer emits only canonical names.
* Not done (said by the reviewer): no direct test of `ManifestPathNotRegularFile`.

Round 3 (narrow review of the round-2 fix, verdict PASS_WITH_NOTES, no P0 to P2): both P3 notes were fixed: the error-variant doc now says that the strict naming rules apply to v2 on every host and to v1 on Windows, and `verify_manifest_refuses_a_non_regular_final_component` asserts the typed `ManifestPathNotRegularFile` for a v1 entry naming a directory. Noted by the reviewer and left as they are (not lexical): case-insensitive filesystems can alias spellings and the walk does not reject hard links or mount points; the cache preflight classifier still treats a backslash as a separator, so a Unix name starting with a literal `cache\` is outside the round-trip test.
