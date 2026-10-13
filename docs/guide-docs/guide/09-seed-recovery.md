בְּשֵׁם יהוה הָרַחֲמָן וְהַחַנּוּן

# Этаж 9 — Offline seed recovery (R2-lite)

A follower stopped with `resync_required` (or a brand-new follower that
cannot start from version 0 against a gapped leader) is recovered by
**offline seeding**: export the leader's logical state from a STOPPED
server, install it into a NEW follower data directory, and Resume from the
seed version + 1. This is the only supported recovery path today. It is
"R2-lite": an offline, operator-driven, whole-repository logical seed —
NOT an online snapshot, NOT automatic reseeding, NOT promotion/failover,
and NOT full-server disaster recovery.

Read this together with:

* [`../architecture/SEED_FORMAT.md`](../architecture/SEED_FORMAT.md) — the
  artifact format specification.
* [`08-interconnect.md`](./08-interconnect.md) — replication semantics,
  gap certification, `resync_required`.
* [`../KNOWN_LIMITATIONS.md`](../KNOWN_LIMITATIONS.md) — the gap and
  durability caveats summarized below.

## 1. When to use it

Use this runbook when ALL of these hold:

* A follower subscription is `resync_required` (a certified recovery gap
  `[J+1, H]` exists on the leader at or above the follower bookmark), or
  you are bootstrapping a new follower after the leader lost its journal
  tail to a crash.
* You can STOP the leader for the export window (minutes for typical
  repository sizes). Export refuses to run against a live server.
* The repository is a normal user `(db, repo)`. System repositories are
  refused.

**What it does NOT recover:** writes already lost from the leader's own
recoverable state (the seed is taken from what the leader has AFTER crash
recovery — nothing resurreces a lost tail); DDL made after the export
(schema is not replicated — see §8); secondary indexes (excluded, see
§8); anything outside the single exported repository.

## 2. Preconditions

* Unix. The seed export/install path requires directory fsync and refuses
  to run on other platforms ("durable seed installation requires Unix").
* The leader server process is STOPPED and will not auto-restart during
  the export. The export takes an exclusive maintenance lock over the
  data directory; a running server (or another offline tool) holds it and
  the export refuses.
* The trusted manifest digest channel: SHA-256 inside the artifact proves
  integrity, not provenance. Receive the digest out-of-band (e.g. from
  the export operator over a trusted channel) and pass it with
  `--manifest-digest` on every later command.
* Enough disk for the artifact (roughly the live logical size of the
  repository) and for the new follower data directory.
* The artifact is SENSITIVE DATA: it contains every live record of the
  repository. Store it with the same protections as a backup of the data
  itself, and delete it when the recovery is done.

## 3. Step-by-step

Placeholders: `<config>` is the server `.ktav` config whose `data_dir`
points at the relevant data directory; `app` / `main` are the source
database and repository names (they must be identical on the follower in
this release); `DIGEST` is the trusted manifest SHA-256; `ENDPOINT` is
the leader address the follower will pull from, e.g. `tcp://10.0.0.5:7331`.

```sh
# 1. STOP the leader server (systemctl stop / service stop). Verify it is
#    down; the export takes the maintenance lock and refuses otherwise.

# 2. Export the repository into a new artifact directory.
shamir-server --config <leader-config> replication-seed-export \
    --db app --repo main --out /secure/seed-app-main
# expected stdout:
#   manifest sha256: <64 hex chars>       <- this is DIGEST

# 3. (Optional but recommended) Verify without opening any repository —
#    safe to run anywhere, including on the follower host.
shamir-server --config <follower-config> replication-seed-verify \
    /secure/seed-app-main --manifest-digest DIGEST
# expected stdout:
#   manifest sha256: <DIGEST>             <- must match DIGEST exactly

# 4. Install into a NEW, previously nonexistent follower data directory.
#    This creates the repository image, the paused receipt-bound
#    subscription `app-main`, the profile `seedprof`, and the durable
#    upstream bookmark at the seed version V. It never activates
#    replication.
shamir-server --config <follower-config> replication-seed-install \
    --db app --repo main \
    --artifact /secure/seed-app-main --manifest-digest DIGEST \
    --subscription app-main --upstream ENDPOINT \
    --publication pub --profile seedprof
# expected stdout:
#   installed manifest sha256: <DIGEST>

# 5. Start the follower server. The subscription stays PAUSED; no pull
#    loop runs. Check status (see §5): installation_state must be
#    `complete` and subscription_state `paused`.

# 6. Resume — an explicit, VALIDATED operator action (root `Manage`):
#    before activating, the server contacts the upstream, validates the
#    repository lineage, history epoch, schema digest, publication scope,
#    and confirms no new source gap sits above the bookmark. Only then
#    does the loop start, pulling from V+1.
```

Resume is issued as an admin operation (`ALTER SUBSCRIPTION app-main
RESUME` through the admin/query interface with root `Manage`), the same
way ordinary subscriptions are managed (see `08-interconnect.md`).

### 3.1 The identity fence is universal (S5c-X PR7)

The identity fence is NOT limited to seeded subscriptions. Every
subscription — seeded or ordinary — records the leader's
`(source_lineage_id, source_history_epoch)` at its first successful
Hello (or Pull if Hello failed) in the SAME durable `__tx__` bookmark
value (one atomic `set` + `flush`). Every advance preserves the pinned
identity; this is not a separate record carrying a stale cursor. Every
later Hello and Pull is compared with that record BEFORE
any event of the response is applied. A different lineage or epoch stops
the loop terminally: nothing from that response is applied, the
subscription shows `seed_wrong_identity` / `seed_wrong_history_epoch` with
structured `recorded_identity` and `observed_identity` fields in
per-repository `ReplicationStatus`, and the
loop is not restarted automatically. Subscriptions created before this
change adopt the first observed identity exactly once (one WARN line in
the follower log) and then fence like any other. Leaders on `in_memory`
and `hybrid` engines keep `__tx__` in RAM only, so every process start
mints a NEW lineage: a restarted ephemeral leader stops its followers by
design, while durable (`fjall`) leaders keep their identity across
restarts. Seeded subscriptions keep their stricter SEED-4 checks; the
identity and receipt schema checks form one decision, with one typed error.
Identity mismatches increment `shamir_repl_identity_mismatch_total` for
seeded and ordinary subscriptions. Missing identity fails closed even on
first contact (`seed_missing_identity`). There is no production disable
switch. Fresh ordinary bookmarks use the same one-WARN adoption rule.

## 4. Failure boundaries during install

Durability order (SEED-3): data + schema + interner + counters + local
floor → upstream bookmark → `Complete` receipt → registry publication →
paused subscription. An interruption at any point leaves the destination
UNUSABLE-but-HARMLESS: the image is never registered or served, and a
retry of the SAME artifact (same digest, same identity) finishes the job
without replaying rows. Abandoned staging attempts are preserved for
inspection and cleaned up by the retry.

| Failure after… | On disk | Operator action |
|---|---|---|
| artifact verification | nothing created | fix digest/artifact, rerun install |
| data/schema/bindings/counter import (partial) | staging attempt only (`*-attempt-*`), nothing published, bookmark 0 | rerun the SAME install command; retry starts a fresh attempt |
| image durable, bookmark NOT yet durable | staging image complete but unbookmarked | rerun the SAME install command |
| bookmark durable, `Complete` NOT written | image looks usable but is refused (`seed_incomplete_install`) | rerun the SAME install command |
| `Complete` written, repo NOT published | receipt on disk, repository absent | rerun: exact retry verifies the receipt and finishes publication without replay |
| published, subscription NOT created | repository served, subscription absent | rerun: exact retry creates the paused subscription only |
| everything (install succeeded) | complete + paused | move on to Resume |

Rerunning after a refusal is always safe: every conflict (different
artifact, existing ordinary repository, wrong names/scope) is refused
with a typed code and NO state change.

## 5. Status and refusal codes

Per-repository status (`ReplicationStatus`; `ListSubscriptions` lists
catalogue rows) reports, for seeded
repositories: `installation_state`, `artifact_digest`, `source_db`,
`source_repo`, `source_lineage_id`, `source_history_epoch`,
`seed_source_version`, `upstream_bookmark`, `next_pull_version`,
`local_baseline_version`, `local_consistent_version`, `schema_digest`,
`certified_gap_first`, `certified_gap_through`, `subscription_state`,
`last_terminal_reason`. Every repository also exposes `recorded_identity`
and `observed_identity`: objects with `lineage_id` (16-byte array) and
`history_epoch`, or null before contact.

Note the two version domains: `upstream_bookmark`/`next_pull_version`/
`seed_source_version` count LEADER versions; `local_baseline_version`/
`local_consistent_version` count FOLLOWER-local versions (the seed lands
at local baseline 1; the first applied upstream event becomes local 2).
They are deliberately never compared.

Refusal codes (they appear in admin errors, terminal
`last_terminal_reason`, and the install CLI's non-zero exit output):

| Code | Meaning | Operator action |
|---|---|---|
| `seed_incomplete_install` | the destination image has no durable Complete receipt | rerun the exact install command (§4) |
| `seed_ordinary_repository` | destination is an existing ordinary (non-seed) repository | seed only into a NEW empty data directory |
| `seed_wrong_scope` | subscription/profile scope does not match the receipt, or the publication covers a different scope | fix the publication/profile to cover exactly the receipt's `(db, repo)` |
| `seed_wrong_publication` | the upstream does not advertise the subscribed publication | create/restore the publication on the leader |
| `seed_missing_identity` | the upstream did not advertise lineage/epoch (older peer or repo missing) | upgrade/repair the leader; do NOT bypass |
| `seed_wrong_identity` | the leader repository was RECREATED or regenerated (new lineage) — checked on EVERY subscription, seeded or not (§3.1) | the old replication relationship is dead: re-export and reseed into a new repository |
| `seed_wrong_history_epoch` | leader history was restored/rewound (epoch advanced) — checked on EVERY subscription, seeded or not (§3.1) | same as above: reseed |
| `seed_wrong_schema` | leader schema diverged after the export (DDL is not replicated) | re-export and reseed (§8) |
| `seed_horizon_above_bookmark` | a NEW crash certified a gap above the follower bookmark | repeat this runbook: export again, seed into ANOTHER new repository |
| `seed_source_regression` | leader settled version regressed below the seed position | leader inconsistency — investigate before trusting it |
| `seed_bookmark_overflow` | bookmark+1 would overflow u64 | practically unreachable; file a bug |
| `seed_competing_owner` | another subscription already owns a pull stream for this repository | keep exactly one pull subscription per repository |
| `seed_upstream_unavailable` | upstream unreachable during Resume validation | check network/leader, retry Resume |
| `reopen_required` | the follower repository needs a cold-reopen certification pass first | restart the follower server, then Resume again |

## 6. Process crash vs power loss — honesty

The CI certification (`crates/shamir-server/tests/seed_recovery_e2e/`)
kills the leader PROCESS (SIGKILL) and proves: crash-recovery
certification, `resync_required` with no payload applied, export→install→
Resume→convergence, refusal codes, and restart-survival at every durable
boundary. That establishes PROCESS-crash behaviour only. Data still in
the OS page cache at process death survives a process kill but not
necessarily a machine power cut; these tests do not — and cannot — prove
power-loss guarantees. The ORDERING claims (history durable before
bookmark durable before Complete before publication) are proven by the
deterministic boundary-injection tests of SEED-3
(`crates/shamir-server/src/replication_seed/install/tests/`), not by
killing processes.

## 7. Permissions

* Export / verify / install authorization is OS filesystem authority:
  whoever can read the data directory and the artifact can do this. There
  is no fabricated "replicator login" for offline tools.
* Online `Resume` requires root `Manage`; status reading requires root
  `List`; the replication transport still requires a `replicator`-role
  (or superuser) session plus per-repository read access on the leader.
* The artifact and the follower data directory are sensitive: protect
  them like the production data they contain.

## 8. Limitations (exactly what R2-lite is)

* **No DDL replication.** Schema is captured once in the artifact; if the
  leader schema changes afterwards, Resume refuses with
  `seed_wrong_schema`. Keep DDL frozen for the replication relationship,
  or reseed.
* **No secondary indexes on the follower.** Index definitions are
  exported as explicitly-excluded metadata; the follower runs with
  indexes disabled.
* **Whole-repository scope only.** One artifact = one `(db, repo)`; the
  subscription is a single whole-repository pull.
* **Unix-only publication.** Install requires directory fsync and refuses
  off-Unix.
* **Not** an online snapshot, **not** automatic reseeding, **not**
  leader promotion, **not** multi-master, **not** full-server disaster
  recovery (no credentials, audit, TLS material, or system stores are
  seeded).
* **Writes lost by the leader are not recovered** — the seed is the
  leader's post-recovery logical state at a settled version V, and V is
  required to cover the certified gap horizon.
