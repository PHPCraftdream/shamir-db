/**
 * Connection-level invariants — what does the shared client hold after
 * the handshake completes?
 *
 * Also carries the napi-binding card regressions (RV-CLIENT-NODE-067017C8 /
 * 105a725d / f90a248f): a controlled TCP forwarding proxy gates the real
 * server's replies so request overlap and close ordering can be observed
 * deterministically through the public `shamir-client` package.
 */

'use strict';

const { ShamirClient } = require('shamir-client');
const { encode, decode } = require('@msgpack/msgpack');
const { createControlledProxy } = require('../helpers/controlled-proxy');

const HELLO = { repl_op: 'hello', proto_ver: 1, node_id: 'client-node-regression' };
const HELLO_BYTES = Buffer.from(encode(HELLO));
const TIMEOUT_MS = 5000;

function bounded(promise, message) {
  let timer;
  return Promise.race([
    promise,
    new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(message)), TIMEOUT_MS);
    }),
  ]).finally(() => clearTimeout(timer));
}

async function makeClient(server, proxy) {
  return ShamirClient.connect({
    host: proxy.host,
    port: proxy.port,
    serverName: 'localhost',
    username: server.user,
    password: server.password,
    acceptNewHost: true,
  });
}

module.exports = async function ({ client, server, test, assert, assertEq }) {
  test('rv_client_node_067017c8_repl_snapshot_survives_same_tick_mutation_loop', async () => {
    const proxy = await createControlledProxy(server.host, server.port);
    let nativeClient;
    try {
      nativeClient = await makeClient(server, proxy);
      await nativeClient.ping();
      proxy.arm();
      for (let iteration = 0; iteration < 25; iteration += 1) {
        if (iteration > 0) proxy.rearm();
        const request = Buffer.from(HELLO_BYTES);
        const pending = nativeClient.repl(request);
        const pendingGuard = pending.catch(() => {});
        // Same-tick fill: with the pre-fix worker-read of the caller's
        // Buffer, at least one iteration races this fill and fails decode.
        request.fill(0);
        await proxy.waitForClientData('replication request reaching server');
        proxy.releaseResponses();
        const response = decode(new Uint8Array(await bounded(pending, 'replication response')));
        assertEq(response.repl_kind, 'hello', `iteration ${iteration}: mutated Buffer must not change request`);
        // ReplHello contract: no server echo of the raw request bytes
        // exists, so byte-level equality is not assertable — leader_epoch
        // binds the ORIGINAL payload semantics (a zeroed buffer would not
        // decode as a hello, or would fail this contract check).
        assertEq(response.leader_epoch, 1, `iteration ${iteration}: hello must report leader_epoch 1`);
        await pendingGuard;
      }
    } finally {
      await proxy.close();
      if (nativeClient) await bounded(nativeClient.close().catch(() => {}), 'cleanup close');
    }
  });

  test('rv_client_node_067017c8_native_pin_validation_precedes_dispatch', () => {
    const nativeBinding = Object.getPrototypeOf(ShamirClient);
    let synchronousError;
    try {
      const result = nativeBinding.connect({
        host: server.host,
        port: server.port,
        serverName: 'localhost',
        username: server.user,
        password: server.password,
        trustedPin: Buffer.alloc(31),
      });
      result.catch(() => {});
    } catch (error) {
      synchronousError = error;
    }
    assert(synchronousError instanceof Error, 'invalid native trustedPin must reject before Promise dispatch');
  });

  test('rv_client_node_067017c8_connect_copies_trusted_pin_and_preserves_wrapper', async () => {
    const pin = Buffer.from(client.serverPubKeyPin());
    const proxy = await createControlledProxy(server.host, server.port);
    let created;
    try {
      const connecting = ShamirClient.connect({
        host: proxy.host,
        port: proxy.port,
        serverName: 'localhost',
        username: server.user,
        password: server.password,
        acceptNewHost: false,
        trustedPin: pin,
      });
      pin.fill(0);
      created = await bounded(connecting, 'connect with snapshotted trustedPin');
      assert(created instanceof ShamirClient, 'connect result must retain wrapper subclass');
    } finally {
      await proxy.close();
      if (created) await bounded(created.close().catch(() => {}), 'cleanup connect close');
    }
  });

  test('rv_client_node_105a725d_second_public_request_is_sent_before_first_reply', async () => {
    const proxy = await createControlledProxy(server.host, server.port);
    let nativeClient;
    let firstGuard;
    let secondGuard;
    try {
      nativeClient = await makeClient(server, proxy);
      await nativeClient.ping();
      proxy.arm();
      const first = nativeClient.ping();
      firstGuard = first.catch(() => {});
      await proxy.waitForClientData('first ping request');
      await proxy.waitForHeldResponse('first ping response held');
      proxy.clearObservedClientData();
      const second = nativeClient.ping();
      secondGuard = second.catch(() => {});
      await proxy.waitForClientData('second ping before first reply');
      proxy.releaseResponses();
      await bounded(Promise.all([first, second]), 'both concurrent pings');
    } finally {
      await proxy.close();
      await Promise.all([firstGuard, secondGuard].filter(Boolean));
      if (nativeClient) await bounded(nativeClient.close().catch(() => {}), 'cleanup close');
    }
  });

  test('rv_client_node_f90a248f_close_cancels_inflight_call_without_response', async () => {
    const proxy = await createControlledProxy(server.host, server.port);
    let nativeClient;
    let pendingGuard;
    try {
      nativeClient = await makeClient(server, proxy);
      await nativeClient.ping();
      const pin = nativeClient.serverPubKeyPin();
      proxy.arm();
      const pending = nativeClient.ping();
      pendingGuard = pending.catch(() => {});
      const pendingFailure = bounded(pending.then(
        () => { throw new Error('in-flight ping unexpectedly resolved'); },
        (error) => error
      ), 'in-flight call cancellation');
      pendingFailure.catch(() => {});
      await proxy.waitForClientData('ping request before close');
      await proxy.waitForHeldResponse('in-flight ping response held');
      await bounded(nativeClient.close(), 'close while response held');
      await pendingGuard;
      const error = await pendingFailure;
      assert(error instanceof Error, 'in-flight public call must reject on close');
      assert(Buffer.from(nativeClient.serverPubKeyPin()).equals(pin), 'cached pin must survive close');
      await bounded(nativeClient.ping().then(
        () => { throw new Error('request after close unexpectedly resolved'); },
        () => undefined
      ), 'request after close rejection');
      await bounded(nativeClient.close(), 'idempotent repeated close');
    } finally {
      await proxy.close();
      await pendingGuard;
      if (nativeClient) await bounded(nativeClient.close().catch(() => {}), 'cleanup close');
    }
  });

  test('session_id is 32 bytes', () => {
    const sid = client.sessionId();
    assert(Buffer.isBuffer(sid), 'sessionId must be Buffer');
    assertEq(sid.length, 32);
  });

  test('server pin (SHA256 ed25519) is 32 bytes and non-zero', () => {
    const pin = client.serverPubKeyPin();
    assertEq(pin.length, 32);
    assert(
      pin.some((b) => b !== 0),
      'pin must not be all zeros'
    );
  });

  test('expires_at_ns is a future BigInt', () => {
    const exp = client.expiresAtNs();
    assertEq(typeof exp, 'bigint');
    const nowNs = BigInt(Date.now()) * 1_000_000n;
    assert(exp > nowNs, `expiry ${exp} must be after now ${nowNs}`);
    // Spec §5: max session age = 24 h. Allow for some clock skew but
    // sanity-check we aren't getting a thousand-year-future expiry.
    const dayPlusNs = nowNs + 25n * 3600n * 1_000_000_000n;
    assert(exp < dayPlusNs, `expiry ${exp} must be within 24 h, got ${exp - nowNs} ns`);
  });

  test('resumption ticket is issued (non-empty)', () => {
    const ticket = client.resumptionTicket();
    assert(ticket !== null, 'expected a resumption ticket');
    assert(ticket.length > 0, 'ticket must be non-empty');
    const exp = client.resumptionExpiresAtNs();
    assertEq(typeof exp, 'bigint');
    assert(exp > 0n, 'resumption expiry must be set');
  });

  test('ping is idempotent', async () => {
    await client.ping();
    await client.ping();
    await client.ping();
  });
};
