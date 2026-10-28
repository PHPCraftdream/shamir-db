import type { Platform } from '../support/platform.js';
import type { BatchResponse } from '../types/write/batch.js';
import type { WireValue } from '../types/write/write.js';
import type { HmacSigner } from '../types/access/admin.js';
import type { KdfParams } from '../protocol/scram.js';
import { ShamirTimeoutError } from '../support/errors.js';
import {
  buildAuthMessageCp,
  computeClientProof,
  TRANSPORT_KIND_WS,
  BINDING_MODE_TLS_NO_EXPORT,
} from '../protocol/scram.js';
import { canonicalDbRequest } from '../protocol/hmac.js';
import {
  setSuperuser as buildSetSuperuserOp,
  setReplicator as buildSetReplicatorOp,
} from '../builders/admin/index.js';
import { listUsers } from '../builders/ddl/index.js';
import { InternerCacheRegistry } from '../support/field-map.js';
import type { InternerDelta } from '../support/field-map.js';
import {
  collectFieldNames,
  encodeRecordIdMsgpack,
  qvHasFnMarker,
  deinternResponse,
} from '../support/interner-ops.js';

/** Result of {@link ShamirClient.txBegin} (`DbResponse::TxOpened`). */
export interface TxOpened {
  /** Opaque handle for subsequent txExecute/txCommit/txRollback. */
  tx_handle: number;
  /** MVCC version the transaction's snapshot reads at. */
  snapshot_version: number | bigint;
  /** Effective isolation (`"snapshot"` | `"serializable"`). */
  isolation: string;
}

/** Result of {@link ShamirClient.createScramUser} (`DbResponse::UserCreated`). */
export interface ScramUserCreated {
  /** Echoed user name (post-normalisation). */
  name: string;
  /** Stable 16-byte user id assigned by the directory. */
  user_id: Uint8Array;
}

/** Result of {@link ShamirClient.setSuperuser} (`DbResponse::SuperuserSet`). */
export interface SuperuserSet {
  /** Echoed target username. */
  user: string;
  /** Echoed requested state (`true` = granted, `false` = revoked). */
  on: boolean;
}

/** Result of {@link ShamirClient.setReplicator} (`DbResponse::ReplicatorSet`). */
export interface ReplicatorSet {
  /** Echoed target username. */
  user: string;
  /** Echoed requested state (`true` = granted, `false` = revoked). */
  on: boolean;
}

/**
 * Race an in-flight `openSocket` promise against a connect deadline (Finding
 * 2.2). If the socket resolves after the timeout already fired, it is closed so
 * no descriptor leaks. `timeoutMs <= 0` disables the bound (awaits the socket
 * directly).
 */
export async function withConnectTimeout(
  socketPromise: Promise<import('../support/platform.js').Socket>,
  timeoutMs: number,
): Promise<import('../support/platform.js').Socket> {
  if (timeoutMs <= 0) return socketPromise;

  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(
      () => reject(new ShamirTimeoutError('connect', timeoutMs)),
      timeoutMs,
    );
    (timer as { unref?: () => void }).unref?.();
  });

  try {
    return await Promise.race([socketPromise, timeout]);
  } catch (e) {
    // If the socket eventually opens after we timed out, close it to avoid a
    // leaked connection.
    void socketPromise.then((s) => void s.close()).catch(() => {});
    throw e;
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}

/** Extract the repo string from a table-ref wire value. */
export function tableRefToRepo(tableRef: unknown): string {
  if (Array.isArray(tableRef) && tableRef.length >= 1) {
    return String(tableRef[0]);
  }
  // Bare string = default repo "main".
  return 'main';
}

/**
 * Extract the repo from any data-op entry (INSERT/UPDATE/SET/DELETE/SELECT).
 * Returns `undefined` for non-data ops.
 */
export function extractRepo(entry: Record<string, unknown>): string | undefined {
  if ('insert_into' in entry) return tableRefToRepo(entry['insert_into']);
  if ('update' in entry) return tableRefToRepo(entry['update']);
  if ('set' in entry && !('key' in entry)) return undefined; // not a SetOp
  if ('set' in entry && 'key' in entry) return tableRefToRepo(entry['set']);
  if ('delete_from' in entry) return tableRefToRepo(entry['delete_from']);
  if ('from' in entry) {
    // ReadQuery: `from` is a TableRefWire — either a bare table name
    // (default repo "main"), a `[repo, table]` pair, OR a legacy form
    // `{ from: table, repo?: string }`. Resolve the repo from whichever
    // form is present so the interner cache is keyed correctly.
    const fromVal = entry['from'];
    if (Array.isArray(fromVal)) {
      return tableRefToRepo(fromVal);
    }
    const repo = entry['repo'];
    return typeof repo === 'string' ? repo : 'main';
  }
  return undefined;
}

/**
 * Detect if any query entry contains query-ref (`$query_ref`) or sub-batch
 * (`batch`) dependencies. When present, the server resolves intermediate
 * results by name-keyed field paths — requesting `result_encoding = 'id'`
 * would break those resolutions.
 */
export function batchHasRefs(
  queries: Record<string, Record<string, unknown>>,
): boolean {
  for (const entry of Object.values(queries)) {
    if ('batch' in entry) return true;
    if (hasQueryRef(entry)) return true;
  }
  return false;
}

/** Recursively check if a value contains `{ "$query": ... }` or `{ "$param": ... }`. */
function hasQueryRef(val: unknown): boolean {
  if (val === null || val === undefined) return false;
  if (typeof val !== 'object') return false;
  if (Array.isArray(val)) return val.some(hasQueryRef);
  const obj = val as Record<string, unknown>;
  if ('$query' in obj || '$param' in obj) return true;
  return Object.values(obj).some(hasQueryRef);
}

/**
 * Create a SCRAM-authenticatable user (one that can log in over the wire).
 * Requires a superuser session AND an HMAC confirmation tag — the tag is
 * unconditional (every call signs it), mirroring `setSuperuser`'s gate
 * (task #604). The server runs Argon2id with its KDF defaults and writes
 * the durable user record. `roles: ["superuser"]` grants admin powers;
 * other strings are app-defined.
 */
export async function createScramUser(
  name: string,
  password: string,
  roles: string[] = [],
  sendDbRequest: (req: object) => Promise<Record<string, unknown>>,
  hmacTagHex: (canonical: Uint8Array) => string,
): Promise<ScramUserCreated> {
  const canonical = canonicalDbRequest({ op: 'create_scram_user', name, password, roles });
  const r = await sendDbRequest({
    op: 'create_scram_user',
    name,
    password,
    roles,
    hmac: hmacTagHex(canonical),
  });
  if (r.kind === 'user_created') {
    return { name: r.name as string, user_id: r.user_id as Uint8Array };
  }
  throw new Error(
    `unexpected DbResponse kind for create_scram_user: ${r.kind}`,
  );
}

/**
 * Grant or revoke superuser status on an existing SCRAM-directory user
 * (top-level `DbRequest::SetSuperuser`, NOT a `BatchOp`). Requires an
 * already-superuser session AND an HMAC confirmation tag — the tag is
 * unconditional (every call signs it via this session's HMAC key).
 * Mirrors `createScramUser`'s top-level shape; the signed wire op is
 * built by the `setSuperuser` admin builder.
 */
export async function setSuperuser(
  user: string,
  on: boolean,
  sendDbRequest: (req: object) => Promise<Record<string, unknown>>,
  hmacTagHex: (canonical: Uint8Array) => string,
): Promise<SuperuserSet> {
  const r = await sendDbRequest(
    buildSetSuperuserOp({ hmacTagHex } as HmacSigner, user, on),
  );
  if (r.kind === 'superuser_set') {
    return { user: r.user as string, on: r.on as boolean };
  }
  throw new Error(
    `unexpected DbResponse kind for set_superuser: ${r.kind}`,
  );
}

/**
 * Grant or revoke replicator status on an existing SCRAM-directory user
 * (top-level `DbRequest::SetReplicator`, NOT a `BatchOp`, task #621).
 * Requires an already-superuser session AND an HMAC confirmation tag —
 * the tag is unconditional (every call signs it via this session's HMAC
 * key). Mirrors `setSuperuser`'s top-level shape and gate exactly; the
 * signed wire op is built by the `setReplicator` admin builder.
 */
export async function setReplicator(
  user: string,
  on: boolean,
  sendDbRequest: (req: object) => Promise<Record<string, unknown>>,
  hmacTagHex: (canonical: Uint8Array) => string,
): Promise<ReplicatorSet> {
  const r = await sendDbRequest(
    buildSetReplicatorOp({ hmacTagHex } as HmacSigner, user, on),
  );
  if (r.kind === 'replicator_set') {
    return { user: r.user as string, on: r.on as boolean };
  }
  throw new Error(
    `unexpected DbResponse kind for set_replicator: ${r.kind}`,
  );
}

/**
 * Change the password for the currently-authenticated user (spec §12.5).
 * Two-step flow over the active session: challenge (server issues fresh
 * nonce + echoes current salt/KDF), then verify (client submits SCRAM
 * proof-of-old-password + pre-derived new credentials). On success every
 * other live session for this user is killed server-side.
 *
 * NOT available on resumed sessions — the client needs the normalized
 * username from the original SCRAM handshake to build `auth_message_cp`.
 */
export async function changePassword(
  oldPassword: string,
  newPassword: string,
  platform: Platform,
  sessionId: Uint8Array,
  normalizedUser: string | undefined,
  sendDbRequest: (req: object) => Promise<Record<string, unknown>>,
): Promise<void> {
  if (normalizedUser === undefined) {
    throw new Error(
      'changePassword requires a full connect() session (not resume)',
    );
  }

  // ── Step 1: challenge ──────────────────────────────────────────────
  const clientNonceCp = platform.randomBytes(32);
  const ch = await sendDbRequest({
    op: 'change_password_challenge',
    client_nonce_cp: clientNonceCp,
  });
  if (ch.kind !== 'change_password_challenge') {
    throw new Error(
      `unexpected DbResponse kind for change_password_challenge: ${ch.kind}`,
    );
  }

  const serverNonceCp = ch.server_nonce_cp as Uint8Array;
  const salt = ch.salt as Uint8Array;
  const kdf: KdfParams = {
    memoryKb: ch.kdf_memory_kb as number,
    time: ch.kdf_time as number,
    parallelism: ch.kdf_parallelism as number,
    argon2Version: ch.kdf_argon2_version as number,
  };

  // ── Step 2: build auth_message_cp + derive proofs ──────────────────
  // Channel binding is always 32 zero bytes on the WS/no-exporter path —
  // the server snapshotted the same value at session creation, so they
  // MUST match bit-for-bit or the server's recomputation fails.
  const authMessageCp = buildAuthMessageCp({
    username: normalizedUser,
    sessionId,
    clientNonceCp,
    serverNonceCp,
    salt,
    kdf,
    transportKind: TRANSPORT_KIND_WS,
    bindingMode: BINDING_MODE_TLS_NO_EXPORT,
    channelBindingAtAuth: new Uint8Array(32),
  });

  // Old-password proof: full SCRAM derivation against current salt/KDF.
  const oldProof = await computeClientProof(
    platform,
    oldPassword,
    salt,
    kdf,
    authMessageCp,
  );

  // New credential material: fresh salt, same KDF params (server ignores
  // the client's KDF choice for new material per ChangePwApply). We only
  // need .storedKey and .serverKey — the dummy authMessage is required
  // by computeClientProof but its proof output is discarded.
  const newSalt = platform.randomBytes(16);
  const newCreds = await computeClientProof(
    platform,
    newPassword,
    newSalt,
    kdf,
    new Uint8Array(1),
  );

  // ── Step 3: verify ─────────────────────────────────────────────────
  const r = await sendDbRequest({
    op: 'change_password_verify',
    client_proof_old: oldProof.clientProof,
    new_salt: newSalt,
    new_stored_key: newCreds.storedKey,
    new_server_key: newCreds.serverKey,
  });
  if (r.kind !== 'change_password_ok') {
    throw new Error(
      `unexpected DbResponse kind for change_password_verify: ${r.kind}`,
    );
  }
}

/**
 * Resolve a username to its real server-assigned `principal64` — a stable
 * 64-bit projection of the random 128-bit `user_id` the directory mints at
 * user creation (task #548). Backed by `ListOp::Users` against the durable
 * principal directory; this is a real network round-trip, NOT a
 * client-side hash.
 *
 * Required by callers of `chown` / `addGroupMember` / `removeGroupMember`,
 * whose wire parameter is a `u64` principal id (never a username string).
 * Throws `Error("no such user: <username>")` when the name is unknown.
 *
 * Uses the `"default"` db context like other Root-scoped admin convenience
 * calls on this client — `ListOp::Users` is authorised at
 * `ResourcePath::Root` (admin_list.rs:77-80) and is NOT database-scoped
 * server-side, so any db context the caller can traverse works; `"default"`
 * matches the established pattern in `createUser`/`grantRole` callers.
 */
export async function resolvePrincipal(
  username: string,
  execute: (db: string, batch: object) => Promise<BatchResponse>,
): Promise<bigint> {
  const resp = await execute('default', {
    id: 'resolve-principal',
    queries: { u: listUsers() },
  });
  const rec = resp.results.u.records[0] as
    | Record<string, unknown>
    | undefined;
  const users =
    (rec?.users as Array<{
      name: string;
      principal64: number | bigint;
    }>) ?? [];
  const entry = users.find((u) => u.name === username);
  if (entry === undefined) {
    throw new Error(`no such user: ${username}`);
  }
  return BigInt(entry.principal64);
}

/**
 * Merge `response.interner_delta` into the per-repo FieldMap caches for
 * `db`. Called after every successful `execute` response.
 *
 * Monotonic: the FieldMap's `applyDelta` rejects stale epochs, so
 * out-of-order or duplicate deltas are harmless.
 */
export function mergeInternerDelta(
  internerCache: InternerCacheRegistry,
  db: string,
  response: BatchResponse,
): void {
  const delta = response.interner_delta;
  if (delta === undefined || delta === null) return;
  for (const [repo, repoDelta] of Object.entries(delta)) {
    const fm = internerCache.getOrCreate(db, repo);
    const normalised: InternerDelta = {
      epoch: BigInt(repoDelta.epoch),
      entries: repoDelta.entries.map(([id, name]) => [BigInt(id), name]),
    };
    fm.applyDelta(normalised);
  }
}

/**
 * Register field `names` against the server's interner for `(db, repo)`
 * and merge the returned `(name, id)` mappings into the local cache.
 *
 * Returns the resolved `{ name → id }` map in input order. Idempotent:
 * the server returns existing ids for already-interned names.
 *
 * Short-circuits when every name is already cached (no roundtrip).
 */
export async function touchFields(
  internerCache: InternerCacheRegistry,
  execute: (db: string, batch: object) => Promise<BatchResponse>,
  db: string,
  repo: string,
  names: string[],
): Promise<Map<string, bigint>> {
  const fm = internerCache.getOrCreate(db, repo);
  const missing = fm.missingNames(names);

  if (missing.length > 0) {
    // Send an interner_touch op for the unknown names.
    const alias = '_ic_touch';
    const batch = {
      id: '_ic_touch',
      queries: {
        [alias]: { interner_touch: repo, names: missing },
      },
    };
    const resp = await execute(db, batch);
    // Parse the touch payload: server returns
    // `{ "interner_touch": "<repo>", "epoch": <u64>, "mappings": [[name, id], ...] }`
    const result = resp.results[alias];
    if (result !== undefined && result.records.length > 0) {
      const rec = result.records[0] as Record<string, unknown>;
      const epoch = rec['epoch'];
      const mappings = rec['mappings'];
      if (Array.isArray(mappings)) {
        for (const pair of mappings as unknown[]) {
          if (Array.isArray(pair) && pair.length === 2) {
            const name = pair[0] as string;
            const id = BigInt(pair[1] as number | bigint);
            fm.insertEntry(name, id);
          }
        }
      }
      if (epoch !== undefined && epoch !== null) {
        const bigEpoch = BigInt(epoch as number | bigint);
        if (bigEpoch > fm.epoch()) {
          // applyDelta with just the epoch bump
          fm.applyDelta({ epoch: bigEpoch, entries: [] });
        }
      }
    }
  }

  // Return resolved map for all requested names (both cached + freshly minted).
  const result = new Map<string, bigint>();
  for (const name of names) {
    const id = fm.getId(name);
    if (id !== undefined) {
      result.set(name, id);
    }
  }
  return result;
}

/**
 * Execute a batch with automatic client-side field interning (Stage 5-wire).
 *
 * 1. Collects field names from INSERT/SET/UPDATE ops, grouped by repo.
 * 2. Touches unknown field names per repo (populates the FieldMap cache).
 * 3. On v2+ servers: encodes fully-literal INSERT records into id-keyed
 *    msgpack (`records_idmsgpack`); removes them from `values`. Records
 *    with `$fn` markers stay on `values` (server-side eval). Sets
 *    `result_encoding = "id"` so the server returns id-keyed rows.
 * 4. Calls `execute()`.
 * 5. De-interns any id-keyed result rows back to name-keyed objects.
 *
 * Mirrors the Rust `Client::execute_with_touch`.
 */
export async function executeWithTouch(
  internerCache: InternerCacheRegistry,
  serverQueryVersion: number,
  execute: (db: string, batch: object) => Promise<BatchResponse>,
  touchFieldsFn: (db: string, repo: string, names: string[]) => Promise<Map<string, bigint>>,
  db: string,
  batch: object,
): Promise<BatchResponse> {
  const batchObj = batch as Record<string, unknown>;
  const queries = batchObj['queries'] as
    | Record<string, Record<string, unknown>>
    | undefined;
  if (!queries) {
    return execute(db, batch);
  }

  // Collect field names per repo (write ops only).
  const perRepo = new Map<string, string[]>();
  // Collect ALL repos referenced by any data op (read + write) for
  // de-interning the response.
  const allRepos = new Set<string>();

  for (const entry of Object.values(queries)) {
    collectFieldNames(entry, perRepo);
    // Extract repo from table-ref for all data ops.
    const repo = extractRepo(entry);
    if (repo !== undefined) {
      allRepos.add(repo);
    }
  }

  // Touch each repo's unknown fields. If touch fails (e.g. non-admin user
  // lacks permission for interner_touch), fall back to plain execute — the
  // id-on-wire path requires a warm cache so we skip it entirely.
  let touchOk = true;
  for (const [repo, names] of perRepo) {
    const unique = [...new Set(names)].sort();
    if (unique.length > 0) {
      try {
        await touchFieldsFn(db, repo, unique);
      } catch {
        touchOk = false;
        break;
      }
    }
  }

  if (!touchOk) {
    // Touch failed — fall back to plain execute (name-keyed, no id-on-wire).
    return execute(db, batch);
  }

  // Ensure a FieldMap exists for every referenced repo. Explicitly attach
  // interner_epochs (including epoch-0 cold repos) so the server returns
  // deltas even for repos the client has never seen before.
  const wireEpochs: Record<string, number> = {};
  for (const repo of allRepos) {
    const fm = internerCache.getOrCreate(db, repo);
    wireEpochs[repo] = Number(fm.epoch());
  }
  // Also include any already-warm repos from prior requests.
  const cached = internerCache.allEpochs(db);
  for (const [repo, e] of Object.entries(cached)) {
    if (!(repo in wireEpochs)) {
      wireEpochs[repo] = Number(e);
    }
  }
  if (Object.keys(wireEpochs).length > 0) {
    batchObj['interner_epochs'] = wireEpochs;
  }

  // v2 id-keyed write path.
  if (serverQueryVersion >= 2) {
    for (const entry of Object.values(queries)) {
      if ('insert_into' in entry && Array.isArray(entry['values'])) {
        const tableRef = entry['insert_into'];
        const repo = tableRefToRepo(tableRef);
        const fm = internerCache.getOrCreate(db, repo);
        const values = entry['values'] as WireValue[];
        const remaining: WireValue[] = [];
        const idmsgpackList: Uint8Array[] = [];

        for (const qv of values) {
          if (qvHasFnMarker(qv)) {
            remaining.push(qv);
          } else {
            const bytes = encodeRecordIdMsgpack(
              qv as Record<string, WireValue>,
              fm,
            );
            idmsgpackList.push(bytes);
          }
        }

        entry['values'] = remaining;
        if (idmsgpackList.length > 0) {
          entry['records_idmsgpack'] = idmsgpackList;
        }
      }
    }
    // Request id-keyed result rows only when no query-ref or sub-batch
    // dependencies exist — those rely on server-side intermediate results
    // staying name-keyed for path resolution ($param, queryRef).
    if (!batchHasRefs(queries)) {
      batchObj['result_encoding'] = 'id';
    }
  }

  const response = await execute(db, batch);

  // De-intern id-keyed result rows (no-op when result_encoding was not set).
  const repos = [...allRepos];
  return deinternResponse(internerCache, db, response, repos);
}
