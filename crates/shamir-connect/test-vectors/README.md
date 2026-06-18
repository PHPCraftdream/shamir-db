# Test Vectors — auth_v1

Bit-exact reference values for spec compliance. **Release blocker per AUTH §16.**

## Files

- `auth_message_default.json` — canonical `auth_message` for the inline example in spec §16
- `auth_message_default.toml` — same vector in TOML format

## Layout

Each test vector follows this schema (stored as a plain key-value document):

```
{
  "name": "human-readable description",
  "spec_section": "AUTH §...",
  "inputs": { ... primitive inputs as hex / strings },
  "expected_hex": "byte-exact output of the operation"
}
```

## Adding new vectors

When implementing a new operation, add a vector here BEFORE writing the impl:
1. Compute the expected output by hand (or with a known-good external tool).
2. Add the vector to the appropriate file.
3. Reference the vector in a `#[test]` that asserts impl output matches.

This enforces TDD per AGENTS.md and protects cross-language interop with the
JS browser SDK (which will load these same vectors).
