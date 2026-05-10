# scripts/

Single-purpose dev utilities. Cross-platform pairs (`*.sh` + `*.ps1`)
where the script is something a developer might run on either Linux,
macOS, or Windows.

## test-all

Runs every test in the workspace through one command, parses cargo's
output into a friendly summary, and exits non-zero only if a test
actually failed (or cargo couldn't build at all).

```bash
# Linux / macOS / Git-Bash on Windows
scripts/test-all.sh                      # whole workspace
scripts/test-all.sh shamir-server        # one crate
scripts/test-all.sh shamir-engine shamir-storage    # several
scripts/test-all.sh -- --nocapture       # forward flags to `cargo test`
scripts/test-all.sh shamir-server -- --test-threads=1
```

```powershell
# Native PowerShell
.\scripts\test-all.ps1
.\scripts\test-all.ps1 shamir-server
.\scripts\test-all.ps1 -- --nocapture
```

Output ends with a summary block:

```
── summary ──
   target:   workspace
   elapsed:  142s
   passed:   1178
   failed:   0
   ignored:  0
   log:      target/test-all.log

all green
```

The full transcript is saved to `target/test-all.log` so you can grep
a failure detail without re-running.
