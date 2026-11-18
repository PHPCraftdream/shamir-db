# Application schema migrations

Schema changes ship as an **ordered, checksummed migration set compiled into
your application**, applied through `@shamir/client`'s typed migration API.
This mirrors the Rust client (`shamir_client::schema_migration`): the same
canonical body format, the same SHA-256 checksums, the same journaling,
fencing and reconciliation on the server. There is **no down or rollback
API** and **no automatic retry**. Version 1 admits **DDL only — no DML**.

```ts
import {
  ApplicationMigrationClient,
  ApplicationMigrationSet,
  MigrationError,
  migrationDdl,
  schemaMigration,
} from '@shamir/client';
```

## Author a set

Bodies are composed **only** with the typed migration builders
(`migrationDdl.*`), which reuse the package's existing DDL builders
internally. Hand-built wire objects have no canonical form and no checksum
the server can verify — they are refused locally.

```ts
const set = new ApplicationMigrationSet([
  schemaMigration('add-audit-table', 1, 'add audit table', [
    migrationDdl.createTable('audit'),
  ]),
  schemaMigration('add-audit-archive', 2, 'add audit archive', [
    migrationDdl.createTable('audit_archive'),
    migrationDdl.createIndex('by_audit_at', 'audit', [['at']]),
  ]),
]);
```

`new ApplicationMigrationSet` validates **before anything can be sent**:
ordinals contiguous from exactly 1, unique ids, only the admitted operation
kinds, the same body rules as the Rust model. A gap or malformed body is a
**local** typed error — zero requests. Each body's canonical bytes and
SHA-256 checksum are computed exactly as the Rust client computes them (the
shared golden-vector file is the cross-client contract; the server
**recomputes** the checksum from the submitted typed body, so a client-
reported checksum is never proof). A body is immutable once applied: changing
it changes its checksum — drift, refused before any apply. Correct a released
migration with a new **forward** migration, never by editing the old body.

### Functional indexes in migrations

`migrationDdl.createIndex` admits functional indexes only with
`index_type: 'functional'` and a `functional_op` of exactly
`lower | upper | trim | length` — the built-ins the server's index backend
constructs directly. Any other name is refused locally with a typed
`MigrationError('invalid_set')` ("outside the built-in functional-index
allowlist"), as is `functional_op`/`functional_args` on any other (or
omitted) index type, a functional index without an explicit op, and ANY
supplied `functional_args` — including `[]` — because the server builds the
index expression from the indexed field and the op alone and never reads the
arguments. `length` counts UTF-8 bytes of a string (element count of a
list), not Unicode characters.

## Status, plan, applyNext

The migration client is composed over a connected client and a **target**:
the target carries the **known stable database identity**, supplied
separately from the connection name.

```ts
const migrations = new ApplicationMigrationClient(
  client,
  { database_identity: stableIdentity }, // provision/discovery: your stored identity
  set,
);

const st = await migrations.status('my_app');   // pure read — no journal or DDL writes
const plan = await migrations.plan('my_app');   // server plan for the next ordinal — never writes
await migrations.applyNext('my_app');           // exactly ONE next ordinal
```

* `status` compares the journal with the local set: the **whole applied
  prefix** (every applied entry's id, ordinal and checksum verified against
  the local set), the next local ordinal, drift, gap, a local set shorter
  than the journal, and the unresolved `in_flight` entry when one exists.
  The unresolved entry is exposed **raw**: its journal kind and current
  `.in_flight.status` plus `.server_error` are surfaced as-is — the server
  may return a blanket in-doubt `status` even for a migration that is
  merely `started`. `status` performs **no** classification split into
  in-doubt vs. in-progress and proves nothing about a live owner; that
  split is made by the migration runner when it refuses `plan`/`apply` on
  an unresolved journal or when `resolveUnknownApply` resolves an unknown
  result. These are status observations, distinct from the
  lost-response `unknown_outcome` attempt token and from the explicit
  identical-body resume described below. `status` and `plan` perform **no**
  migration journal or DDL changes.
* `applyNext` runs **one** `status` read first and refuses **before the
  APPLY send** on drift, a gap, a changed checksum of an applied body, an
  unresolved journal **whose id and ordinal do not match the next apply**
  (a matching unresolved id+ordinal is not refused: it sends the explicit
  same-body resume described below), or nothing left to apply. Otherwise it
  sends exactly one ordinal, with the expected predecessor and fence
  generation taken from that status. The `applied` list and the top-level
  `fence_generation` are additive R1c step 3a fields, and their absence is
  judged per case, never guessed around:

  * `applied` is **omitted on a journal with no applied entries — that is
    normal**; it is required only once a nonempty applied prefix is
    reported, and a nonempty prefix reported without the full entries is an
    **unsupported-server error**.
  * A **truly empty baseline** (ordinal 0, no head, no in-flight entry) may
    omit the top-level fence; the client treats it as `fence_generation: 0`.
  * An unresolved **first** migration (no applied entries) may omit the
    top-level fence when the `in_flight` entry itself explicitly reports a
    usable nonzero fence generation — the flight's own fence is used.
  * Any other nonempty journal reported without a durable
    `fence_generation` is an **unsupported-server error**.

  There is **no fallback** to individual DDL calls — an old server that does not know
  the typed operations surfaces a distinct unsupported-server error.
* Permissions: `status` needs `Read` on the database; `plan` and `apply`
  additionally need `Manage`, and apply still passes every underlying
  operation's normal DDL authorization. Server maintenance mode, follower
  read-only state or unavailable durable storage surface as typed errors —
  wait or restore, never retry blindly.

### Identity mismatch

`status` is a **result, not a throw** for a wrong identity: it echoes the
**serving** identity in `reported_target` and reports the mismatch as
`server_error === 'schema_migration_identity_mismatch'`. Update your stored
identity from `reported_target` and retry. Any other failure is a typed
throw — re-throw what you do not handle.

```ts
const st = await migrations.status('my_app');
if (st.server_error === 'schema_migration_identity_mismatch') {
  console.log('serving identity:', st.reported_target.database_identity);
} else if (st.server_error !== undefined) {
  throw new MigrationError(st.server_error, `status failed: ${st.server_error}`);
}
```

## Never retry blindly

A transport error, timeout or close **after an apply was sent** throws a
typed error with `error.code === 'unknown_outcome'` and an `error.attempt`
token. The client does **not** resend — no reconnect, resume or
"retryable"-flag path can re-send an apply. Ask the server what happened:

```ts
try {
  await migrations.applyNext('my_app');
} catch (e) {
  if (e instanceof MigrationError && e.code === 'unknown_outcome' && e.attempt) {
    // The old client may be closed. Obtain a fresh, authenticated client
    // through the app's usual connect/resume path, build a new
    // ApplicationMigrationClient over the SAME target and SAME set, then
    // reconcile.
    const reconnectedClient = await app.connectOrResume();
    const resumed = new ApplicationMigrationClient(
      reconnectedClient,
      { database_identity: stableIdentity },
      set,
    );
    const outcome = await resumed.resolveUnknownApply(e.attempt);
    // outcome.kind: 'applied'        — proven applied; continue with the next ordinal
    //               'ready_to_apply' — proven absent; calling applyNext again is safe
    //               anything else    — evidence is UNPROVEN; reconcile out of band
    //               ('evidence_unavailable' and 'unmatched_unresolved' carry
    //               the journal ordinal)
  } else if (e instanceof MigrationError && e.code === 'schema_migration_in_doubt') {
    // an unresolved journal. The server may refuse an APPLY before it is
    // sent, but it may also send this refusal after the APPLY reached it —
    // the absence of an `unknown_outcome` attempt token is not evidence
    // that nothing was sent. If a refusal surfaces here, reconcile: ask
    // the server (via status) what the journal holds rather than assuming
    // the ordinal is untouched. An in_doubt/failed event surfaces as
    // schema_migration_in_doubt with the server's evidence as e.reason /
    // e.operationIndex; other unresolved kinds surface as
    // schema_migration_in_progress.
  } else {
    throw e; // unrelated failures are re-thrown, never swallowed
  }
}
```

`resolveUnknownApply(attempt)` first validates the token against the set
(exact id / ordinal / canonical checksum — a mismatch fails closed) and
then runs ONE read-only `status` for the attempt's database and target,
resolving `{ kind: 'applied' | 'ready_to_apply' | 'evidence_unavailable' |
'unmatched_unresolved' }`. Only `applied` and `ready_to_apply` are
**proven** verdicts; the other kinds mean the evidence is unproven —
reconcile out of band, do not resend. If the journal has an unresolved
`in_doubt` or `failed` event matching the attempt, `resolveUnknownApply`
throws a `MigrationError` with code `schema_migration_in_doubt` carrying
`reason` and `operationIndex`; an unresolved event of any other kind
surfaces as `schema_migration_in_progress` — that migration is still
running and holds the fence. If instead the journal holds an unresolved
flight for a **different** migration (no id/ordinal match) and there is
no server error, the existing `unmatched_unresolved` verdict stands.

The server-side resume of an in-doubt apply accepts the **identical stored
body**, reachable through `applyNext` again on the SAME set. This holds
only because the set is compiled into your application: the bytes are
identical only if you build against the **exact same set** — if your build
changed any body, its checksum changed and the resume is refused (drift).
This is the operator-approved path; it is not an automatic retry.

## Forward-only

There is no down/rollback migration API, client- or server-side. To correct a
mistake, ship a new forward migration. Restoring an external backup is an
operator procedure outside this API.

## Known server limitation

An HMAC-gated migration entry can be canonicalized and accepted, but the
server currently **refuses to execute that step** (the engine cannot verify
nested session tags on migration bodies). The failure surfaces as a typed
server error; it is not a client-side format problem. Do not work around it
with hand-built bodies.

## Node binding note

`crates/client/shamir-client-node` is a **low-level transport binding**. Migration authoring uses the typed
TypeScript API above — never hand-built MessagePack, and the Node binding
defines **no third canonicalization**. The canonical bytes and checksums come
solely from `@shamir/client`.
