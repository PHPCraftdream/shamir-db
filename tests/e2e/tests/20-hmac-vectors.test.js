'use strict';

const assert = require('assert');
const fs = require('fs');
const path = require('path');
const hmac = require('../helpers/hmac');
const { ddl } = require('@shamir/client');
const vectorsPath = path.resolve(__dirname, '../../../crates/shamir-query-types/test-vectors/admin_hmac_v2.json');
const vectors = JSON.parse(fs.readFileSync(vectorsPath, 'utf8'));

module.exports = async function ({ test, assertEq }) {
  test('rv_query_types_f1dc8e9a verifies all HMAC known-answer vectors and TS builder', () => {
    assertEq(vectors.vectors.length, 31, 'fixture contains all 31 operations');
    assert(/^[0-9a-f]{64}$/.test(vectors.session_id_hex), 'session id is valid 32-byte hex');
    const sessionId = Buffer.from(vectors.session_id_hex, 'hex');
    assertEq(sessionId.length, 32, 'session id length');
    const clientForVector = { sessionId: () => sessionId };

    for (const vector of vectors.vectors) {
      assert(/^[0-9a-f]{4}$/.test(vector.op_tag), `invalid op_tag for ${vector.op}`);
      assert(/^[0-9a-f]+$/.test(vector.fields_hex) && vector.fields_hex.length % 2 === 0, `invalid fields_hex for ${vector.op}`);
      assert(/^[0-9a-f]+$/.test(vector.canonical_hex) && vector.canonical_hex.length % 2 === 0, `invalid canonical_hex for ${vector.op}`);
      assert(/^[0-9a-f]{64}$/.test(vector.tag_hex), `invalid tag_hex for ${vector.op}`);

      const canonical = Buffer.from(vector.canonical_hex, 'hex');
      assertEq(canonical.subarray(0, 30).toString('hex'), `7368616d69722d646220686d61632063616e6f6e6963616c20763200${vector.op_tag}`, `canonical prefix/tag for ${vector.op}`);
      assertEq(canonical.subarray(30).toString('hex'), vector.fields_hex, `canonical fields for ${vector.op}`);
      assertEq(hmac.sign(clientForVector, canonical), vector.tag_hex, `helper signing matches fixture for ${vector.op}`);
    }

    const vector = vectors.vectors.find((v) => v.op === 'drop_db');
    assert(vector, 'drop_db fixture vector exists');
    const tsBuiltOp = ddl.dropDb(
      { hmacTagHex: (bytes) => hmac.sign(clientForVector, Buffer.from(bytes)) },
      vector.fields.name,
      { cascade: vector.fields.cascade === 1, if_exists: vector.fields.if_exists === 1 },
    );
    assertEq(tsBuiltOp.hmac, vector.tag_hex, 'TS builder canonicalization and helper signing match fixture tag');
  });
};
