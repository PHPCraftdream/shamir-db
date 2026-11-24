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
