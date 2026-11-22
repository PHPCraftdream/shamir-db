# RecordId on the wire

Applies to protocol v4+ (alpha; pre-v1 clients are in-tree and were updated
in the same change — no compatibility shim, per the P1 remediation ruling
RU6).

## Fields

Exactly four wire-visible fields carry a `RecordId` as a **canonical base58
string**, never as raw MessagePack bytes (`GetRecord.key` is the sole `bin`
exception — see below):

| Message | Field |
|---|---|
| `QueryResult` | `op_id` |
| `DdlOpStatus` (inside the `ddl_op_status` response envelope) | `op_id` |
| `DropIndexOp` | `request_id` |
| `RenameIndexOp` | `request_id` |

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
- every OTHER surface keeps the base58 **string** form listed in the table
  above — do not "unify" this field with them.

## Format

- base58btc over the 16 raw id bytes read big-endian, alphabet
  `123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz`;
- every leading `0x00` byte becomes one leading `'1'`;
- length is 16–22 characters; equals `RecordId::to_string()` in Rust.

The value marker after each field key is always a MessagePack **str**
(fixstr `0xa0`–`0xbf` / str8/16/32) — never `bin` (`0xc4`+).

## Decode

Strict: only a canonical base58 string is accepted. Raw bytes (`bin`), a
non-canonical string, or any other type is a decode error. There is no
legacy-bin decoder.

## Poll token

`GetDdlOpStatus.op_id` is a string. The poll token is the response's
`op_id` string **byte-for-byte**: decode the response, take the string, put
it unchanged into the next request. The TS client types all four fields as
`string` and requires no translation.

## Unchanged

- `RecordId`'s own `Serialize`/`Deserialize` (raw bytes) — used by the
  bincode **disk** encoding of the DDL op log, which stays byte-identical
  (version `0x01`).
- `CorruptRecordRef.id`, which already used the base58 string convention.
