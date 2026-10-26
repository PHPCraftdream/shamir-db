//! Disk-mirror golden test for the DDL op-status log (RU6, item 4):
//! `DdlOpStatus` keeps its pre-RU6 bincode bytes — the wire change
//! (base58 string `op_id`) must not leak into `ddl_op_log` storage.
//!
//! BOTH directions go through the production fns
//! (`base_index::infra::ddl_op_log::{encode_disk_status, decode_op_status}`),
//! i.e. the exact `write_op_status` / `read_op_status` persistence path —
//! no direct `bincode::serialize(&DdlOpStatus)`, which after the wire
//! change would encode the base58-string wire form instead of the disk form.
//!
//! Oracle derivation (no runtime capture possible — tests run in CI only):
//! the disk record is `[0x01 version byte][bincode(DiskDdlOpStatus)]`;
//! bincode (default config, little-endian, u64 lengths) of
//! `{ op_id: RecordId, kind, state }` with `op_id` bytes `0x01..=0x10`
//! (`serialize_bytes` → `[u64 len=16][bytes]`), `kind = Other { description:
//! "x" }` (variant index 10), `state = InProgress` (variant index 0):
//! `[u64 len=16][01..10] | u32 10 | [u64 len=1]['x'] | u32 0`.

use bytes::Bytes;

use crate::base_index::infra::ddl_op_log::{decode_op_status, encode_disk_status};

use shamir_query_types::read::{DdlOpKind, DdlOpState, DdlOpStatus};
use shamir_types::types::record_id::RecordId;

#[test]
fn rv_query_types_a984dfed_ddl_op_log_disk_bytes_unchanged() {
    let op_id = RecordId::try_from_bytes(&(0x01u8..=0x10).collect::<Vec<u8>>()).unwrap();
    let status = DdlOpStatus {
        op_id,
        kind: DdlOpKind::Other {
            description: "x".to_string(),
        },
        state: DdlOpState::InProgress,
    };

    // Production encode path — [VERSION_BYTE | bincode(DiskDdlOpStatus)].
    let bytes = encode_disk_status(&status).expect("encode_disk_status");

    let mut expected = Vec::new();
    expected.push(0x01); // DDL_OP_LOG_VERSION
    expected.extend_from_slice(&16u64.to_le_bytes());
    expected.extend_from_slice(op_id.as_bytes());
    expected.extend_from_slice(&10u32.to_le_bytes()); // DdlOpKind::Other
    expected.extend_from_slice(&1u64.to_le_bytes());
    expected.push(b'x');
    expected.extend_from_slice(&0u32.to_le_bytes()); // DdlOpState::InProgress
    assert_eq!(
        bytes, expected,
        "disk bytes must be [0x01][bincode payload]"
    );

    // Production decode path round-trips the same bytes.
    let back = decode_op_status(&Bytes::from(bytes))
        .expect("decode_op_status")
        .expect("record present");
    assert_eq!(back.op_id, op_id);
    assert_eq!(back.kind, status.kind);
    assert!(matches!(back.state, DdlOpState::InProgress));
}
