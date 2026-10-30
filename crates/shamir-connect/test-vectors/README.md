# Test Vectors — auth_v1

Bit-exact reference values for spec compliance. **Release blocker per AUTH §16.**

Most existing cryptographic vectors are fixed values computed by the Rust crypto
functions. The complete message-layout fixtures listed below were independently
assembled from their explicit inputs.

## Location & format

Vectors live here in `crates/shamir-connect/test-vectors/`. Existing crypto
vectors use JSON+TOML pairs; the two message-layout fixtures are TOML-only.

- `.json` — canonical cross-language source of truth (a browser/TS SDK loads
  these directly).
- `.toml` — the same vector; consumed by the Rust test suite via
  `include_str!` + `toml::from_str`.

> The original AUTH §16 prose named a single `auth_v1.msgpack` blob. That file
> never existed; the per-vector JSON+TOML convention is the real, established,
> working one. Msgpack is used only internally for wire serialization
> (e.g. `TicketPlain`); the vectors are JSON+TOML for legibility and
> git-diffability. §16 has been rewritten to describe this reality.

Each file follows this schema:

```
{
  "name": "human-readable description",
  "spec_section": "AUTH §...",
  "inputs": { ... primitive inputs as hex / strings },
  "expected": { ... byte-exact output of the operation as hex }
}
```

## Files

All vectors share ONE coherent fixed scenario (username `"alice"`, fixed
nonces/salt, `KdfParams::DEFAULT`, transport_kind=tcp, binding_mode=
tls_exporter) so they chain end-to-end: auth_message → Argon2id → SCRAM proofs
→ identity_sig → rotation.

| File | Category | What it pins |
|---|---|---|
| `auth_message_default.{json,toml}` | auth_message | 148-byte canonical auth_message (spec §16 inline example) |
| `changepw_message.toml` | password change | Complete message bytes and inputs |
| `bootstrap_message.toml` | bootstrap | Complete message bytes and inputs |
| `kdf_canonical_string_default.{json,toml}` | kdf_canonical_string | 13-byte BE serialization of KdfParams |
| `argon2id_default.{json,toml}` | Argon2id | 32-byte salted_password for `"hello world!1"` + fixed salt + DEFAULT params |
| `scram_flow_default.{json,toml}` | SCRAM chain | client_key / server_key / stored_key / client_proof / server_signature over the pinned auth_message |
| `identity_sig_default.{json,toml}` | identity_sig | Ed25519 (fixed seed) identity_input + 64-byte signature |
| `fake_blob_default.{json,toml}` | fake_blob | 80-byte HKDF blob (salt‖stored_key‖server_key) for a fixed unknown user |
| `resumption_ticket_roundtrip.{json,toml}` | resumption ticket | AES-256-GCM encrypt/decrypt over a realistic TicketPlain (msgpack) |
| `identity_rotation_signed_by_old.{json,toml}` | identity rotation | Ed25519 old→new rotation event signature (signed_by_old) |

## Verification

Every vector is asserted byte-for-byte by `common/tests/test_vectors_tests.rs`,
which re-runs the real production function with the fixed `inputs` and compares
against the pinned `expected`. An implementation drift (domain-tag reorder,
HKDF info-string change, KdfParams encoding change) fails the test loudly.

## Adding new vectors

1. Run the real function with fixed inputs and capture its hex output (never
   hand-compute crypto).
2. Add a `.json` + `.toml` pair here with the same schema.
3. Add a byte-for-byte assertion in `test_vectors_tests.rs`.

This protects cross-language interop with the JS browser SDK (which loads these
same vectors) and pins exact bytes against silent regression.
