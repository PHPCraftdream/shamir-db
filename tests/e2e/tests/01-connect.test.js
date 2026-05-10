/**
 * Connection-level invariants — what does the shared client hold after
 * the handshake completes?
 */

'use strict';

module.exports = async function ({ client, test, assert, assertEq }) {
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
