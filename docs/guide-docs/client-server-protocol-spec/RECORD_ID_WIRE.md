# RecordId on the wire

Applies to protocol v4+ (alpha; pre-v1 clients are in-tree and were updated
in the same change — no compatibility shim, per the P1 remediation ruling
RU6).

## Convention

A wire-visible `RecordId` travels as a **canonical base58 string**, never as
raw MessagePack bytes. There is exactly ONE `bin` exception — `GetRecord.key`
(below). The tables below enumerate every field, so "string" is a rule you
can check, not an inference.

## Core result and correlation fields (base58 strings)

| Message | Field |
|---|---|
| `QueryResult` | `op_id` |
| `DdlOpStatus` (inside the `ddl_op_status` response envelope) | `op_id` |
| `DropIndexOp` | `request_id` |
| `RenameIndexOp` | `request_id` |

These four use the shared `record_id_string` codec
(`crates/query/shamir-query-types/src/wire/record_id_string.rs`, the
`option` submodule for the two optional `request_id`s).

## Other fields that also carry a `RecordId` as a base58 string

The four above are the core result/correlation set — NOT a closed inventory
of "exactly four" fields. The same string convention is used elsewhere:

| Message | Field | Codec |
|---|---|---|
| `Pagination::After` (batch read) | `after_id` | `opt_record_id_base58` (`read/clause/limit.rs`) |
| `SchemaMigrationTarget` (plan/apply) | `database_identity` | `record_id_string` (`admin/types/schema_migration/protocol/request.rs`) |
| `InsertedRecord` (write result) | `_id` | emitted as `RecordId::to_string()` (`ops/write/inserted_record.rs`) |
| read result row | `_id` | a base58 string, same convention |
| `CorruptRecordRef` (scan diagnostics) | `id` | `id_as_base58_string` (`read/core/query_result.rs`) |
| `GetDdlOpStatus` (request) | `op_id` | a plain `String` field — the poll token below |

The value marker after each such field key is always a MessagePack **str**
(fixstr `0xa0`–`0xbf` / str8/16/32) — never `bin` (`0xc4`+). The string
codecs reject a `bin` payload even when its bytes are the ASCII of the
canonical token.

## The one `bin` exception: `GetRecord.key`

`DbRequest::GetRecord.key` (admin-shell P2 exact-key point read) is the ONE
place a record id travels as **raw MessagePack `bin`**, never as a base58
string:

- the value marker is `bin8` (`0xc4`), its length byte `0x10`, then EXACTLY
  16 raw id bytes — 18 bytes of msgpack in total
  (`serde_bytes::ByteArray<16>` in
  `crates/query/shamir-query-types/src/wire/db_message.rs`,
  `mod record_id_wire`);
- the width is fixed: a 15-byte or 17-byte payload is a typed decode error,
  never a silently re-sized id, and the sequence form is bounded by the same
  fixed size (no unbounded allocation);
- every OTHER surface keeps the base58 **string** form listed in the tables
  above — do not "unify" this field with them.

## Format

- base58btc over the 16 raw id bytes read big-endian, alphabet
  `123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz`;
- every leading `0x00` byte becomes one leading `'1'`;
- length is 16–22 characters; 22 is the **maximum**, not the only canonical
  form — an id with leading zero bytes encodes shorter. It equals
  `RecordId::to_string()` in Rust.

## Decode

**String fields (the default).** Strict: only a canonical base58 string is
accepted. Raw bytes (`bin`), a non-canonical string, or any other type is a
decode error. There is no legacy-bin decoder.

**`GetRecord.key` (the fixed-16-byte field).** Its decoder is
`serde_bytes::ByteArray<16>`, which accepts, besides the `bin8` form: a
MessagePack array of exactly 16 integers (the `visit_seq` path, bounded by
the fixed size), and a MessagePack `str` whose UTF-8 length is exactly 16
raw bytes (`visit_str` forwards to the byte path). Neither of the last two
is a base58 decode — they carry raw id bytes. A payload of any of these
shapes whose width is not exactly 16 is a typed decode error.

## Poll token

`GetDdlOpStatus.op_id` is a string. The poll token is the response's
`op_id` string **byte-for-byte**: decode the response, take the string, put
it unchanged into the next request. The TS client types all four
core/correlation fields as `string` and requires no translation.

## Unchanged

- `RecordId`'s own `Serialize`/`Deserialize` (raw bytes) — used by the
  bincode **disk** encoding of the DDL op log, which stays byte-identical
  (version `0x01`).
- `CorruptRecordRef.id`, which already used the base58 string convention.
