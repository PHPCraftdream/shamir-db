# R2d design addendum: scheduled online backups

This resolves R2a's append-only retention question for scheduled copies only.
The committed brief is `cf36bb3f8`; the release plan requires scheduled,
restorable copies, bounded retention and an alert when copies stop.

## Boot configuration

The optional top-level `backup` block enables the schedule; omission disables
it. Admin still takes explicit `to`, may create it, and retains its
absolute/outside-source check and authorization.

| Field | Default and boot validation |
|---|---|
| `backup.destination` | Required when enabled. Existing absolute directory; reject missing, non-directory, non-private, link/reparse roots, or a canonical destination equal to, inside, or an ancestor of canonical `data_dir`. Use `shamir-private-dir::verify_private_dir` on the original path before canonicalization; do not create or repair it. |
| `backup.interval` | Required. One positive ASCII whole number immediately followed by `s`, `m`, `h`, or `d` (`d` = 24 hours). No fractions, compounds, signs or whitespace. Checked parse, multiplication and monotonic deadline; reject less than 60 seconds. No existing humantime parser assumed. |
| `backup.window` | Optional, no restriction by default. Strict local `HH:MM-HH:MM`, 00:00–23:59; reject malformed or equal endpoints. |
| `backup.run_on_start` | Boolean, default false. |
| `backup.retention.keep_last` | Integer, default 7; reject less than 1 and more than 1000 (one pass scans at most 4,096 root entries, shared with foreign entries). |

Validation is syntax-only (`Config::validate`); the destination's existence, privacy and disjointness from `data_dir` are checked when the server resolves the schedule for launch, so `restore --from` with an absent backup mount still validates.

Retain the canonical root and recheck its path and privacy before use;
never fall back to admin's create-if-missing behavior for the scheduled path.

## Scheduler and lifecycle

Start a server-owned task after the registry is ready. Use monotonic ticks from
boot readiness: first tick at boot with `run_on_start`, otherwise at boot plus
one interval. On late wake evaluate at most one tick and move the deadline to
the next future slot using saturating monotonic elapsed time; no replay, queue
or immediate retry. Check future deadline
arithmetic too; report and stop the schedule if it becomes unrepresentable.
A safe helper in `shamir-types`' existing time surface converts the current
instant to local minute using its existing chrono 0.4.45, with no Cargo edge.
Evaluate at the tick (including DST): `[start,end)` normally, `>= start OR
< end` overnight. A repeated local hour may admit two distinct monotonic ticks;
a missing hour cannot admit one. The window limits starts, not completion.

| In precedence order | Action |
|---|---|
| Shutdown | No new work; cancel cooperatively and join. |
| Outside window | Count `skipped_window`. |
| Registry/permit occupied, or own post-copy work still running | Count `skipped_busy`; do not enqueue. |
| Otherwise | Attempt the same registry/adapter/protocol path as admin; count `started` for admitted work and one terminal outcome per non-skip attempt. |

Use an internal system actor, no session and no follower gate for this
node-local schedule; admin superuser/follower policy and wire types stay as
is. Share the registry and nonblocking permit, not a separate check-then-start
lock. Await a run-specific terminal result, not a polled status which a later
admin run could replace. Use the actual protocol result, including early mkdir
and launch errors, cancellation and unwind, not the progress stage. Keep all
schedule state in memory. Hold the scheduled admission through retention to
avoid overlap with admin writes in the root; never wait for the permit on a tick.

Bias shutdown over ticks and retention starts. Graceful teardown requests
cooperative cancellation and joins the in-flight work; an active syscall is not
interrupted, and a copy already in final verification can finish successfully.
The existing 30-second `serve` deadline can report an unclean shutdown before
that work finishes; forced exit can leave partial output, including a partially
deleted copy. Do not start retention during shutdown. Protocol directories use UTC
`YYYYMMDD_HHMMSS` at second resolution; collisions fail without overwrite.

## Ownership, verification and pruning

A v2 manifest proves integrity, not who created it. After scheduled protocol
success, retention verifies and atomically tags the fresh copy in the
verifier-exempt `backup.meta.json` BEFORE scanning the rest of the root, so a
scan that then refuses (budget, cancellation, an unreadable foreign entry)
still leaves the fresh copy owned; an unverifiable fresh copy is never tagged
(preserve existing fields; stage on the same filesystem, flush and atomically
replace). The versioned tag
uses a digest of the stable random ID in `server_meta` and canonical `data_dir`.
The ID is created on first enablement if absent and reused across restarts.
Neither the server's public identity nor its keys are inputs, so rotating keys
does not orphan old copies. Relocation or cloning to another canonical path
changes the tag; old tags are not rewritten. This is ownership metadata, not
schedule state or authentication. Use a separate root for a clone. One server
account and trusted operators control the root: no external edits, concurrent
writers, or mounts inside it. Operators may copy out but must not copy back.
In particular, Unix device checks cannot detect same-device bind mounts.
Tagging failure counts as retention failure; preserve the verified backup and
skip pruning, even if a temp sidecar remains. No manifest, protocol or restore change.

1. After success only, scan every direct child; require the exact timestamp name,
   a real directory, and matching versioned owner tag. Preflight without following
   links; reject symlinks, reparse points, special files and (on Unix) entries
   on a different device. Check actual sizes
   and every manifest/WAL/stamp path the verifier may read is contained and safe
   before calling existing v2 `verify_manifest`. Require parsed v2 online manifest.
2. Use sequential checked budgets: 4,096 root entries, 32 tree levels, 64 KiB
   sidecar, 16 MiB manifest, 100,000 tree entries per candidate, and 1,000,000
   tree entries and 1 TiB of input per pass. Byte budget: the pass charges the
   verifier's real reads, by size, when the manifest's file set is known. Each
   manifest-listed file is charged once (the claimed-file loop); a WAL segment
   is charged a second time (the verifier's WAL loop hashes it again) and the
   stamp a second time (`inspect` reads it before the manifest-file hash). The
   manifest is handed to the verifier and never re-read, the tree walk charges
   directory entries only, so unknown files in a rejected candidate cost one
   entry each. Exhausted budget or incomplete root scan means no deletion;
   count failure.
   Foreign files, partial or corrupt copies, unsafe trees, other owners and
   unmarked R2a/admin copies stay untouched and are counted. If the just-created
   copy cannot be verified within the same pass, do not delete anything.
3. Sort verified owned copies by manifest creation nanoseconds, then directory
   name. Protect the newest `keep_last` and separately the just-created copy
   (clock rollback can put it outside that set). Delete oldest eligible first.
   Even if all copies are old, the newest floor stays. The verified protected
   set is never a deletion target.
4. Use existing cleanup primitive `std::fs::remove_dir_all` on each eligible
   directory, one at a time. Check cancellation between calls and join an active
   call; do not abandon it. Log and count deletion errors without failing the
   backup. A partially removed copy may be left for operator cleanup; never retry
   it as a verified copy without re-verification.

## Metrics and alerting

Extend the existing backup snapshot/export on `/metrics`; keep R2a series intact.
Added series use the `shamir_backup_` prefix; only the scheduled counter has a label.

| Suffix | Meaning |
|---|---|
| `last_success_timestamp_seconds` | Unix seconds of latest successful admin or scheduled online protocol, zero before success. |
| `last_duration_seconds`, `last_bytes` | Duration and full manifest payload bytes of that success, zero initially. Share duration measurement/state with existing metrics while preserving their semantics; count WAL and sidecar payload listed in the manifest once, not metadata or retry traffic. |
| `scheduled_runs_total{result=…}` | `started`, `skipped_busy`, `skipped_window`, `failed`, `completed`; failure/completion refer to copy outcome, not retention. |
| `scheduled_failure_streak`, `schedule_enabled` | Saturating streak increments on scheduled `failed`, resets on `completed`, unchanged on skips/admin. Enabled is 0/1. Both reset at boot. |
| `schedule_started_timestamp_seconds` | Unix time recorded at each enabled scheduler boot; 0 before enablement. |
| `retention_deleted_total`, `retention_failed_total`, `retention_untouched` | Successful deletions, retention errors, and noneligible foreign/unverifiable entry count from the last complete classification, respectively. |

Ship `deploy/alerts/shamir-backup.rules.yml`: `BackupStale` gates on enabled
and measures age from the last success, or from the positive schedule-start
timestamp before the first success. Both paths use the same threshold
(1.5 × the interval; 24h gives 36h) and `for: 5m`. Adjust the threshold for
backup runtime, allowed windows and planned outages. Admin success also clears
staleness. `BackupFailing`: enabled,
streak >= 3 and scheduled `failed` count >= 3; the streak proves ordering.
`BackupRetentionFailing`: positive increase
in retention failures over a configured lookback. Logs contain no payloads.

## CI acceptance and boundaries

Test validation refusals, injected tick/window/DST and shutdown decisions, busy
and early failure accounting, ownership/restart/clone and sidecar failures,
unsafe or over-budget trees, incomplete scans, corrupt/foreign entries, floor/age
and clock rollback, and no pruning on failed copies. Real server: drive per-run
ticks and await completion signals without sleeps; allow wall stamp advancement
between runs (inject test time if needed). Verify, restore and scrape actual
copies, check pruning and admin latest-success, and parse rules/check metric names.
Confirm affected test selection on Linux, Windows and macOS before release.
Update Russian operations §8 with config, alert wiring, triage and unchanged restore.
Defer object storage delivery, PITR, dashboards and client changes. Built-in
backup encryption (R2c) was dropped; encryption is the operator's responsibility.
