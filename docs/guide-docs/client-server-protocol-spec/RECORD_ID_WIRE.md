# RecordId on the wire

Applies to protocol v4+ (alpha; pre-v1 clients are in-tree and were updated
in the same change — no compatibility shim, per the P1 remediation ruling
RU6).

## Fields

Exactly four wire-visible fields carry a `RecordId` as a **canonical base58
string**, never as raw MessagePack bytes:

| Message | Field |
|---|---|
| `QueryResult` | `op_id` |
| `DdlOpStatus` (inside the `ddl_op_status` response envelope) | `op_id` |
| `DropIndexOp` | `request_id` |
| `RenameIndexOp` | `request_id` |

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
