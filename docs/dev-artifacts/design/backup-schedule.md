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
cooperative cancellation and awaits the scheduler task. Each admitted run has TWO
signals: the copy outcome is published at protocol completion — streak,
last-success stamp and the completed/failed counters, before retention — and a
LATE "worker finished" signal is sent only after the retention pass (including a
caught retention panic) and the final record publication (`running = false`).
The scheduler awaits that late signal before its own task ends, so
`ShutdownOwner::join_schedule` still waits for the WHOLE run — retention
included — and the data-directory instance lock is released only after it. The
wait is the run's completion signal, not a join of the native runner thread. An
active syscall is not interrupted, and a copy already in final verification can
finish successfully. The existing 30-second `serve` deadline can report an
unclean shutdown but does not abandon the worker: the scheduled worker and the
data-directory lock are awaited after the deadline, and nothing forces a process
exit; a kill by the operator can still leave partial output, including a
partially deleted copy. A cancellation observed before the retention pass starts
skips its scan. Protocol directories use UTC `YYYYMMDD_HHMMSS` at second
resolution; collisions fail without overwrite.

## Publication ordering, last error and cleanup outcome

The copy outcome publishes at protocol completion, BEFORE retention:
`note_success` (last-success timestamp — also persisted durably to the
`server_meta` store at this point — bytes, duration), `note_finished`
(completed/failed counters, `active` gauge) and the scheduler's outcome signal
all fire there (the late "worker finished" signal follows after retention). At
boot the last-success stamp is seeded back from that row
(`install_backup_meta_store`), so the staleness evidence outlives the process.
The registry admission (`running`) stays true through retention, so an admin
copy and the next tick are refused
while retention runs; the copy permit is released before it. Retention and
adoption run under `catch_unwind`: a panic there is ONE retention failure
(`shamir_backup_retention_failed_total`), never a copy failure, and a
cancellation observed inside the pass is not a failure at all. The scheduler
resets `shamir_backup_scheduled_failure_streak` on that completion, so a long
retention never keeps the failure streak active.

A scheduled run's `last_error` is rendered path-free as `<stage>: <reason>`
(`OnlineBackupError::stage_and_reason`). Stage words: `preparing`, `captured`,
`copying`, `sealing`, `verifying`, `done`, `failed`. Reason tokens: `busy`,
`cancelled`, `port-failed`, `manifest-failed`, `verify-failed`,
`recopy-cap-exceeded`, `io-<kind>`, `destination-io-<kind>`, `copy-cancelled`,
`copy-io-<kind>`, `copy-destination-io-<kind>`, `copy-recopy-cap`,
`copy-audit-files-unstable`
(`<kind>` is the kebab-case `io::ErrorKind` name; `io-<kind>` is a source-side
io error, `destination-io-<kind>` a destination-side one — the snapshot child's
create and the manifest/backup-metadata publication), plus a
`; cleanup-failed-<kind>` suffix when a failed run could not remove its own
partial snapshot directory — the original failure stays primary and the suffix
carries only the `io::ErrorKind`. An admin run keeps the full `Display` chain,
and the privileged `BackupStatus` keeps its intentional `destination`/`dest_dir`
paths. The scheduled-only logs (marker provisioning, manifest-capture refusal)
carry `reason`/`kind` fields and no path.

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
   a real directory, and matching versioned owner tag — or, for an untagged child,
   an exact match against the bounded in-memory provenance (recorded name, manifest
   creation time and manifest-byte SHA-256 from the post-protocol capture), which is
   tagged first and only then becomes an owned candidate. Preflight without following
   links; reject symlinks, reparse points, special files and (on Unix) entries
   on a different device. Check actual sizes
   and every manifest/WAL/stamp path the verifier may read is contained and safe
   before calling existing v2 `verify_retention_candidate` (`preflight.rs`). Require parsed v2
   online manifest.
2. Use sequential checked budgets: 4,096 root entries, 32 tree levels, 64 KiB
   sidecar, 16 MiB manifest, 100,000 tree entries per candidate, and 1,000,000
   tree entries and 1 TiB of input per pass. Byte budget: the pass charges the
   verifier's real reads, by size, when the manifest's file set is known. Each
   manifest-listed file is charged once (the claimed-file loop); a WAL segment
   is charged a second time (the verifier's WAL loop hashes it again) and the
   stamp a second time (`inspect` reads it before the manifest-file hash). The
   manifest is handed to the verifier and never re-read within one verification,
   the tree walk charges directory entries only, so unknown files in a rejected
   candidate cost one entry each, and file-byte charges are applied only after
   the cheap structural refusals. Bounded control-file reads are charged as
   well, each with one extra sentinel byte (`meta.len() + 1`): `manifest.json`
   once and `backup.meta.json` once per candidate; the adoption loop re-reads
   the sidecar and the bounded manifest (same caps and charges). Exhausted
   budget or incomplete root scan means no deletion;
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
| `last_success_timestamp_seconds` | Unix seconds of the NEWEST evidence that copies work: the latest successful admin or scheduled online protocol (persisted durably at copy success in the `server_meta` store, fjall keyspace `server_meta_v1`, key `backup_last_success_unix_secs_v1`, epoch seconds) or, before the first success, the schedule's FIRST arming time (key `backup_armed_at_unix_secs_v1`, written once by `arm_schedule_clock` when a schedule is first enabled and removed by `disarm_schedule_clock` when none is configured). Removing the schedule deletes the row, so re-adding one arms a NEW time at the first boot that has a schedule again, and the staleness clock restarts there; the gap with no schedule is not reported as staleness. That removal is best-effort like the arming write AND not atomic (the row is removed, then the store is persisted): a failure logs one bounded `persist-armed-at-failed` token and the durable outcome is UNCERTAIN — a later boot may still see the OLD row (and keep that arming time) or none (and arm afresh). Arming evidence counts only when THIS boot has a configured schedule: an unconfigured boot ignores the armed-at row entirely, even when the disarm failed to delete it — `install_backup_meta_store(…, false)` takes the disarm path, which returns 0 without reading the row — so that boot's series reports only the last-success row (or Zero), while a configured boot seeds from the row it reads (`ArmingAction::Keep`, `WriteNow` or `UseNowWithoutWriting`). An UNREADABLE row is used for the current boot only and is never overwritten. `install_backup_meta_store` seeds the in-memory cell with `max(armed_at, last_success)` at boot, `note_success` raises it with `fetch_max`, and each scrape `fetch_min`s it to "now" IN PLACE: a future stamp is replaced by the current time exactly ONCE and later scrapes report that same clamped value, so the exported age grows. Before the first success the series therefore reports the first arming, and a restored `data_dir` carries both persisted rows of the backup it came from. Zero whenever no USABLE evidence remains (no configured schedule, no readable armed-at row and no last-success row) — not only when no schedule was ever armed: an arm-then-remove deletes the row (a failed removal may leave the old one — the outcome is uncertain), and an UNREADABLE armed-at row is used for the current boot only (`ArmingAction::UseNowWithoutWriting`, one bounded `read-armed-at-failed` token) and is NEVER overwritten, so the durable first-arming stamp survives a broken read. |
| `last_duration_seconds`, `last_bytes` | Duration and full manifest payload bytes of the last SUCCESSFUL copy, zero before the first success of the process. In-memory only: the durable rows carry no duration or bytes, so after a restart both read 0 until the first success in the new process. `shamir_backup_last_duration_seconds` is a last-value gauge rendered from the per-scrape snapshot and reads 0 before the first success; `shamir_backup_duration_seconds` is observable-only and OMITTED until the first success of the process (no fabricated zero). Neither changes on a failed run. Share duration measurement/state with existing metrics while preserving their semantics; count WAL and sidecar payload listed in the manifest once, not metadata or retry traffic. |
| `scheduled_runs_total{result=…}` | `started`, `skipped_busy`, `skipped_window`, `failed`, `completed`; the terminal outcome publishes at protocol completion and refers to the copy, not retention. |
| `scheduled_failure_streak`, `schedule_enabled` | Saturating streak increments on scheduled `failed` (including an admission refusal) and resets on `completed`, i.e. at copy completion before retention; unchanged on skips/admin, and a retention panic does not touch it. Enabled is 0/1. Both are process state, zero at boot. |
| `schedule_started_timestamp_seconds` | Unix time recorded at each enabled scheduler boot; 0 before enablement, a new value after a restart. |
| `retention_deleted_total`, `retention_failed_total`, `retention_untouched` | Successful deletions, retention errors, and noneligible foreign/unverifiable entry count from the last complete classification, respectively. Errors include marker-provisioning failures before the copy and before the pass, tag/delete-lease/deletion errors and a panic in the pass; they exclude reader-busy skips (`skipped_in_use`), a cancelled pass and the best-effort boot marker attempt. |

The four LAST-VALUE gauges (`last_success_timestamp_seconds`,
`scheduled_failure_streak`, `last_duration_seconds`, `duration_seconds`) are not
stored in the process-global recorder: `render_backup_scrape_gauges` renders them
from each scrape's own `BackupMetricsSnapshot` into that response body, the same
way the per-server activity gauges are rendered. Two overlapping scrapes
therefore cannot apply in reverse order and an older concurrent scrape cannot
publish an older value; the cumulative counters keep the recorder's
`.absolute()` (`fetch_max`) monotonicity. A value the step cannot observe (the
pin duration, the WAL segment count) stays unset rather than fabricated.

Ship `deploy/alerts/shamir-backup.rules.yml`: `BackupStale` gates on enabled
and measures age from the last success — before the first success the series
reports the schedule's first arming time, so the same expression measures that
age too (the unchanged zero-value fallback to the positive schedule-start
timestamp remains in the rule). The last-success series
survives restarts: it is seeded at boot from the newest of the durable
`server_meta` rows and clamped to the current time once at scrape, so a restart
no longer zeroes the staleness clock. A restored data directory carries the
persisted rows of the backup it came from, so after a restore the series reports
the age of that restored evidence, not zero. Both paths use the same threshold
(1.5 × the interval; 24h gives 36h) and `for: 5m`. Adjust the threshold for
backup runtime, allowed windows and planned outages. Admin success also clears
staleness. `BackupFailing`: enabled,
streak >= 3 and scheduled `failed` count >= 3; the streak proves ordering and
resets at copy completion, before retention. `BackupRetentionFailing`: positive increase
in retention failures over a configured lookback; it covers marker-provisioning
failures, tag/delete-lease/deletion errors and a retention-pass panic, but not
reader-busy skips or the best-effort boot marker attempt. Thresholds are
unchanged. Logs contain no payloads.

## CI acceptance and boundaries

Test validation refusals, injected tick/window/DST and shutdown decisions, busy
and early failure accounting, ownership/restart/clone and sidecar failures,
unsafe or over-budget trees, incomplete scans, corrupt/foreign entries, the
`keep_last` floor and clock rollback, and no pruning on failed copies. Real server: drive per-run
ticks and await completion signals without sleeps; allow wall stamp advancement
between runs (inject test time if needed). Verify, restore and scrape actual
copies, check pruning and admin latest-success, and parse rules/check metric names.
Confirm affected test selection on Linux, Windows and macOS before release.
Update Russian operations §8 with config, alert wiring, triage and unchanged restore.
Defer object storage delivery, PITR, dashboards and client changes. Built-in
backup encryption (R2c) was dropped; encryption is the operator's responsibility.
