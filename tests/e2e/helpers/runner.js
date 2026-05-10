/**
 * Tiny test runner.
 *
 * Each test file exports `module.exports = async function(ctx) { ... }`.
 * `ctx` carries a connected `client`, a unique-per-file `db` name (the
 * test creates the db itself if it needs one), and the assertion helpers
 * below.
 *
 * Inside a file:
 *
 *   ctx.test('reads after a single insert', async () => {
 *     ...
 *   });
 *
 * The runner collects every `test(...)` call, runs them sequentially,
 * counts pass/fail, prints a summary, and resolves with a non-zero exit
 * code if anything failed.
 */

'use strict';

const path = require('path');
const fs = require('fs');

class AssertionError extends Error {
  constructor(message) {
    super(message);
    this.name = 'AssertionError';
  }
}

function assert(cond, message) {
  if (!cond) throw new AssertionError(message || 'expected truthy value');
}
function assertEq(actual, expected, message) {
  if (actual !== expected) {
    throw new AssertionError(
      `${message || 'values differ'}\n  expected: ${JSON.stringify(expected)}\n  actual:   ${JSON.stringify(actual)}`
    );
  }
}
function assertDeepEq(actual, expected, message) {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) {
    throw new AssertionError(
      `${message || 'deep values differ'}\n  expected: ${e}\n  actual:   ${a}`
    );
  }
}
async function assertThrows(fn, predicate, message) {
  let err = null;
  try {
    await fn();
  } catch (e) {
    err = e;
  }
  if (!err) {
    throw new AssertionError(`${message || 'expected to throw'}, but resolved`);
  }
  if (predicate && !predicate(err)) {
    throw new AssertionError(
      `${message || 'thrown error did not match predicate'}: ${err.message}`
    );
  }
  return err;
}

async function runFile(filePath, sharedCtx) {
  const tests = [];
  const ctx = {
    ...sharedCtx,
    test(name, fn) {
      tests.push({ name, fn });
    },
    assert,
    assertEq,
    assertDeepEq,
    assertThrows,
  };

  const mod = require(filePath);
  if (typeof mod !== 'function') {
    throw new Error(`Test file ${filePath} must export an async function`);
  }
  await mod(ctx);

  const results = [];
  for (const t of tests) {
    const start = Date.now();
    try {
      await t.fn();
      results.push({ name: t.name, pass: true, ms: Date.now() - start });
    } catch (e) {
      results.push({
        name: t.name,
        pass: false,
        ms: Date.now() - start,
        error: e,
      });
    }
  }
  return { file: path.basename(filePath), results };
}

async function runAll(testDir, sharedCtx) {
  const files = fs
    .readdirSync(testDir)
    .filter((f) => f.endsWith('.test.js'))
    .sort()
    .map((f) => path.join(testDir, f));

  const fileResults = [];
  let pass = 0;
  let fail = 0;

  for (const file of files) {
    const rel = path.relative(process.cwd(), file);
    console.log(`\n${'─'.repeat(60)}\n${rel}\n${'─'.repeat(60)}`);
    const r = await runFile(file, sharedCtx);
    fileResults.push(r);
    for (const t of r.results) {
      if (t.pass) {
        pass += 1;
        console.log(`  ✓ ${t.name} (${t.ms}ms)`);
      } else {
        fail += 1;
        console.log(`  ✗ ${t.name} (${t.ms}ms)`);
        const msg = (t.error && (t.error.message || String(t.error))) || '?';
        console.log(`    ${msg.split('\n').join('\n    ')}`);
      }
    }
  }

  console.log(`\n${'─'.repeat(60)}\nSummary\n${'─'.repeat(60)}`);
  console.log(`  files:  ${fileResults.length}`);
  console.log(`  passed: ${pass}`);
  console.log(`  failed: ${fail}`);
  return { pass, fail, fileResults };
}

module.exports = {
  runAll,
  AssertionError,
  assert,
  assertEq,
  assertDeepEq,
  assertThrows,
};
