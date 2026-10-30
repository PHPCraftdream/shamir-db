'use strict';

const assert = require('node:assert/strict');
const test = require('node:test');
const Module = require('node:module');
const codec = require('@msgpack/msgpack');
const { encode } = codec;
let decodedInputs = [];

// These are codec-boundary responses, not examples of query construction.
let response;
let requestBytes;
class NativeClient {
  async createScramUser() { return response; }
  async repl() { return response; }
  async setReplicator() { return response; }
  async execute(db, bytes) { requestBytes = bytes; return response; }
}

const load = Module._load;
let wrapper;
try {
  Module._load = function (request, parent, isMain) {
    if (request === '../native/index.js') return { ShamirClient: NativeClient };
    if (request === '@msgpack/msgpack') return {
      ...codec,
      decode(input, options) { decodedInputs.push(input); return codec.decode(input, options); },
    };
    return load.call(this, request, parent, isMain);
  };
  wrapper = require('../../js/wrapper.js');
} finally {
  Module._load = load;
}

test('raw user ID colliding with a kind-only error is returned unchanged', async () => {
  response = Buffer.from([0x82, 0xa4, 0x6b, 0x69, 0x6e, 0x64, 0xa5, 0x65,
    0x72, 0x72, 0x6f, 0x72, 0xa1, 0x78, 0xa1, 0x79]);
  assert.strictEqual(await new wrapper.ShamirClient().createScramUser('u', 'p', []), response);
});

test('raw response methods preserve successes and throw typed errors', async () => {
  const client = new wrapper.ShamirClient();
  response = Buffer.from(encode({ repl_kind: 'events', events: new Uint8Array([1, 2]) }));
  assert.strictEqual(await client.repl(Buffer.alloc(0)), response);
  const truncated = Buffer.from([0x83, 0xa4, 0x6b, 0x69, 0x6e, 0x64, 0xa5,
    0x65, 0x72, 0x72, 0x6f, 0x72]);
  response = truncated;
  assert.strictEqual(await client.repl(Buffer.alloc(0)), truncated);
  response = Buffer.from(encode({ kind: 'error', code: 'timeout', message: 'late' }));
  for (const call of [() => client.repl(Buffer.alloc(0)),
    () => client.createScramUser('u', 'p', []), () => client.setReplicator('u', true)]) {
    await assert.rejects(call, error => error instanceof wrapper.ShamirDbError &&
      error.code === 'timeout' && error.detail === 'late' && error.retryable);
  }
});

test('raw successes are not decoded and execute decodes the original Buffer', async () => {
  const client = new wrapper.ShamirClient();
  decodedInputs = [];
  response = Buffer.from(encode({ repl_kind: 'events', events: new Uint8Array([1, 2]) }));
  assert.strictEqual(await client.repl(Buffer.alloc(0)), response);
  response = Buffer.alloc(16, 7);
  assert.strictEqual(await client.createScramUser('u', 'p', []), response);
  assert.equal(decodedInputs.length, 0);
  response = Buffer.from(encode({ result: 3 }));
  await client.execute('db', {});
  assert.strictEqual(decodedInputs[0], response);
});

test('independent int64 and uint64 fixtures round trip without rounding', async () => {
  const client = new wrapper.ShamirClient();
  const fixtures = [
    ['d3 00 20 00 00 00 00 00 01', 9007199254740993n],
    ['d3 ff df ff ff ff ff ff ff', -9007199254740993n],
    ['d3 80 00 00 00 00 00 00 00', -(1n << 63n)],
    ['d3 7f ff ff ff ff ff ff ff', (1n << 63n) - 1n],
    ['d3 00 00 00 00 00 00 00 00', 0],
    ['cf 00 00 00 00 00 00 00 00', 0],
    ['cf 00 1f ff ff ff ff ff ff', Number.MAX_SAFE_INTEGER],
    ['d3 ff e0 00 00 00 00 00 01', Number.MIN_SAFE_INTEGER],
    ['cf 80 00 00 00 00 00 00 00', 1n << 63n],
    ['cf ff ff ff ff ff ff ff ff', (1n << 64n) - 1n],
    ['d3 ff ff ff ff ff ff ff ff', -1],
  ];
  for (const [hex, expected] of fixtures) {
    response = Buffer.from(hex.replaceAll(' ', ''), 'hex');
    const decoded = await client.execute('db', {});
    assert.equal(decoded, expected);
    await client.execute('db', BigInt(expected));
    assert.equal(codec.decode(requestBytes, { useBigInt64: true }), BigInt(expected));
  }
});

test('wide safe numbers use integer markers; nested binary and floats survive', async () => {
  const client = new wrapper.ShamirClient();
  response = Buffer.from('81a17694cf0000000100000000d3ffffffff7fffffffc402cfffcb3ff8000000000000', 'hex');
  const nested = await client.execute('db', {});
  assert.deepEqual(nested.v.slice(0, 2), [4294967296, -2147483649]);
  assert.deepEqual([...nested.v[2]], [0xcf, 0xff]);
  assert.equal(nested.v[3], 1.5);
  response = Buffer.from([0xc0]);
  await client.execute('db', [4294967296, -2147483649, 1.5, 'x', new Uint8Array([0xcf, 0xff]), Buffer.from([0xd3, 0])]);
  assert.equal(requestBytes[1], 0xcf);
  assert.equal(requestBytes[10], 0xd3);
  const decoded = codec.decode(requestBytes, { useBigInt64: true });
  assert.deepEqual(decoded.slice(0, 4), [4294967296n, -2147483649n, 1.5, 'x']);
  assert.deepEqual([...decoded[4]], [0xcf, 0xff]);
  assert.deepEqual([...decoded[5]], [0xd3, 0]);
});

test('unsafe Numbers and out-of-range BigInts are rejected before native execute', async () => {
  const client = new wrapper.ShamirClient();
  for (const value of [Number.MAX_SAFE_INTEGER + 1, Number.MIN_SAFE_INTEGER - 1,
    -(1n << 63n) - 1n, 1n << 64n]) {
    requestBytes = undefined;
    await assert.rejects(() => client.execute('db', { nested: [value] }), RangeError);
    assert.equal(requestBytes, undefined);
  }
});

test('documented actual TS builder executes through the Node wrapper', async () => {
  response = Buffer.from('81a27631cf0020000000000001', 'hex');
  const example = require('../../node_modules/.builder-test/shamir-client-node/src/tests/builder-example.js');
  const result = await example.builderExample(new wrapper.ShamirClient());
  assert.deepEqual(result, { v1: 9007199254740993n });
  const sent = codec.decode(requestBytes, { useBigInt64: true });
  assert.equal(sent.id, 'rw');
  assert.equal(sent.queries.rd.from, 'items');
  assert.equal(sent.queries.rd.temporal.at.version, 4294967296n);
  assert.notEqual(requestBytes.indexOf(Buffer.from('cf0000000100000000', 'hex')), -1);
});

test('execute decodes typed errors and ordinary success', async () => {
  const client = new wrapper.ShamirClient();
  response = Buffer.from(encode({ result: 3 }));
  assert.deepEqual(await client.execute('db', {}), { result: 3 });
  response = Buffer.from(encode({ kind: 'error', code: 'validation', message: 'bad' }));
  await assert.rejects(() => client.execute('db', {}), error =>
    error instanceof wrapper.ShamirDbError && error.code === 'validation');
});
