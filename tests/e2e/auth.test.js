/**
 * E2E test: connect to ShamirDB over TLS, authenticate via MessagePack framing.
 *
 * Protocol:
 *   [length: 4 BE][msgpack payload: length bytes]
 *
 * Usage:
 *   1. cargo run --bin shamir-server
 *   2. cd tests/e2e && npm install && npm test
 */

const tls = require('tls');
const fs = require('fs');
const path = require('path');
const { encode, decode } = require('@msgpack/msgpack');

const HOST = '127.0.0.1';
const PORT = 3742;
const CERT_PATH = path.join(__dirname, '..', '..', 'server-cert.pem');

// --- Framing helpers ---

function encodeFrame(obj) {
  const payload = Buffer.from(encode(obj));
  const header = Buffer.alloc(4);
  header.writeUInt32BE(payload.length, 0);
  return Buffer.concat([header, payload]);
}

function createFrameReader() {
  let buffer = Buffer.alloc(0);
  const pending = [];
  let resolver = null;

  function push(chunk) {
    buffer = Buffer.concat([buffer, chunk]);
    drain();
  }

  function drain() {
    while (buffer.length >= 4) {
      const len = buffer.readUInt32BE(0);
      if (buffer.length < 4 + len) break;
      const payload = buffer.slice(4, 4 + len);
      buffer = buffer.slice(4 + len);
      const msg = decode(payload);
      if (resolver) {
        const r = resolver;
        resolver = null;
        r(msg);
      } else {
        pending.push(msg);
      }
    }
  }

  function read() {
    return new Promise((resolve) => {
      if (pending.length > 0) {
        resolve(pending.shift());
      } else {
        resolver = resolve;
      }
    });
  }

  return { push, read };
}

// --- Test helpers ---

function connect() {
  return new Promise((resolve, reject) => {
    const cert = fs.readFileSync(CERT_PATH);
    const socket = tls.connect({
      host: HOST,
      port: PORT,
      ca: [cert],
      servername: 'localhost',
    }, () => {
      if (!socket.authorized) {
        console.warn('TLS warning:', socket.authorizationError);
      }
      resolve(socket);
    });
    socket.on('error', reject);
  });
}

async function authenticate(socket, reader, user, password) {
  socket.write(encodeFrame({ user, password }));
  return await reader.read();
}

async function sendQuery(socket, reader, request) {
  socket.write(encodeFrame(request));
  return await reader.read();
}

// --- Tests ---

async function testSuccessfulAuth() {
  console.log('TEST: Successful authentication...');
  const socket = await connect();
  const reader = createFrameReader();
  socket.on('data', (chunk) => reader.push(chunk));

  const resp = await authenticate(socket, reader, 'admin', 'admin123');

  console.assert(resp.authenticated === true, 'Expected authenticated=true, got', resp);
  console.assert(typeof resp.session_id === 'string', 'Expected session_id string, got', resp);
  console.log('  OK: session_id =', resp.session_id);

  socket.end();
}

async function testFailedAuth() {
  console.log('TEST: Failed authentication (wrong password)...');
  const socket = await connect();
  const reader = createFrameReader();
  socket.on('data', (chunk) => reader.push(chunk));

  const resp = await authenticate(socket, reader, 'admin', 'wrong_password');

  console.assert(resp.authenticated === false, 'Expected authenticated=false, got', resp);
  console.assert(resp.error === 'authentication_failed', 'Expected error, got', resp);
  console.log('  OK: rejected');

  socket.end();
}

async function testAuthThenQuery() {
  console.log('TEST: Auth then query...');
  const socket = await connect();
  const reader = createFrameReader();
  socket.on('data', (chunk) => reader.push(chunk));

  // Auth
  const authResp = await authenticate(socket, reader, 'admin', 'admin123');
  console.assert(authResp.authenticated === true, 'Auth failed');

  // Insert
  const insertResp = await sendQuery(socket, reader, {
    id: 1,
    queries: {
      ins: {
        insert_into: 'users',
        values: [
          { name: 'Alice', age: 30 },
          { name: 'Bob', age: 25 },
        ]
      }
    }
  });

  console.assert(insertResp.results.ins.records.length === 2, 'Expected 2 inserted records');
  console.log('  OK: inserted 2 records');

  // Read
  const readResp = await sendQuery(socket, reader, {
    id: 2,
    queries: {
      all: { from: 'users' }
    }
  });

  console.assert(readResp.results.all.records.length === 2, 'Expected 2 records');
  console.log('  OK: read', readResp.results.all.records.length, 'records');

  socket.end();
}

async function testUnknownUser() {
  console.log('TEST: Unknown user...');
  const socket = await connect();
  const reader = createFrameReader();
  socket.on('data', (chunk) => reader.push(chunk));

  const resp = await authenticate(socket, reader, 'nobody', 'pass');

  console.assert(resp.authenticated === false, 'Expected rejected');
  console.log('  OK: rejected unknown user');

  socket.end();
}

// --- Run ---

async function main() {
  console.log(`\nConnecting to ShamirDB at ${HOST}:${PORT}...\n`);

  try {
    await testSuccessfulAuth();
    await testFailedAuth();
    await testUnknownUser();
    await testAuthThenQuery();

    console.log('\n✅ All tests passed!\n');
  } catch (e) {
    console.error('\n❌ Test failed:', e.message || e);
    process.exit(1);
  }
}

main();
