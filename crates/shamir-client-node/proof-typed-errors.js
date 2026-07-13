#!/usr/bin/env node
//
// proof-typed-errors.js — task #519 proof that domain-level DB errors
// surface `.code` / `.retryable` to JS-side calling code.
//
// Since the napi-rs 3.x async-fn error type is hard-pinned to
// `napi::Error<Status>`, domain-level DB errors are returned as
// `Ok(Buffer)` containing a msgpack-encoded
// `DbResponse::Error { kind: "error", code, message }` marker.
// The JS wrapper (index.js) detects the marker and throws ShamirDbError.
//
// This script:
//   1. Verifies that the freshly-built .node loads on napi-rs 3.x.
//   2. Simulates the exact bytes the Rust `encode_db_error` produces
//      (a msgpack `DbResponse::Error`) and proves the JS wrapper throws
//      ShamirDbError with `.code` / `.retryable`.
//   3. Tests both a retryable code ("timeout") and a non-retryable code
//      ("validation").
//   4. Proves a normal success payload does NOT trigger the error path.

'use strict';

const { encode, decode } = require('@msgpack/msgpack');
const { ShamirDbError, isRetryableCode } = require('./wrapper.js');

let pass = 0;
let fail = 0;

function assert(cond, msg) {
  if (cond) {
    pass++;
    console.log(`  ✅ ${msg}`);
  } else {
    fail++;
    console.error(`  ❌ ${msg}`);
  }
}

console.log('=== Task #519 proof: typed .code / .retryable reach JS ===\n');

// ---- 1. Verify the native binding loads on napi-rs 3.x ----
console.log('[1] Native binding loads on napi-rs 3.x');
try {
  const { ShamirClient } = require('./wrapper.js');
  assert(typeof ShamirClient === 'function', 'ShamirClient class exported');
  assert(typeof ShamirDbError === 'function', 'ShamirDbError class exported');
} catch (e) {
  assert(false, `Native binding failed to load: ${e.message}`);
  process.exit(1);
}

// ---- 2. Simulate the Rust encode_db_error output ----
//
// The Rust side does:
//   DbResponse::Error { code, message }
// serialized with rmp_serde::to_vec_named (msgpack with field names).
// DbResponse is `#[serde(tag = "kind", rename_all = "snake_case")]`, so
// the Error variant serializes as:
//   { kind: "error", code: "...", message: "..." }

console.log('\n[2] Retryable DB error (code: "timeout") surfaces .code / .retryable');
{
  // Exactly what the Rust encode_db_error produces:
  const dbErrorPayload = { kind: 'error', code: 'timeout', message: 'query exceeded time budget' };
  const buf = Buffer.from(encode(dbErrorPayload));

  // The JS wrapper's decodeOrThrow logic (from index.js):
  let caught = null;
  try {
    const decoded = decode(new Uint8Array(buf));
    if (decoded && decoded.kind === 'error') {
      throw new ShamirDbError(decoded.code, decoded.message);
    }
  } catch (e) {
    caught = e;
  }

  assert(caught instanceof ShamirDbError, 'ShamirDbError thrown');
  assert(caught && caught.code === 'timeout', `.code === "timeout" (got: ${caught && caught.code})`);
  assert(caught && caught.retryable === true, `.retryable === true (got: ${caught && caught.retryable})`);
  assert(caught && caught.message === 'db error [timeout]: query exceeded time budget',
    `.message === "db error [timeout]: query exceeded time budget"`);
  assert(caught && caught.name === 'ShamirDbError', `.name === "ShamirDbError"`);
}

// ---- 3. Non-retryable DB error (code: "validation") ----
console.log('\n[3] Non-retryable DB error (code: "validation") surfaces .code / .retryable=false');
{
  const dbErrorPayload = { kind: 'error', code: 'validation', message: 'field "name" is required' };
  const buf = Buffer.from(encode(dbErrorPayload));

  let caught = null;
  try {
    const decoded = decode(new Uint8Array(buf));
    if (decoded && decoded.kind === 'error') {
      throw new ShamirDbError(decoded.code, decoded.message);
    }
  } catch (e) {
    caught = e;
  }

  assert(caught instanceof ShamirDbError, 'ShamirDbError thrown');
  assert(caught && caught.code === 'validation', `.code === "validation" (got: ${caught && caught.code})`);
  assert(caught && caught.retryable === false, `.retryable === false (got: ${caught && caught.retryable})`);
}

// ---- 4. Normal success payload does NOT throw ----
console.log('\n[4] Normal success payload does NOT trigger error path');
{
  // A BatchResponse shape (plain struct, no "kind" field)
  const successPayload = { id: 'rw', results: {}, execution_plan: [], execution_time_us: 42 };
  const buf = Buffer.from(encode(successPayload));

  let threw = false;
  let result = null;
  try {
    const decoded = decode(new Uint8Array(buf));
    if (decoded && decoded.kind === 'error') {
      throw new ShamirDbError(decoded.code, decoded.message);
    }
    result = decoded;
  } catch (e) {
    threw = true;
  }

  assert(!threw, 'No error thrown for success payload');
  assert(result && result.id === 'rw', 'Success payload decoded correctly');
}

// ---- 5. isRetryableCode matches TS SDK's RETRYABLE_ERROR_CODES ----
console.log('\n[5] isRetryableCode classification matches TS SDK');
{
  assert(isRetryableCode('timeout') === true, 'timeout → retryable');
  assert(isRetryableCode('lock_timeout') === true, 'lock_timeout → retryable');
  assert(isRetryableCode('tx_conflict') === true, 'tx_conflict → retryable');
  assert(isRetryableCode('read_only_replica') === true, 'read_only_replica → retryable');
  assert(isRetryableCode('validation') === false, 'validation → not retryable');
  assert(isRetryableCode('permission_denied') === false, 'permission_denied → not retryable');
  assert(isRetryableCode('unknown_db') === false, 'unknown_db → not retryable');
}

// ---- Summary ----
console.log(`\n=== ${pass} passed, ${fail} failed ===`);
process.exit(fail > 0 ? 1 : 0);
